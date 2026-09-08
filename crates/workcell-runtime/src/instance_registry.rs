//! Live harness-instance registry for a collapsed-local workcell.
//!
//! A workcell is awake to the processes running inside it: every harness
//! instance (Hermes, Claude Code, Codex, zcode, …) that is executing in this
//! workcell is recorded here as a `workcell.harness-instance/v1` record. The
//! store is plain JSON under the workcell state root, beside the existing
//! `workspaces`/`artifacts` layout (`local.rs`), and it is owned by the
//! Workcell layer (host+services).
//!
//! Identity law: `instance_ref` is `instance:<slug>:<sha256>` where the hash
//! covers the executable receipt and first-seen identity material — never the
//! pid, which is a volatile observation re-written on each scan (M2).
//!
//! Temperament (doctor's three honest states): a failed operation is named
//! unavailability; `stale` is a disclosed state, never a silent delete; a
//! conflicting identity for a known slug is a named finding, never
//! auto-resolved.

use std::{
    collections::BTreeMap,
    fs,
    path::{Path, PathBuf},
};

use epilogos_workcell_core::{Result, WorkcellError, WorkcellRef};
use serde_json::{json, Map, Value};
use sha2::{Digest, Sha256};

pub const HARNESS_INSTANCE_SCHEMA: &str = "workcell.harness-instance/v1";
pub const REGISTRY_SCHEMA: &str = "workcell.registry/v1";
pub const REGISTRY_DIR: &str = "instances";
pub const REGISTRY_FILE: &str = "registry.json";

pub const EVIDENCE_LIVE_PID: &str = "live-pid";
pub const EVIDENCE_GATEWAY_CONFIRMED: &str = "gateway-confirmed";
pub const EVIDENCE_DECLARED_UNVERIFIED: &str = "declared-unverified";
pub const EVIDENCE_GRADES: [&str; 3] = [
    EVIDENCE_LIVE_PID,
    EVIDENCE_GATEWAY_CONFIRMED,
    EVIDENCE_DECLARED_UNVERIFIED,
];

pub const LIVENESS_LIVE: &str = "live";
pub const LIVENESS_STALE: &str = "stale";
pub const LIVENESS_STATES: [&str; 2] = [LIVENESS_LIVE, LIVENESS_STALE];

/// Outcome of a registration attempt, mirroring the byte-identical
/// `outcome: unchanged` law used by Central adoptions.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RegisterOutcome {
    Registered,
    Unchanged,
    /// Same slug already holds a record whose executable identity differs.
    /// Named and returned; never auto-resolved.
    Conflict {
        existing: Value,
        incoming: Value,
    },
}

/// Registry file (`$state_root/instances/registry.json`).
#[derive(Debug, Clone)]
pub struct InstanceRegistry {
    state_root: PathBuf,
    workcell_ref: WorkcellRef,
}

impl InstanceRegistry {
    pub fn new(state_root: impl Into<PathBuf>, workcell_ref: WorkcellRef) -> Self {
        Self {
            state_root: state_root.into(),
            workcell_ref,
        }
    }

    fn registry_path(&self) -> PathBuf {
        self.state_root.join(REGISTRY_DIR).join(REGISTRY_FILE)
    }

    fn empty_file(&self) -> Value {
        json!({
            "schema": REGISTRY_SCHEMA,
            "workcell_ref": self.workcell_ref.to_string(),
            "instances": {},
        })
    }

    /// Load the registry file. A missing file is an empty registry (the
    /// workcell has observed no instances yet), not an error; an unreadable
    /// or invalid file is named unavailability.
    pub fn load(&self) -> Result<Value> {
        let path = self.registry_path();
        if !path.exists() {
            return Ok(self.empty_file());
        }
        let raw = fs::read_to_string(&path).map_err(|error| {
            WorkcellError::OperationFailed(format!(
                "read instance registry {}: {error}",
                path.display()
            ))
        })?;
        let parsed: Value = serde_json::from_str(&raw).map_err(|error| {
            WorkcellError::OperationFailed(format!(
                "parse instance registry {}: {error}",
                path.display()
            ))
        })?;
        validate_registry_file(&parsed)?;
        Ok(parsed)
    }

    pub fn list(&self) -> Result<Vec<Value>> {
        let file = self.load()?;
        let instances = file
            .get("instances")
            .and_then(Value::as_object)
            .expect("validated registry file has an instances object");
        Ok(instances.values().cloned().collect())
    }

