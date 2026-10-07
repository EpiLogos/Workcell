//! Capacity admission: observed host envelope, declared task budget, and a
//! durable ledger of committed allocations.
//!
//! Three distinct quantities are kept apart on purpose:
//!
//! 1. **Observed envelope** — what this machine actually reports (memory
//!    totals/availability, cgroup limits, CPU count). Never invented.
//! 2. **Task budget** — the enforceable ceiling this Workcell may commit to
//!    agent work. It is derived from the envelope by a visible, adjustable
//!    policy so the resident field (gateways, services, desktop) is protected
//!    while useful work remains possible.
//! 3. **Committed allocations** — the sum of *minimum* requirements of live
//!    material, accounted simultaneously, not one request at a time.
//!
//! Admission accounts for simultaneous allocations: a second allocation that
//! independently fits but does not fit beside the first is refused (or waits)
//! with the actual reason — which resource, committed, ceiling. Reservations
//! are released through the native material lifecycle, reconciled away after
//! failure, and re-created on re-entry from durable world provenance. Shared
//! resident costs stay in the envelope/budget layer; per-world commitments are
//! attributable task costs.

use std::{
    collections::BTreeMap,
    fs,
    path::{Path, PathBuf},
    thread::available_parallelism,
};

use serde_json::{json, Value};

use epilogos_workcell_core::{Result, WorkcellError};

use crate::host::now_unix_ms;

pub const ADMISSION_SCHEMA: &str = "workcell.admission/v1";
pub const ADMISSION_FILE: &str = "admission.json";

/// Memory units the demand grammar accepts for admission accounting. These
/// mirror the provider unit tables (binary units); unknown units refuse rather
/// than guess.
pub fn memory_to_bytes(amount: u64, unit: Option<&str>) -> Result<u64> {
    let multiplier: u64 = match unit.map(str::to_ascii_lowercase).as_deref() {
        None | Some("b") | Some("bytes") => 1,
        Some("kib") | Some("ki") => 1024,
        Some("mib") | Some("mi") => 1024_u64.pow(2),
        Some("gib") | Some("gi") => 1024_u64.pow(3),
        Some("tib") | Some("ti") => 1024_u64.pow(4),
        Some(other) => {
            return Err(WorkcellError::InvalidDemand(format!(
                "admission cannot interpret memory unit `{other}`"
            )))
        }
    };
    amount
        .checked_mul(multiplier)
        .ok_or_else(|| WorkcellError::InvalidDemand("memory requirement overflowed".into()))
}

/// CPU units for admission: a plain count, or millicpu (`m`), which rounds a
/// minimum *up* because a fractional floor must still be fully reservable.
pub fn cpu_to_milli_cpu(amount: u64, unit: Option<&str>) -> Result<u64> {
    match unit.map(str::to_ascii_lowercase).as_deref() {
        None | Some("count") | Some("cpu") | Some("cpus") | Some("cores") => amount
            .checked_mul(1000)
            .ok_or_else(|| WorkcellError::InvalidDemand("CPU requirement overflowed".into())),
        Some("m") => Ok(amount),
        Some(other) => Err(WorkcellError::InvalidDemand(format!(
            "admission cannot interpret CPU unit `{other}`"
        ))),
    }
}

/// Facts this host actually reports. Every field is a reading, not a default.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct HostObservation {
    /// `MemTotal` from `/proc/meminfo`.
    pub memory_total_bytes: u64,
    /// `MemAvailable` from `/proc/meminfo` at observation time.
    pub memory_available_bytes: u64,
    /// The observing process's cgroup v2 `memory.max`; `None` means "max".
    pub cgroup_memory_limit_bytes: Option<u64>,
    /// The observing process's cgroup v2 `memory.current` (resident usage of
    /// everything in this scope, including shared services).
    pub cgroup_memory_current_bytes: Option<u64>,
    /// Schedulable parallelism (respects cgroup cpuset/quota on Linux).
    pub cpu_count: u32,
    /// Observed at.
    pub observed_at_unix_ms: u64,
}

/// The observation seam, so tests can inject machine facts instead of lying
/// about them.
pub trait HostObserver {
    fn observe(&self) -> Result<HostObservation>;
}

/// Linux observer: `/proc/meminfo`, the process's own cgroup v2 limit, and
/// `available_parallelism`. Any unreadable optional field is honestly `None`.
pub struct LinuxHostObserver;