    pub fn show(&self, instance_ref: &str) -> Result<Value> {
        let file = self.load()?;
        let instances = file
            .get("instances")
            .and_then(Value::as_object)
            .expect("validated registry file has an instances object");
        instances.get(instance_ref).cloned().ok_or_else(|| {
            WorkcellError::InvalidDemand(format!("unknown instance `{instance_ref}`"))
        })
    }

    /// Register one validated `workcell.harness-instance/v1` record.
    ///
    /// Same `instance_ref` with byte-identical content → `Unchanged`. Same
    /// slug with a differing executable identity → `Conflict` (named, not
    /// written). Anything else → written and `Registered`.
    pub fn register(&self, record: Value) -> Result<RegisterOutcome> {
        validate_instance_record(&record)?;
        let record_ref = record_ref(&record)?;
        let slug = harness_slug(&record)?;

        let mut file = self.load()?;
        let instances = file
            .get_mut("instances")
            .and_then(Value::as_object_mut)
            .expect("validated registry file has an instances object");

        if let Some(existing) = instances.get(&record_ref) {
            if existing == &record {
                return Ok(RegisterOutcome::Unchanged);
            }
            // Same identity hash but different bytes: refresh the observation
            // fields (pids, observed_at, seams) in place; identity is stable.
            let mut updated = existing.clone();
            for key in ["pids", "observed_at", "seams", "liveness"] {
                if let Some(value) = record.get(key) {
                    updated[key] = value.clone();
                }
            }
            instances.insert(record_ref.clone(), updated.clone());
            self.store(&file)?;
            return Ok(RegisterOutcome::Registered);
        }

        for (existing_ref, existing) in instances.iter() {
            if existing_ref.starts_with(&format!("instance:{slug}:")) {
                let same_identity = existing
                    .pointer("/executable/sha256")
                    .and_then(Value::as_str)
                    == record.pointer("/executable/sha256").and_then(Value::as_str);
                if !same_identity {
                    return Ok(RegisterOutcome::Conflict {
                        existing: existing.clone(),
                        incoming: record,
                    });
                }
            }
        }

        instances.insert(record_ref, record);
        self.store(&file)?;
        Ok(RegisterOutcome::Registered)
    }

    /// Declare a prespecified harness instance before it is observed:
    /// `evidence_grade: declared-unverified`, no pids, born in the
    /// disclosed not-observed state.
    ///
    /// Adopt-after-detect (plan §3): a later scan observing the same slug
    /// binds the declaration to what exists via [`InstanceRegistry::adopt`].
    /// Until then the declaration waits — it is never aged by missed scans
    /// and never silently deleted.
    ///
    /// Conflict law, mirrored from `machine.adopt-current`: a declaration
    /// whose concrete identity (non-empty executable sha256, or the
    /// identity material when no executable is given) differs from an
    /// existing record for the same slug is a named `Conflict`, never
    /// auto-resolved. Declaring a slug that already holds a live record is
    /// an `Unchanged` no-op — live evidence dominates a declaration.
    pub fn declare(
        &self,
        slug: &str,
        executable: Option<&Path>,
        identity_material: &str,
    ) -> Result<RegisterOutcome> {
        let (path, sha256) = match executable {
            Some(path) => (path.to_path_buf(), sha256_file(path)?),
            None => (PathBuf::new(), String::new()),
        };
        let record = build_instance_record(
            &self.workcell_ref,
            &InstanceObservation {
                slug: slug.to_owned(),
                executable: path,
                executable_sha256: sha256.clone(),
                identity_material: identity_material.to_owned(),
                pids: Vec::new(),
                evidence_grade: EVIDENCE_DECLARED_UNVERIFIED.to_owned(),
                seams: Vec::new(),
            },
        );
        validate_instance_record(&record)?;
        let declared_ref = record_ref(&record)?;

        let mut file = self.load()?;
        let instances = file
            .get_mut("instances")
            .and_then(Value::as_object_mut)
            .expect("validated registry file has an instances object");

        if let Some(existing) = instances.get(&declared_ref) {
            if existing == &record {
                return Ok(RegisterOutcome::Unchanged);
            }
            // Same identity, refreshed declaration fields in place.
            let mut updated = existing.clone();
            for key in ["observed_at"] {
                if let Some(value) = record.get(key) {
                    updated[key] = value.clone();
                }
            }
            instances.insert(declared_ref.clone(), updated);
            self.store(&file)?;
            return Ok(RegisterOutcome::Registered);
        }

        for (existing_ref, existing) in instances.iter() {
            if !existing_ref.starts_with(&format!("instance:{slug}:")) {
                continue;
            }
            let existing_sha = existing
                .pointer("/executable/sha256")
                .and_then(Value::as_str)
                .unwrap_or_default();
            let existing_material = existing
                .get("identity_material")
                .and_then(Value::as_str)
                .unwrap_or_default();
            let existing_declared = existing
                .get("evidence_grade")
                .and_then(Value::as_str)
                .unwrap_or_default()
                == EVIDENCE_DECLARED_UNVERIFIED;
            let identity_differs = if !sha256.is_empty() && !existing_sha.is_empty() {
                sha256 != existing_sha
            } else if existing_declared {
                identity_material != existing_material
            } else {
                false
            };
            if identity_differs {
                return Ok(RegisterOutcome::Conflict {
                    existing: existing.clone(),
                    incoming: record,
                });
            }
            // Same slug, no concrete identity conflict: an existing live
            // record already knows more than a declaration; another
            // declared record with matching identity was handled above by
            // instance_ref. Either way nothing to write.
            return Ok(RegisterOutcome::Unchanged);
        }

        instances.insert(declared_ref, record);
        self.store(&file)?;
        Ok(RegisterOutcome::Registered)
    }

    /// Adopt-after-detect: replace a `declared-unverified` record with the
    /// observed live identity, carrying lineage. The caller has already
    /// established that the declared identity does not concretely conflict
    /// with the observation; any other same-slug surprise surfaces as a
    /// named `Conflict`, never auto-resolved.
    pub fn adopt(&self, observed: Value, declared_ref: &str) -> Result<RegisterOutcome> {
        validate_instance_record(&observed)?;
        let observed_ref = record_ref(&observed)?;
        let mut file = self.load()?;
        let instances = file
            .get_mut("instances")
            .and_then(Value::as_object_mut)
            .expect("validated registry file has an instances object");
        let Some(declared) = instances.get(declared_ref) else {
            return Err(WorkcellError::InvalidDemand(format!(
                "cannot adopt: no declared record `{declared_ref}`"
            )));
        };
        if observed_ref == declared_ref {
            // Declaration already matched the observed identity.
            if declared == &observed {
                return Ok(RegisterOutcome::Unchanged);
            }
            let mut updated = declared.clone();
            for key in ["pids", "observed_at", "seams", "liveness", "evidence_grade"] {
                if let Some(value) = observed.get(key) {
                    updated[key] = value.clone();
                }
            }
            instances.insert(observed_ref.clone(), updated);
            self.store(&file)?;
            return Ok(RegisterOutcome::Registered);
        }
        for (existing_ref, existing) in instances.iter() {
            if existing_ref.starts_with(&format!("instance:{}:", harness_slug(&observed)?))
                && existing_ref != declared_ref
                && existing.pointer("/executable/sha256") != observed.pointer("/executable/sha256")
            {
                return Ok(RegisterOutcome::Conflict {
                    existing: existing.clone(),
                    incoming: observed,
                });
            }
        }
        let mut adopted = observed;
        adopted["lineage"] = json!({
            "adopted_from": declared_ref,
            "declared_identity_material": declared
                .get("identity_material")
                .cloned()
                .unwrap_or(Value::Null),
        });
        instances.remove(declared_ref);
        instances.insert(observed_ref, adopted);
        self.store(&file)?;
        Ok(RegisterOutcome::Registered)
    }

    /// Apply liveness bookkeeping from a completed scan: the named records
    /// get their miss count and liveness state written; every other record
    /// is untouched. Records are never deleted here — `stale` is disclosed,
    /// not removed.
    pub fn apply_liveness(&self, updates: &[(String, u64, &str)]) -> Result<()> {
        let mut file = self.load()?;
        let instances = file
            .get_mut("instances")
            .and_then(Value::as_object_mut)
            .expect("validated registry file has an instances object");
        for (reference, misses, liveness) in updates {
            if let Some(record) = instances.get_mut(reference) {
                record["consecutive_misses"] = (*misses).into();
                record["liveness"] = (*liveness).into();
            }
        }
        self.store(&file)
    }