impl LinuxHostObserver {
    fn read_meminfo(field: &str) -> Option<u64> {
        let text = fs::read_to_string("/proc/meminfo").ok()?;
        let label = format!("{field}:");
        for line in text.lines() {
            let mut parts = line.split_whitespace();
            if parts.next() == Some(label.as_str()) {
                let value = parts.next()?.parse::<u64>().ok()?;
                // /proc/meminfo reports KiB.
                return value.checked_mul(1024);
            }
        }
        None
    }

    fn cgroup_v2_path() -> Option<PathBuf> {
        let text = fs::read_to_string("/proc/self/cgroup").ok()?;
        // cgroup v2: `0::/path`
        let line = text.lines().find(|line| line.starts_with("0::"))?;
        let relative = line.trim_start_matches("0::").trim();
        Some(Path::new("/sys/fs/cgroup").join(relative.trim_start_matches('/')))
    }

    fn read_cgroup_u64(file: &str) -> Option<u64> {
        let path = Self::cgroup_v2_path()?.join(file);
        let text = fs::read_to_string(path).ok()?;
        let trimmed = text.trim();
        if trimmed == "max" {
            return None;
        }
        trimmed.parse::<u64>().ok()
    }
}

impl HostObserver for LinuxHostObserver {
    fn observe(&self) -> Result<HostObservation> {
        let memory_total_bytes = Self::read_meminfo("MemTotal").ok_or_else(|| {
            WorkcellError::Unavailable("host observation: /proc/meminfo MemTotal unreadable".into())
        })?;
        let memory_available_bytes = Self::read_meminfo("MemAvailable").ok_or_else(|| {
            WorkcellError::Unavailable(
                "host observation: /proc/meminfo MemAvailable unreadable".into(),
            )
        })?;
        let cpu_count = available_parallelism()
            .map(|value| value.get() as u32)
            .map_err(|error| {
                WorkcellError::Unavailable(format!(
                    "host observation: schedulable parallelism unreadable: {error}"
                ))
            })?;
        Ok(HostObservation {
            memory_total_bytes,
            memory_available_bytes,
            cgroup_memory_limit_bytes: Self::read_cgroup_u64("memory.max"),
            cgroup_memory_current_bytes: Self::read_cgroup_u64("memory.current"),
            cpu_count,
            observed_at_unix_ms: now_unix_ms()?,
        })
    }
}

/// The visible policy that turns an observed envelope into a task budget.
/// Defaults are policy, stated here and reported in every readback; the
/// *numbers* they operate on are always this machine's readings.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct BudgetPolicy {
    /// Fraction (in basis points) of the enforceable memory ceiling this
    /// Workcell may commit to task allocations. Default 4000 = 40%.
    pub task_memory_fraction_bp: u16,
    /// CPUs held back for the resident field. Default 1.
    pub resident_cpu_reserve: u32,
}

impl Default for BudgetPolicy {
    fn default() -> Self {
        Self {
            task_memory_fraction_bp: 4000,
            resident_cpu_reserve: 1,
        }
    }
}

impl BudgetPolicy {
    pub const POLICY_NAME: &'static str = "fraction-of-ceiling:40%,cpu-reserve:1";
}

/// The enforceable ceiling for task allocations, derived from an observation.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CapacityBudget {
    /// Enforceable memory ceiling: the cgroup limit when finite, else total.
    pub memory_ceiling_bytes: u64,
    /// What task allocations may commit in total (policy fraction).
    pub memory_task_bytes: u64,
    /// Enforceable CPU count for tasks (parallelism minus reserve, floor 1).
    pub cpu_task_count: u32,
    pub policy: BudgetPolicy,
    pub observed_at_unix_ms: u64,
}

impl CapacityBudget {
    pub fn from_observation(observation: &HostObservation, policy: BudgetPolicy) -> Self {
        let memory_ceiling_bytes = observation
            .cgroup_memory_limit_bytes
            .unwrap_or(observation.memory_total_bytes)
            .min(observation.memory_total_bytes);
        let memory_task_bytes =
            memory_ceiling_bytes.saturating_mul(policy.task_memory_fraction_bp as u64) / 10_000;
        let cpu_task_count = observation
            .cpu_count
            .saturating_sub(policy.resident_cpu_reserve)
            .max(1);
        Self {
            memory_ceiling_bytes,
            memory_task_bytes,
            cpu_task_count,
            policy,
            observed_at_unix_ms: observation.observed_at_unix_ms,
        }
    }