    fn store(&self, file: &Value) -> Result<()> {
        let path = self.registry_path();
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).map_err(|error| {
                WorkcellError::OperationFailed(format!(
                    "create instance registry directory {}: {error}",
                    parent.display()
                ))
            })?;
        }
        let bytes =
            serde_json::to_string_pretty(file).expect("registry file is a validated JSON value");
        atomic_write(&path, bytes.as_bytes())
    }
}

/// Stable identity hash: sha256 over the executable receipt and the
/// first-seen identity material. Pids are never identity.
pub fn identity_hash(executable_sha256: &str, identity_material: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(executable_sha256.as_bytes());
    hasher.update(b"\0");
    hasher.update(identity_material.as_bytes());
    hex(&hasher.finalize())
}

/// The observed material from which an instance record is built.
///
/// `pids` holds every live process observation for this identity this scan:
/// multi-execution is the default paradigm — one contract identity, many
/// simultaneous executions, each named.
pub struct InstanceObservation {
    pub slug: String,
    pub executable: PathBuf,
    pub executable_sha256: String,
    /// Stable first-seen identity material (e.g. the resolved executable
    /// path at first observation). Never a pid.
    pub identity_material: String,
    pub pids: Vec<u32>,
    pub evidence_grade: String,
    pub seams: Vec<Value>,
}

/// Build a `workcell.harness-instance/v1` record from an observation.
pub fn build_instance_record(
    workcell_ref: &WorkcellRef,
    observation: &InstanceObservation,
) -> Value {
    let stable_hash = identity_hash(
        &observation.executable_sha256,
        &observation.identity_material,
    );
    // A declared-unverified instance has no live observation; `stale` is the
    // disclosed not-observed state, never a silent absence.
    let liveness = if observation.evidence_grade == EVIDENCE_DECLARED_UNVERIFIED {
        LIVENESS_STALE
    } else {
        LIVENESS_LIVE
    };
    json!({
        "schema": HARNESS_INSTANCE_SCHEMA,
        "instance_ref": format!("instance:{}:{stable_hash}", observation.slug),
        "harness_ref": format!("harness/{}", observation.slug),
        "workcell_ref": workcell_ref.to_string(),
        "pids": observation.pids,
        "executable": {
            "path": observation.executable.display().to_string(),
            "sha256": observation.executable_sha256,
        },
        "seams": observation.seams,
        "evidence_grade": observation.evidence_grade,
        "liveness": liveness,
        "consecutive_misses": 0,
        "observed_at": observed_now(),
    })
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

fn sha256_file(path: &Path) -> Result<String> {
    let bytes = fs::read(path).map_err(|error| {
        WorkcellError::InvalidDemand(format!(
            "cannot hash declared executable {}: {error}",
            path.display()
        ))
    })?;
    let mut hasher = Sha256::new();
    hasher.update(&bytes);
    Ok(hex(&hasher.finalize()))
}

fn observed_now() -> String {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| format!("unix:{}", duration.as_secs()))
        .unwrap_or_else(|_| "unknown".to_string())
}

fn record_ref(record: &Value) -> Result<String> {
    required_str(record, "instance_ref")
}

fn harness_slug(record: &Value) -> Result<String> {
    let harness_ref = required_str(record, "harness_ref")?;
    harness_ref
        .strip_prefix("harness/")
        .map(str::to_owned)
        .ok_or_else(|| {
            WorkcellError::InvalidDemand(format!(
                "harness_ref `{harness_ref}` must read `harness/<slug>`"
            ))
        })
}

fn required_str(record: &Value, field: &str) -> Result<String> {
    record
        .get(field)
        .and_then(Value::as_str)
        .map(str::to_owned)
        .ok_or_else(|| {
            WorkcellError::InvalidDemand(format!("instance record requires a string `{field}`"))
        })
}