    pub fn observe(observer: &dyn HostObserver, policy: BudgetPolicy) -> Result<Self> {
        Ok(Self::from_observation(&observer.observe()?, policy))
    }

    pub fn as_json(&self) -> Value {
        json!({
            "memory_ceiling_bytes": self.memory_ceiling_bytes,
            "memory_task_bytes": self.memory_task_bytes,
            "cpu_task_count": self.cpu_task_count,
            "policy": BudgetPolicy::POLICY_NAME,
            "observed_at_unix_ms": self.observed_at_unix_ms,
        })
    }
}

/// The task-shaped resources of one demand, in admitted units.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct TaskResources {
    /// The floor the host must be able to satisfy for this allocation.
    pub memory_minimum_bytes: Option<u64>,
    /// An explicit caller-authorised ceiling. Distinct from the minimum: this
    /// is what a provider may enforce (Docker `--memory`, `--cpus`).
    pub memory_maximum_bytes: Option<u64>,
    /// CPU floor in millicpu.
    pub cpu_minimum_milli: Option<u64>,
    /// Explicit CPU ceiling in millicpu.
    pub cpu_maximum_milli: Option<u64>,
}

impl TaskResources {
    /// Extract from a demand's resource requirements. Only `required`-shaped
    /// flat resources are admitted here; the planner already tiers preferred
    /// and optional requirements explicitly.
    pub fn from_resources(
        resources: &[epilogos_workcell_core::ResourceRequirement],
    ) -> Result<Self> {
        let mut parsed = TaskResources::default();
        for resource in resources {
            match resource.key.as_str() {
                "memory" => {
                    if let Some(minimum) = resource.minimum {
                        let bytes = memory_to_bytes(minimum, resource.unit.as_deref())?;
                        if parsed
                            .memory_minimum_bytes
                            .is_some_and(|existing| existing > bytes)
                        {
                            return Err(WorkcellError::InvalidDemand(
                                "demand declares two conflicting memory minimums".into(),
                            ));
                        }
                        parsed.memory_minimum_bytes =
                            Some(bytes.max(parsed.memory_minimum_bytes.unwrap_or(0)));
                    }
                    if let Some(maximum) = resource.maximum {
                        let bytes = memory_to_bytes(maximum, resource.unit.as_deref())?;
                        parsed.memory_maximum_bytes = Some(
                            parsed
                                .memory_maximum_bytes
                                .map_or(bytes, |existing| existing.min(bytes)),
                        );
                    }
                }
                "cpu" | "cpus" => {
                    if let Some(minimum) = resource.minimum {
                        let milli = cpu_to_milli_cpu(minimum, resource.unit.as_deref())?;
                        parsed.cpu_minimum_milli = Some(
                            parsed
                                .cpu_minimum_milli
                                .map_or(milli, |existing| existing.max(milli)),
                        );
                    }
                    if let Some(maximum) = resource.maximum {
                        let milli = cpu_to_milli_cpu(maximum, resource.unit.as_deref())?;
                        parsed.cpu_maximum_milli = Some(
                            parsed
                                .cpu_maximum_milli
                                .map_or(milli, |existing| existing.min(milli)),
                        );
                    }
                }
                // Other resource keys (gpu, …) are not host-admitted here;
                // their providers decide. Admission records what it can prove.
                _ => {}
            }
        }
        if let (Some(minimum), Some(maximum)) =
            (parsed.memory_minimum_bytes, parsed.memory_maximum_bytes)
        {
            if minimum > maximum {
                return Err(WorkcellError::InvalidDemand(format!(
                    "memory minimum {minimum}B exceeds declared ceiling {maximum}B"
                )));
            }
        }
        if let (Some(minimum), Some(maximum)) = (parsed.cpu_minimum_milli, parsed.cpu_maximum_milli)
        {
            if minimum > maximum {
                return Err(WorkcellError::InvalidDemand(format!(
                    "CPU minimum {}m exceeds declared ceiling {}m",
                    minimum, maximum
                )));
            }
        }
        Ok(parsed)
    }

    pub fn is_empty(&self) -> bool {
        self.memory_minimum_bytes.is_none()
            && self.memory_maximum_bytes.is_none()
            && self.cpu_minimum_milli.is_none()
            && self.cpu_maximum_milli.is_none()
    }

    pub fn as_json(&self) -> Value {
        let mut value = serde_json::Map::new();
        if let Some(minimum) = self.memory_minimum_bytes {
            value.insert("memory_minimum_bytes".into(), json!(minimum));
        }
        if let Some(maximum) = self.memory_maximum_bytes {
            value.insert("memory_maximum_bytes".into(), json!(maximum));
        }
        if let Some(minimum) = self.cpu_minimum_milli {
            value.insert("cpu_minimum_millicpu".into(), json!(minimum));
        }
        if let Some(maximum) = self.cpu_maximum_milli {
            value.insert("cpu_maximum_millicpu".into(), json!(maximum));
        }
        Value::Object(value)
    }

    fn from_json(value: &Value) -> Result<Self> {
        let object = value.as_object().ok_or_else(|| {
            WorkcellError::OperationFailed("admission reservation must be an object".into())
        })?;
        let get_u64 = |key: &str| -> Result<Option<u64>> {
            object
                .get(key)
                .map(|value| {
                    value.as_u64().ok_or_else(|| {
                        WorkcellError::OperationFailed(format!(
                            "admission reservation field `{key}` must be a number"
                        ))
                    })
                })
                .transpose()
        };
        Ok(Self {
            memory_minimum_bytes: get_u64("memory_minimum_bytes")?,
            memory_maximum_bytes: get_u64("memory_maximum_bytes")?,
            cpu_minimum_milli: get_u64("cpu_minimum_millicpu")?,
            cpu_maximum_milli: get_u64("cpu_maximum_millicpu")?,
        })
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Reservation {
    pub demand_ref: String,
    pub world_ref: Option<String>,
    pub resources: TaskResources,
    pub reserved_at_unix_ms: u64,
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct DeclinedAdmission {
    at_unix_ms: u64,
    demand_ref: String,
    reason: String,
}

/// Durable admission ledger. One file per state root; a Workcell host (CLI or
/// Control Service) holds the kernel lock for its own state, and the file is
/// rewritten atomically on every mutation.
#[derive(Clone, Debug)]
pub struct AdmissionLedger {
    path: PathBuf,
    reservations: Vec<Reservation>,
    declined: Vec<DeclinedAdmission>,
}

impl AdmissionLedger {
    pub fn load(state_root: &Path) -> Result<Self> {
        let path = state_root.join(ADMISSION_FILE);
        let mut ledger = Self {
            path,
            reservations: Vec::new(),
            declined: Vec::new(),
        };
        match fs::read_to_string(&ledger.path) {
            Ok(text) => {
                let value: Value = serde_json::from_str(&text).map_err(|error| {
                    WorkcellError::OperationFailed(format!(
                        "parse admission ledger `{}`: {error}",
                        ledger.path.display()
                    ))
                })?;
                let object = value.as_object().ok_or_else(|| {
                    WorkcellError::OperationFailed("admission ledger must be an object".into())
                })?;
                if object.get("schema").and_then(Value::as_str) != Some(ADMISSION_SCHEMA) {
                    return Err(WorkcellError::OperationFailed(
                        "admission ledger has an unknown schema".into(),
                    ));
                }
                if let Some(entries) = object.get("reservations").and_then(Value::as_array) {
                    for entry in entries {
                        let demand_ref = entry
                            .get("demand_ref")
                            .and_then(Value::as_str)
                            .ok_or_else(|| {
                                WorkcellError::OperationFailed(
                                    "admission reservation has no demand_ref".into(),
                                )
                            })?
                            .to_owned();
                        let resources = entry
                            .get("resources")
                            .map(TaskResources::from_json)
                            .transpose()?
                            .unwrap_or_default();
                        ledger.reservations.push(Reservation {
                            world_ref: entry
                                .get("world_ref")
                                .and_then(Value::as_str)
                                .map(str::to_owned),
                            demand_ref,
                            resources,
                            reserved_at_unix_ms: entry
                                .get("reserved_at_unix_ms")
                                .and_then(Value::as_u64)
                                .unwrap_or_default(),
                        });
                    }
                }
                if let Some(entries) = object.get("declined").and_then(Value::as_array) {
                    for entry in entries {
                        ledger.declined.push(DeclinedAdmission {
                            at_unix_ms: entry
                                .get("at_unix_ms")
                                .and_then(Value::as_u64)
                                .unwrap_or_default(),
                            demand_ref: entry
                                .get("demand_ref")
                                .and_then(Value::as_str)
                                .unwrap_or_default()
                                .to_owned(),
                            reason: entry
                                .get("reason")
                                .and_then(Value::as_str)
                                .unwrap_or_default()
                                .to_owned(),
                        });
                    }
                }
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => {
                return Err(WorkcellError::OperationFailed(format!(
                    "read admission ledger `{}`: {error}",
                    ledger.path.display()
                )))
            }
        }
        Ok(ledger)
    }

    fn save(&self) -> Result<()> {
        if let Some(parent) = self.path.parent() {
            fs::create_dir_all(parent).map_err(|error| {
                WorkcellError::OperationFailed(format!("create admission state directory: {error}"))
            })?;
        }
        let value = json!({
            "schema": ADMISSION_SCHEMA,
            "reservations": self.reservations.iter().map(|reservation| {
                let mut entry = json!({
                    "demand_ref": reservation.demand_ref,
                    "resources": reservation.resources.as_json(),
                    "reserved_at_unix_ms": reservation.reserved_at_unix_ms,
                });
                if let Some(world_ref) = &reservation.world_ref {
                    entry["world_ref"] = json!(world_ref);
                }
                entry
            }).collect::<Vec<_>>(),
            "declined": self.declined.iter().map(|declined| json!({
                "at_unix_ms": declined.at_unix_ms,
                "demand_ref": declined.demand_ref,
                "reason": declined.reason,
            })).collect::<Vec<_>>(),
        });
        let writing = self.path.with_extension("json.writing");
        fs::write(
            &writing,
            serde_json::to_string(&value).map_err(|error| {
                WorkcellError::OperationFailed(format!("encode admission ledger: {error}"))
            })?,
        )
        .map_err(|error| {
            WorkcellError::OperationFailed(format!(
                "write admission ledger `{}`: {error}",
                writing.display()
            ))
        })?;
        fs::rename(&writing, &self.path).map_err(|error| {
            WorkcellError::OperationFailed(format!(
                "publish admission ledger `{}`: {error}",
                self.path.display()
            ))
        })?;
        Ok(())
    }

    fn committed(&self, exclude_demand: Option<&str>) -> (u64, u64) {
        let mut memory_bytes = 0_u64;
        let mut cpu_milli = 0_u64;
        for reservation in &self.reservations {
            if exclude_demand == Some(reservation.demand_ref.as_str()) {
                continue;
            }
            memory_bytes += reservation.resources.memory_minimum_bytes.unwrap_or(0);
            cpu_milli += reservation.resources.cpu_minimum_milli.unwrap_or(0);
        }
        (memory_bytes, cpu_milli)
    }

    /// Account a new allocation simultaneously with every live one. A refusal
    /// is a *waiting for capacity* outcome with the actual reason: the demand
    /// is well-formed and its provider exists, but the host cannot commit this
    /// floor beside what is already committed. It is not provider
    /// unavailability and never silently reroutes to weaker placement.
    pub fn admit(
        &mut self,
        budget: &CapacityBudget,
        demand_ref: &str,
        resources: &TaskResources,
    ) -> Result<()> {
        if resources.is_empty() {
            return Ok(());
        }
        let (committed_memory, committed_cpu) = self.committed(None);
        let mut reasons: Vec<String> = Vec::new();
        if let Some(minimum) = resources.memory_minimum_bytes {
            if minimum > budget.memory_task_bytes {
                reasons.push(format!(
                    "memory minimum {minimum}B exceeds the whole task budget {}B",
                    budget.memory_task_bytes
                ));
            } else if committed_memory + minimum > budget.memory_task_bytes {
                reasons.push(format!(
                    "memory floor {minimum}B plus committed {committed_memory}B exceeds task budget {}B",
                    budget.memory_task_bytes
                ));
            }
        }
        if let Some(minimum) = resources.cpu_minimum_milli {
            let budget_milli = budget.cpu_task_count as u64 * 1000;
            if minimum > budget_milli {
                reasons.push(format!(
                    "CPU minimum {minimum}m exceeds the whole task budget {budget_milli}m"
                ));
            } else if committed_cpu + minimum > budget_milli {
                reasons.push(format!(
                    "CPU floor {minimum}m plus committed {committed_cpu}m exceeds task budget {budget_milli}m"
                ));
            }
        }
        if let Some(reason) = reasons.into_iter().next() {
            let declined = DeclinedAdmission {
                at_unix_ms: now_unix_ms()?,
                demand_ref: demand_ref.to_owned(),
                reason: reason.clone(),
            };
            self.declined.push(declined);
            let overflow = self.declined.len().saturating_sub(32);
            if overflow > 0 {
                self.declined.drain(0..overflow);
            }
            self.save()?;
            return Err(WorkcellError::Capacity(format!(
                "demand `{demand_ref}` is waiting for capacity: {reason}"
            )));
        }
        if self
            .reservations
            .iter()
            .any(|reservation| reservation.demand_ref == demand_ref)
        {
            // Same demand re-prepared on the same basis: the reservation
            // already exists; the durable receipt layer returns the same
            // world, so no double commit is needed.
            return Ok(());
        }
        self.reservations.push(Reservation {
            demand_ref: demand_ref.to_owned(),
            world_ref: None,
            resources: resources.clone(),
            reserved_at_unix_ms: now_unix_ms()?,
        });
        self.save()
    }

    /// Bind the materialised world to its reservation so release and
    /// reconciliation can find it by world ref.
    pub fn bind_world(&mut self, demand_ref: &str, world_ref: &str) -> Result<()> {
        if let Some(reservation) = self
            .reservations
            .iter_mut()
            .find(|reservation| reservation.demand_ref == demand_ref)
        {
            if reservation.world_ref.is_none() {
                reservation.world_ref = Some(world_ref.to_owned());
                self.save()?;
            }
        }
        Ok(())
    }

    /// Re-create a reservation on re-entry from durable world provenance
    /// (recorded at first prepare). A world that returns after host restart
    /// keeps occupying the budget it was admitted under.
    pub fn reenter(
        &mut self,
        demand_ref: &str,
        world_ref: &str,
        resources: &TaskResources,
    ) -> Result<()> {
        if self
            .reservations
            .iter()
            .any(|reservation| reservation.demand_ref == demand_ref)
        {
            return self.bind_world(demand_ref, world_ref);
        }
        if resources.is_empty() {
            return Ok(());
        }
        self.reservations.push(Reservation {
            demand_ref: demand_ref.to_owned(),
            world_ref: Some(world_ref.to_owned()),
            resources: resources.clone(),
            reserved_at_unix_ms: now_unix_ms()?,
        });
        self.save()
    }

    /// The live reservation for one demand, if any.
    pub fn reservation_for(&self, demand_ref: &str) -> Option<&Reservation> {
        self.reservations
            .iter()
            .find(|reservation| reservation.demand_ref == demand_ref)
    }

    /// Release the reservation through the native material lifecycle.
    pub fn release_world(&mut self, world_ref: &str) -> Result<()> {
        let before = self.reservations.len();
        self.reservations
            .retain(|reservation| reservation.world_ref != Some(world_ref.to_owned()));
        if self.reservations.len() != before {
            self.save()?;
        }
        Ok(())
    }

    /// Release by demand ref (a failed prepare never produced a world).
    pub fn release_demand(&mut self, demand_ref: &str) -> Result<()> {
        let before = self.reservations.len();
        self.reservations
            .retain(|reservation| reservation.demand_ref != demand_ref);
        if self.reservations.len() != before {
            self.save()?;
        }
        Ok(())
    }

    /// Drop reservations for worlds that verifiably no longer exist. Reconcile
    /// after failure: a crashed provider or lost material must not hold budget
    /// forever. Reservations without a world binding (in-flight prepares) are
    /// kept — only presence facts retire them here.
    pub fn reconcile(
        &mut self,
        present_world_refs: &std::collections::BTreeSet<String>,
    ) -> Result<usize> {
        let before = self.reservations.len();
        self.reservations
            .retain(|reservation| match &reservation.world_ref {
                Some(world_ref) => present_world_refs.contains(world_ref),
                None => true,
            });
        let released = before - self.reservations.len();
        if released > 0 {
            self.save()?;
        }
        Ok(released)
    }

    pub fn as_json(&self) -> Value {
        json!({
            "schema": ADMISSION_SCHEMA,
            "reservations": self.reservations.iter().map(|reservation| {
                let mut entry = json!({
                    "demand_ref": reservation.demand_ref,
                    "resources": reservation.resources.as_json(),
                    "reserved_at_unix_ms": reservation.reserved_at_unix_ms,
                });
                if let Some(world_ref) = &reservation.world_ref {
                    entry["world_ref"] = json!(world_ref);
                }
                entry
            }).collect::<Vec<_>>(),
            "declined": self.declined.iter().map(|declined| json!({
                "at_unix_ms": declined.at_unix_ms,
                "demand_ref": declined.demand_ref,
                "reason": declined.reason,
            })).collect::<Vec<_>>(),
        })
    }

    /// The readback for status consumers: envelope, budget, committed,
    /// available and the last waiting-for-capacity reasons.
    pub fn readback(budget: &CapacityBudget, ledger: &AdmissionLedger) -> Value {
        let (committed_memory, committed_cpu) = ledger.committed(None);
        json!({
            "budget": budget.as_json(),
            "accounting_available": true,
            "committed": {
                "memory_bytes": committed_memory,
                "cpu_millicpu": committed_cpu,
                "reservations": ledger.reservations.len(),
            },
            "available": {
                "memory_bytes": budget.memory_task_bytes.saturating_sub(committed_memory),
                "cpu_millicpu": (budget.cpu_task_count as u64 * 1000)
                    .saturating_sub(committed_cpu),
            },
            "waiting": ledger.declined.iter().rev().take(8).map(|declined| json!({
                "at_unix_ms": declined.at_unix_ms,
                "demand_ref": declined.demand_ref,
                "reason": declined.reason,
            })).collect::<Vec<_>>(),
            "ledger": ledger.as_json(),
        })
    }
}

/// Read admission facts back out of durable world provenance. Absent keys are
/// absent resources; a partial record is exactly what was recorded.
pub fn task_resources_from_provenance(provenance: &BTreeMap<String, String>) -> TaskResources {
    let parse = |key: &str| -> Option<u64> {
        provenance
            .get(key)
            .and_then(|value| value.parse::<u64>().ok())
    };
    TaskResources {
        memory_minimum_bytes: parse("admission_memory_minimum_bytes"),
        memory_maximum_bytes: parse("admission_memory_maximum_bytes"),
        cpu_minimum_milli: parse("admission_cpu_minimum_millicpu"),
        cpu_maximum_milli: parse("admission_cpu_maximum_millicpu"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn observation(total_mib: u64, available_mib: u64, cpus: u32) -> HostObservation {
        HostObservation {
            memory_total_bytes: total_mib * 1024 * 1024,
            memory_available_bytes: available_mib * 1024 * 1024,
            cgroup_memory_limit_bytes: None,
            cgroup_memory_current_bytes: None,
            cpu_count: cpus,
            observed_at_unix_ms: 1,
        }
    }

    struct FixedObserver(HostObservation);

    impl HostObserver for FixedObserver {
        fn observe(&self) -> Result<HostObservation> {
            Ok(self.0.clone())
        }
    }

    #[test]
    fn budget_is_derived_from_observed_facts_by_visible_policy() {
        // Through the observer seam, the way a real composition observes.
        let budget = CapacityBudget::observe(
            &FixedObserver(observation(3800, 700, 4)),
            BudgetPolicy::default(),
        )
        .unwrap();
        assert_eq!(budget.memory_ceiling_bytes, 3800 * 1024 * 1024);
        // 40% of ceiling.
        assert_eq!(budget.memory_task_bytes, 3800 * 1024 * 1024 * 4000 / 10_000);
        // One CPU held back for the resident field.
        assert_eq!(budget.cpu_task_count, 3);
    }

    #[test]
    fn a_finite_cgroup_limit_is_the_enforceable_ceiling() {
        let mut observed = observation(3800, 700, 4);
        observed.cgroup_memory_limit_bytes = Some(1024 * 1024 * 1024);
        let budget = CapacityBudget::from_observation(&observed, BudgetPolicy::default());
        assert_eq!(budget.memory_ceiling_bytes, 1024 * 1024 * 1024);
        assert_eq!(budget.memory_task_bytes, 1024 * 1024 * 1024 * 4000 / 10_000);
    }

    #[test]
    fn simultaneous_allocations_are_accounted_together_not_independently() {
        let budget =
            CapacityBudget::from_observation(&observation(1000, 100, 4), BudgetPolicy::default());
        // 40% of 1000MiB = 400MiB task budget.
        let dir = std::env::temp_dir().join(format!("wc-admission-{}", std::process::id()));
        let mut ledger = AdmissionLedger::load(&dir).unwrap();
        let first = TaskResources {
            memory_minimum_bytes: Some(250 * 1024 * 1024),
            ..TaskResources::default()
        };
        let second = TaskResources {
            memory_minimum_bytes: Some(250 * 1024 * 1024),
            ..TaskResources::default()
        };
        ledger.admit(&budget, "demand:first", &first).unwrap();
        // The second request is independently acceptable but not beside the first.
        let error = ledger.admit(&budget, "demand:second", &second).unwrap_err();
        assert!(
            error.to_string().contains("waiting for capacity"),
            "{error}"
        );
        assert!(error.to_string().contains("plus committed"), "{error}");
        // Release frees the budget.
        ledger.release_demand("demand:first").unwrap();
        ledger.admit(&budget, "demand:second", &second).unwrap();
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_floor_larger_than_the_whole_budget_names_the_resource() {
        let budget =
            CapacityBudget::from_observation(&observation(1000, 100, 4), BudgetPolicy::default());
        let dir = std::env::temp_dir().join(format!("wc-admission-big-{}", std::process::id()));
        let mut ledger = AdmissionLedger::load(&dir).unwrap();
        let huge = TaskResources {
            memory_minimum_bytes: Some(900 * 1024 * 1024),
            ..TaskResources::default()
        };
        let error = ledger.admit(&budget, "demand:huge", &huge).unwrap_err();
        assert!(
            error.to_string().contains("exceeds the whole task budget"),
            "{error}"
        );
        assert!(!ledger.declined.is_empty());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn reconcile_retires_only_verifiably_absent_worlds() {
        let budget =
            CapacityBudget::from_observation(&observation(1000, 100, 4), BudgetPolicy::default());
        let dir =
            std::env::temp_dir().join(format!("wc-admission-reconcile-{}", std::process::id()));
        let mut ledger = AdmissionLedger::load(&dir).unwrap();
        ledger
            .admit(
                &budget,
                "demand:a",
                &TaskResources {
                    memory_minimum_bytes: Some(1),
                    ..TaskResources::default()
                },
            )
            .unwrap();
        ledger.bind_world("demand:a", "world:a").unwrap();
        // In-flight reservation (no world yet) survives reconciliation.
        ledger
            .admit(
                &budget,
                "demand:b",
                &TaskResources {
                    memory_minimum_bytes: Some(1),
                    ..TaskResources::default()
                },
            )
            .unwrap();
        let released = ledger
            .reconcile(&std::collections::BTreeSet::from(
                ["world:other".to_owned()],
            ))
            .unwrap();
        assert_eq!(released, 1);
        // The in-flight reservation (admitted, no world yet) survives.
        assert_eq!(ledger.reservations.len(), 1);
        assert_eq!(ledger.reservations[0].demand_ref, "demand:b");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn minimum_and_ceiling_stay_distinct_and_ordered() {
        let resources = TaskResources::from_resources(&[
            epilogos_workcell_core::ResourceRequirement {
                key: "memory".into(),
                minimum: Some(256),
                maximum: Some(512),
                unit: Some("MiB".into()),
            },
            epilogos_workcell_core::ResourceRequirement {
                key: "cpu".into(),
                minimum: Some(500),
                maximum: Some(1500),
                unit: Some("m".into()),
            },
        ])
        .unwrap();
        assert_eq!(resources.memory_minimum_bytes, Some(256 * 1024 * 1024));
        assert_eq!(resources.memory_maximum_bytes, Some(512 * 1024 * 1024));
        assert_eq!(resources.cpu_minimum_milli, Some(500));
        assert_eq!(resources.cpu_maximum_milli, Some(1500));
        let invalid =
            TaskResources::from_resources(&[epilogos_workcell_core::ResourceRequirement {
                key: "memory".into(),
                minimum: Some(900),
                maximum: Some(512),
                unit: Some("MiB".into()),
            }]);
        assert!(invalid.is_err());
    }
}