fn validate_registry_file(file: &Value) -> Result<()> {
    let object = file.as_object().ok_or_else(|| {
        WorkcellError::OperationFailed("instance registry file must be a JSON object".into())
    })?;
    if object.get("schema").and_then(Value::as_str) != Some(REGISTRY_SCHEMA) {
        return Err(WorkcellError::OperationFailed(format!(
            "instance registry file must declare schema `{REGISTRY_SCHEMA}`"
        )));
    }
    if object.get("workcell_ref").and_then(Value::as_str).is_none() {
        return Err(WorkcellError::OperationFailed(
            "instance registry file requires a string `workcell_ref`".into(),
        ));
    }
    let instances = object
        .get("instances")
        .and_then(Value::as_object)
        .ok_or_else(|| {
            WorkcellError::OperationFailed(
                "instance registry file requires an `instances` object".into(),
            )
        })?;
    for (key, record) in instances {
        validate_instance_record(record).map_err(|error| {
            WorkcellError::OperationFailed(format!("registry entry `{key}`: {error}"))
        })?;
        if record_ref(record).ok().as_deref() != Some(key.as_str()) {
            return Err(WorkcellError::OperationFailed(format!(
                "registry key `{key}` does not match its record's instance_ref"
            )));
        }
    }
    Ok(())
}

/// Record validation mirroring the Actuation `harness-detection/v1`
/// validator temperament: every missing or malformed field is named.
pub fn validate_instance_record(record: &Value) -> Result<()> {
    let object = record.as_object().ok_or_else(|| {
        WorkcellError::InvalidDemand("instance record must be a JSON object".into())
    })?;
    if object.get("schema").and_then(Value::as_str) != Some(HARNESS_INSTANCE_SCHEMA) {
        return Err(WorkcellError::InvalidDemand(format!(
            "instance record must declare schema `{HARNESS_INSTANCE_SCHEMA}`"
        )));
    }
    let reference = required_str(record, "instance_ref")?;
    if !reference.starts_with("instance:") {
        return Err(WorkcellError::InvalidDemand(format!(
            "instance_ref `{reference}` must read `instance:<slug>:<hash>`"
        )));
    }
    let harness_ref = required_str(record, "harness_ref")?;
    if !harness_ref.starts_with("harness/") || harness_ref.len() <= "harness/".len() {
        return Err(WorkcellError::InvalidDemand(format!(
            "harness_ref `{harness_ref}` must read `harness/<slug>`"
        )));
    }
    required_str(record, "workcell_ref")?;

    let executable = record
        .get("executable")
        .and_then(Value::as_object)
        .ok_or_else(|| {
            WorkcellError::InvalidDemand("instance record requires an `executable` object".into())
        })?;
    if executable.get("path").and_then(Value::as_str).is_none() {
        return Err(WorkcellError::InvalidDemand(
            "instance record requires `executable.path`".into(),
        ));
    }
    if executable.get("sha256").and_then(Value::as_str).is_none() {
        return Err(WorkcellError::InvalidDemand(
            "instance record requires `executable.sha256`".into(),
        ));
    }

    let grade = required_str(record, "evidence_grade")?;
    if !EVIDENCE_GRADES.contains(&grade.as_str()) {
        return Err(WorkcellError::InvalidDemand(format!(
            "evidence_grade `{grade}` must be one of {EVIDENCE_GRADES:?}"
        )));
    }
    if grade == EVIDENCE_DECLARED_UNVERIFIED {
        let pids = record
            .get("pids")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        if !pids.is_empty() {
            return Err(WorkcellError::InvalidDemand(
                "a `declared-unverified` instance must not claim pids".into(),
            ));
        }
    } else if record
        .get("pids")
        .and_then(Value::as_array)
        .map(|pids| pids.is_empty())
        .unwrap_or(true)
        && grade != EVIDENCE_GATEWAY_CONFIRMED
    {
        return Err(WorkcellError::InvalidDemand(
            "a detected instance requires at least one observed pid".into(),
        ));
    }

    let liveness = required_str(record, "liveness")?;
    if !LIVENESS_STATES.contains(&liveness.as_str()) {
        return Err(WorkcellError::InvalidDemand(format!(
            "liveness `{liveness}` must be one of {LIVENESS_STATES:?}"
        )));
    }
    if liveness == LIVENESS_LIVE
        && record
            .get("pids")
            .and_then(Value::as_array)
            .map(|pids| pids.is_empty())
            .unwrap_or(true)
        && grade != EVIDENCE_GATEWAY_CONFIRMED
    {
        return Err(WorkcellError::InvalidDemand(
            "a `live` instance requires at least one observed pid (or a carrier-confirmed grade)"
                .into(),
        ));
    }

    let seams = record
        .get("seams")
        .and_then(Value::as_array)
        .ok_or_else(|| {
            WorkcellError::InvalidDemand("instance record requires a `seams` array".into())
        })?;
    for seam in seams {
        let seam_object = seam
            .as_object()
            .ok_or_else(|| WorkcellError::InvalidDemand("each seam must be an object".into()))?;
        for field in ["kind", "path", "exists"] {
            if seam_object.get(field).is_none() {
                return Err(WorkcellError::InvalidDemand(format!(
                    "each seam requires `{field}`"
                )));
            }
        }
    }

    required_str(record, "observed_at")?;
    Ok(())
}

/// Convenience re-export for callers composing seam facets in the Actuation
/// facet vocabulary (`skills|plugins|hooks|commands|models|…`).
pub fn seam(kind: &str, path: impl Into<PathBuf>, exists: bool, count: Option<usize>) -> Value {
    let mut seam = Map::new();
    seam.insert("kind".into(), kind.into());
    seam.insert("path".into(), path.into().display().to_string().into());
    seam.insert("exists".into(), exists.into());
    if let Some(count) = count {
        seam.insert("count".into(), count.into());
    }
    Value::Object(seam)
}

fn atomic_write(path: &Path, bytes: &[u8]) -> Result<()> {
    let temp = path.with_extension("json.tmp");
    fs::write(&temp, bytes).map_err(|error| {
        WorkcellError::OperationFailed(format!(
            "write instance registry {}: {error}",
            temp.display()
        ))
    })?;
    fs::rename(&temp, path).map_err(|error| {
        WorkcellError::OperationFailed(format!(
            "commit instance registry {}: {error}",
            path.display()
        ))
    })
}

/// Sort helper for stable listings.
pub fn sort_by_reference(mut records: Vec<Value>) -> Vec<Value> {
    records.sort_by_key(|record| record_ref(record).unwrap_or_default());
    records
}

/// Group a listing by harness slug (projection-candidate read shape).
pub fn by_slug(records: &[Value]) -> BTreeMap<String, Vec<Value>> {
    let mut grouped: BTreeMap<String, Vec<Value>> = BTreeMap::new();
    for record in records {
        if let Ok(slug) = harness_slug(record) {
            grouped.entry(slug).or_default().push(record.clone());
        }
    }
    grouped
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_root(name: &str) -> PathBuf {
        let root = std::env::temp_dir().join(format!(
            "workcell-registry-test-{name}-{}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).unwrap();
        root
    }

    fn sample_record(root: &Path, slug: &str, sha: &str, pid: Option<u32>) -> Value {
        let executable = root.join("bin").join(slug);
        build_instance_record(
            &WorkcellRef::new("workcell:local").unwrap(),
            &InstanceObservation {
                slug: slug.to_owned(),
                executable: executable.clone(),
                executable_sha256: sha.to_owned(),
                identity_material: executable.display().to_string(),
                pids: pid.into_iter().collect(),
                evidence_grade: if pid.is_some() {
                    EVIDENCE_LIVE_PID.into()
                } else {
                    EVIDENCE_DECLARED_UNVERIFIED.into()
                },
                seams: vec![seam("skills", executable.join("skills"), false, None)],
            },
        )
    }

    #[test]
    fn missing_registry_reads_as_empty() {
        let root = temp_root("empty");
        let registry = InstanceRegistry::new(&root, WorkcellRef::new("workcell:local").unwrap());
        assert_eq!(registry.list().unwrap(), Vec::<Value>::new());
    }

    #[test]
    fn register_list_show_roundtrip() {
        let root = temp_root("roundtrip");
        let registry = InstanceRegistry::new(&root, WorkcellRef::new("workcell:local").unwrap());
        let record = sample_record(&root, "hermes", "aa".repeat(32).as_str(), Some(42));
        let reference = record["instance_ref"].as_str().unwrap().to_owned();

        assert_eq!(
            registry.register(record.clone()).unwrap(),
            RegisterOutcome::Registered
        );
        assert_eq!(registry.list().unwrap().len(), 1);
        let shown = registry.show(&reference).unwrap();
        assert_eq!(shown["harness_ref"].as_str().unwrap(), "harness/hermes");
        assert_eq!(shown["evidence_grade"].as_str().unwrap(), EVIDENCE_LIVE_PID);
    }

    #[test]
    fn byte_identical_reregister_is_unchanged() {
        let root = temp_root("unchanged");
        let registry = InstanceRegistry::new(&root, WorkcellRef::new("workcell:local").unwrap());
        let record = sample_record(&root, "codex", "bb".repeat(32).as_str(), Some(7));
        registry.register(record.clone()).unwrap();
        assert_eq!(
            registry.register(record).unwrap(),
            RegisterOutcome::Unchanged
        );
        assert_eq!(registry.list().unwrap().len(), 1);
    }

    #[test]
    fn same_slug_different_executable_is_named_conflict() {
        let root = temp_root("conflict");
        let registry = InstanceRegistry::new(&root, WorkcellRef::new("workcell:local").unwrap());
        registry
            .register(sample_record(
                &root,
                "claude",
                "cc".repeat(32).as_str(),
                Some(1),
            ))
            .unwrap();
        let outcome = registry.register(sample_record(
            &root,
            "claude",
            "dd".repeat(32).as_str(),
            Some(2),
        ));
        match outcome.unwrap() {
            RegisterOutcome::Conflict { existing, incoming } => {
                assert_eq!(existing["executable"]["sha256"], "cc".repeat(32));
                assert_eq!(incoming["executable"]["sha256"], "dd".repeat(32));
            }
            other => panic!("expected conflict, got {other:?}"),
        }
        // The conflicting record is never auto-resolved into the store.
        assert_eq!(registry.list().unwrap().len(), 1);
    }

    #[test]
    fn refreshed_observation_keeps_identity() {
        let root = temp_root("refresh");
        let registry = InstanceRegistry::new(&root, WorkcellRef::new("workcell:local").unwrap());
        let first = sample_record(&root, "zcode", "ee".repeat(32).as_str(), Some(10));
        let reference = first["instance_ref"].as_str().unwrap().to_owned();
        registry.register(first).unwrap();

        let mut second = sample_record(&root, "zcode", "ee".repeat(32).as_str(), Some(11));
        second["observed_at"] = "unix:2".into();
        assert_eq!(
            registry.register(second).unwrap(),
            RegisterOutcome::Registered
        );
        let shown = registry.show(&reference).unwrap();
        assert_eq!(shown["pids"], json!([11]));
        assert_eq!(shown["instance_ref"].as_str().unwrap(), reference);
    }

    #[test]
    fn declared_unverified_carries_no_pid_and_validates() {
        let root = temp_root("declared");
        let registry = InstanceRegistry::new(&root, WorkcellRef::new("workcell:local").unwrap());
        let record = sample_record(&root, "gemini", "ff".repeat(32).as_str(), None);
        assert_eq!(record["pids"], json!([]));
        assert_eq!(record["evidence_grade"], EVIDENCE_DECLARED_UNVERIFIED);
        registry.register(record).unwrap();
    }

    #[test]
    fn invalid_records_are_rejected_by_name() {
        let mut record = sample_record(
            &std::env::temp_dir(),
            "pi",
            "ab".repeat(32).as_str(),
            Some(3),
        );
        record["evidence_grade"] = "guessed".into();
        let error = validate_instance_record(&record).unwrap_err().to_string();
        assert!(error.contains("evidence_grade"), "{error}");

        let mut live_without_pid = sample_record(
            &std::env::temp_dir(),
            "pi",
            "ab".repeat(32).as_str(),
            Some(3),
        );
        live_without_pid["pids"] = json!([]);
        let error = validate_instance_record(&live_without_pid)
            .unwrap_err()
            .to_string();
        assert!(error.contains("pid"), "{error}");
    }

    #[test]
    fn corrupted_registry_is_named_unavailability() {
        let root = temp_root("corrupt");
        let dir = root.join(REGISTRY_DIR);
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join(REGISTRY_FILE), "{ not json").unwrap();
        let registry = InstanceRegistry::new(&root, WorkcellRef::new("workcell:local").unwrap());
        let error = registry.list().unwrap_err().to_string();
        assert!(error.contains("parse instance registry"), "{error}");
    }

    #[test]
    fn identity_hash_is_stable_and_pid_independent() {
        let first = identity_hash("aa", "/usr/local/bin/hermes");
        let second = identity_hash("aa", "/usr/local/bin/hermes");
        let third = identity_hash("ab", "/usr/local/bin/hermes");
        assert_eq!(first, second);
        assert_ne!(first, third);
    }
}
