//! Bounded, on-demand resource observations for registered harness processes.
//!
//! This is intentionally not a metrics daemon. One call samples one PID over
//! one bounded interval and returns only facts the local OS `ps` provider can
//! establish without reading process arguments or environment variables.

use std::{
    fs,
    path::{Path, PathBuf},
    process::Command,
    thread,
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use epilogos_workcell_core::{Result, WorkcellError, WorkcellRef};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};

use crate::instance_registry::{validate_instance_record, InstanceRegistry, LIVENESS_LIVE};

pub const RESOURCE_USAGE_SCHEMA: &str = "workcell.resource-usage/v1";
pub const DEFAULT_INTERVAL: Duration = Duration::from_millis(100);
pub const MAX_INTERVAL: Duration = Duration::from_secs(60);

#[derive(Debug, Clone, PartialEq, Eq)]
struct ProcessSample {
    pid: u32,
    start_marker: String,
    executable: PathBuf,
    cpu_time_millis: Option<u64>,
    rss_bytes: Option<u64>,
}

/// One completed bounded observation. The JSON projection is kept here so
/// native callers and the CLI share exactly one contract.
#[derive(Debug, Clone)]
pub struct ResourceUsageReport {
    value: Value,
}

impl ResourceUsageReport {
    pub fn as_json(&self) -> Value {
        self.value.clone()
    }
}

/// Observe one PID belonging to one currently-live registered instance.
///
/// The registry binding is checked before and after sampling, while the OS
/// process start marker and executable are checked across both samples. A PID
/// replacement or concurrent registry rebind is rejected rather than merged.
pub fn observe_resource_usage(
    registry: &InstanceRegistry,
    instance_ref: &str,
    requested_pid: Option<u32>,
    interval: Duration,
    external_correlation_refs: Vec<String>,
) -> Result<ResourceUsageReport> {
    if interval > MAX_INTERVAL {
        return Err(WorkcellError::InvalidDemand(format!(
            "resource observation interval must not exceed {} milliseconds",
            MAX_INTERVAL.as_millis()
        )));
    }
    if external_correlation_refs
        .iter()
        .any(|reference| reference.is_empty())
    {
        return Err(WorkcellError::InvalidDemand(
            "external correlation refs must not be empty".into(),
        ));
    }

    let before_record = registry.show(instance_ref)?;
    validate_instance_record(&before_record)?;
    let pid = select_pid(&before_record, requested_pid)?;
    let binding = binding_snapshot(&before_record, pid)?;

    let started_at = unix_millis()?;
    let first = sample_process(pid)?;
    validate_process_binding(&before_record, &first)?;
    thread::sleep(interval);
    let second = sample_process(pid)?;
    let ended_at = unix_millis()?;
    validate_process_binding(&before_record, &second)?;
    validate_same_process(&first, &second)?;

    let after_record = registry.show(instance_ref)?;
    let after_binding = binding_snapshot(&after_record, pid)?;
    if binding != after_binding {
        return Err(WorkcellError::Unavailable(format!(
            "instance/PID binding drifted while observing `{instance_ref}` pid {pid}"
        )));
    }

    let duration_ms = ended_at.saturating_sub(started_at);
    let cpu_delta = match (first.cpu_time_millis, second.cpu_time_millis) {
        (Some(first), Some(second)) => second.checked_sub(first),
        _ => None,
    };
    let cpu_utilisation = cpu_delta
        .and_then(|delta| (duration_ms > 0).then_some((delta as f64 / duration_ms as f64) * 100.0));
    let usage_ref = usage_ref(instance_ref, pid, &first.start_marker, started_at, ended_at);
    let executable = before_record
        .get("executable")
        .cloned()
        .unwrap_or(Value::Null);
    let seams = before_record
        .get("seams")
        .cloned()
        .unwrap_or_else(|| json!([]));

    let value = json!({
        "schema": RESOURCE_USAGE_SCHEMA,
        "ok": true,
        "status": "ok",
        "usage_ref": usage_ref,
        "workcell_ref": before_record["workcell_ref"],
        "harness_instance_ref": before_record["instance_ref"],
        "harness_ref": before_record["harness_ref"],
        "material_binding": {
            "kind": "process",
            "pid": pid,
            "process_start_marker": first.start_marker,
            "executable": executable,
            "instance_evidence_grade": before_record["evidence_grade"],
            "instance_seams": seams,
        },
        "interval": {
            "started_at": format!("unix-ms:{started_at}"),
            "ended_at": format!("unix-ms:{ended_at}"),
            "duration_ms": duration_ms,
            "requested_duration_ms": interval.as_millis() as u64,
        },
        "provider": {
            "provider_ref": local_provider_ref(),
            "source": local_provider_source(),
            "platform": std::env::consts::OS,
            "collection": "bounded-on-demand",
            "raw_native_refs": raw_native_refs(pid),
            "privacy": {
                "argv_collected": false,
                "environment_collected": false,
            },
        },
        "metrics": {
            "cpu_time": metric_observed(second.cpu_time_millis, "milliseconds", "ps time"),
            "cpu_utilisation": metric_derived(cpu_utilisation, "percent", "cpu_time delta / wall interval"),
            "memory_rss": metric_observed(second.rss_bytes, "bytes", "ps rss"),
            "memory_peak_rss": metric_unsupported("the local ps projection does not establish peak RSS"),
            "gpu_utilisation": metric_unsupported("the local ps provider does not expose GPU utilisation"),
            "vram": metric_unsupported("the local ps provider does not expose VRAM allocation"),
            "network_bytes": metric_unsupported("the local ps provider does not expose per-process network bytes"),
            "storage_io_bytes": metric_unsupported("the portable local ps projection does not establish storage I/O bytes"),
        },
        "external_correlation_refs": external_correlation_refs,
    });
    validate_resource_usage(&value)?;
    Ok(ResourceUsageReport { value })
}

fn local_provider_ref() -> &'static str {
    if cfg!(target_os = "linux") {
        "provider:local-os:ps-procfs"
    } else {
        "provider:local-os:ps"
    }
}

fn local_provider_source() -> &'static str {
    if cfg!(target_os = "linux") {
        "ps+procfs-exe"
    } else {
        "ps"
    }
}

fn raw_native_refs(pid: u32) -> Vec<String> {
    let mut refs = vec![format!("local-os:ps:pid:{pid}:lstart,time,rss,comm")];
    if cfg!(target_os = "linux") {
        refs.push(format!("local-os:procfs:pid:{pid}:exe"));
    }
    refs
}

/// Validate the portable v1 reading. This guards persisted/provider fixtures
/// as well as values produced by this runtime; missing metrics cannot become
/// numeric zero through a permissive projection.
pub fn validate_resource_usage(reading: &Value) -> Result<()> {
    let object = reading.as_object().ok_or_else(|| {
        WorkcellError::InvalidDemand("resource usage reading must be an object".into())
    })?;
    if object.get("schema").and_then(Value::as_str) != Some(RESOURCE_USAGE_SCHEMA) {
        return Err(WorkcellError::InvalidDemand(format!(
            "resource usage reading must declare schema `{RESOURCE_USAGE_SCHEMA}`"
        )));
    }
    exact_keys(
        object,
        &[
            "schema",
            "ok",
            "status",
            "usage_ref",
            "workcell_ref",
            "harness_instance_ref",
            "harness_ref",
            "material_binding",
            "interval",
            "provider",
            "metrics",
            "external_correlation_refs",
        ],
        "resource usage reading",
    )?;
    if object.get("ok").and_then(Value::as_bool) != Some(true) {
        return Err(WorkcellError::InvalidDemand(
            "a completed resource usage reading must declare `ok: true`".into(),
        ));
    }
    if object.get("status").and_then(Value::as_str) != Some("ok") {
        return Err(WorkcellError::InvalidDemand(
            "a completed resource usage reading must have status `ok`".into(),
        ));
    }
    let usage_ref = required_reading_str(reading, "usage_ref")?;
    if usage_ref.strip_prefix("usage:").is_none_or(|hash| {
        hash.len() != 64
            || !hash
                .bytes()
                .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    }) {
        return Err(WorkcellError::InvalidDemand(
            "usage_ref must read `usage:<64 lowercase hex characters>`".into(),
        ));
    }
    WorkcellRef::new(&required_reading_str(reading, "workcell_ref")?)?;
    let instance_ref = required_reading_str(reading, "harness_instance_ref")?;
    if !instance_ref.starts_with("instance:") {
        return Err(WorkcellError::InvalidDemand(
            "harness_instance_ref must read `instance:<slug>:<hash>`".into(),
        ));
    }
    let harness_ref = required_reading_str(reading, "harness_ref")?;
    if !harness_ref.starts_with("harness/") || harness_ref.len() == "harness/".len() {
        return Err(WorkcellError::InvalidDemand(
            "harness_ref must read `harness/<slug>`".into(),
        ));
    }

    let binding = object
        .get("material_binding")
        .and_then(Value::as_object)
        .ok_or_else(|| WorkcellError::InvalidDemand("material_binding must be an object".into()))?;
    exact_keys(
        binding,
        &[
            "kind",
            "pid",
            "process_start_marker",
            "executable",
            "instance_evidence_grade",
            "instance_seams",
        ],
        "material_binding",
    )?;
    if binding.get("kind").and_then(Value::as_str) != Some("process")
        || binding
            .get("pid")
            .and_then(Value::as_u64)
            .is_none_or(|pid| pid == 0)
        || binding
            .get("process_start_marker")
            .and_then(Value::as_str)
            .is_none_or(str::is_empty)
    {
        return Err(WorkcellError::InvalidDemand(
            "material_binding requires process kind, pid and process_start_marker".into(),
        ));
    }
    if !matches!(
        binding
            .get("instance_evidence_grade")
            .and_then(Value::as_str),
        Some("live-pid" | "gateway-confirmed")
    ) || binding
        .get("instance_seams")
        .and_then(Value::as_array)
        .is_none()
    {
        return Err(WorkcellError::InvalidDemand(
            "material_binding requires detected instance evidence and an instance_seams array"
                .into(),
        ));
    }
    let executable = binding
        .get("executable")
        .and_then(Value::as_object)
        .ok_or_else(|| {
            WorkcellError::InvalidDemand("material_binding.executable must be an object".into())
        })?;
    exact_keys(
        executable,
        &["path", "sha256"],
        "material_binding.executable",
    )?;
    for field in ["path", "sha256"] {
        if executable
            .get(field)
            .and_then(Value::as_str)
            .is_none_or(str::is_empty)
        {
            return Err(WorkcellError::InvalidDemand(format!(
                "material_binding.executable.{field} must be a non-empty string"
            )));
        }
    }

    let interval = object
        .get("interval")
        .and_then(Value::as_object)
        .ok_or_else(|| WorkcellError::InvalidDemand("interval must be an object".into()))?;
    exact_keys(
        interval,
        &[
            "started_at",
            "ended_at",
            "duration_ms",
            "requested_duration_ms",
        ],
        "interval",
    )?;
    let started = unix_marker(interval.get("started_at"), "interval.started_at")?;
    let ended = unix_marker(interval.get("ended_at"), "interval.ended_at")?;
    let duration = interval
        .get("duration_ms")
        .and_then(Value::as_u64)
        .ok_or_else(|| WorkcellError::InvalidDemand("interval.duration_ms must be u64".into()))?;
    if ended < started || ended - started != duration {
        return Err(WorkcellError::InvalidDemand(
            "interval timestamps and duration_ms do not agree".into(),
        ));
    }
    let requested = interval
        .get("requested_duration_ms")
        .and_then(Value::as_u64)
        .ok_or_else(|| {
            WorkcellError::InvalidDemand("interval.requested_duration_ms must be u64".into())
        })?;
    if requested > MAX_INTERVAL.as_millis() as u64 || duration < requested {
        return Err(WorkcellError::InvalidDemand(
            "interval is outside the requested bounded observation".into(),
        ));
    }

    let provider = object
        .get("provider")
        .and_then(Value::as_object)
        .ok_or_else(|| WorkcellError::InvalidDemand("provider must be an object".into()))?;
    exact_keys(
        provider,
        &[
            "provider_ref",
            "source",
            "platform",
            "collection",
            "raw_native_refs",
            "privacy",
        ],
        "provider",
    )?;
    for field in ["provider_ref", "source", "platform"] {
        if provider
            .get(field)
            .and_then(Value::as_str)
            .is_none_or(str::is_empty)
        {
            return Err(WorkcellError::InvalidDemand(format!(
                "provider.{field} must be a non-empty string"
            )));
        }
    }
    if provider.get("collection").and_then(Value::as_str) != Some("bounded-on-demand") {
        return Err(WorkcellError::InvalidDemand(
            "provider.collection must be `bounded-on-demand`".into(),
        ));
    }
    let raw_refs = provider
        .get("raw_native_refs")
        .and_then(Value::as_array)
        .ok_or_else(|| {
            WorkcellError::InvalidDemand("provider.raw_native_refs must be an array".into())
        })?;
    if raw_refs.is_empty()
        || raw_refs
            .iter()
            .any(|reference| reference.as_str().is_none_or(str::is_empty))
    {
        return Err(WorkcellError::InvalidDemand(
            "provider.raw_native_refs must contain non-empty opaque refs".into(),
        ));
    }
    let privacy = provider
        .get("privacy")
        .and_then(Value::as_object)
        .ok_or_else(|| WorkcellError::InvalidDemand("provider.privacy must be an object".into()))?;
    exact_keys(
        privacy,
        &["argv_collected", "environment_collected"],
        "provider.privacy",
    )?;
    if privacy.get("argv_collected").and_then(Value::as_bool) != Some(false)
        || privacy
            .get("environment_collected")
            .and_then(Value::as_bool)
            != Some(false)
    {
        return Err(WorkcellError::InvalidDemand(
            "resource usage readings must disclose argv/environment as not collected".into(),
        ));
    }

    let metrics = object
        .get("metrics")
        .and_then(Value::as_object)
        .ok_or_else(|| WorkcellError::InvalidDemand("metrics must be an object".into()))?;
    let expected = [
        ("cpu_time", "milliseconds"),
        ("cpu_utilisation", "percent"),
        ("memory_rss", "bytes"),
        ("memory_peak_rss", "bytes"),
        ("gpu_utilisation", "percent"),
        ("vram", "bytes"),
        ("network_bytes", "bytes"),
        ("storage_io_bytes", "bytes"),
    ];
    if metrics.len() != expected.len() {
        return Err(WorkcellError::InvalidDemand(
            "metrics must contain exactly the portable v1 metric fields".into(),
        ));
    }
    for (name, unit) in expected {
        validate_metric(metrics.get(name), name, unit)?;
    }

    let correlations = object
        .get("external_correlation_refs")
        .and_then(Value::as_array)
        .ok_or_else(|| {
            WorkcellError::InvalidDemand("external_correlation_refs must be an array".into())
        })?;
    if correlations
        .iter()
        .any(|reference| reference.as_str().is_none_or(str::is_empty))
    {
        return Err(WorkcellError::InvalidDemand(
            "external correlation refs must remain non-empty opaque strings".into(),
        ));
    }
    Ok(())
}

fn required_reading_str(reading: &Value, field: &str) -> Result<String> {
    reading
        .get(field)
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
        .map(str::to_owned)
        .ok_or_else(|| {
            WorkcellError::InvalidDemand(format!(
                "resource usage reading requires non-empty `{field}`"
            ))
        })
}

fn exact_keys(
    object: &serde_json::Map<String, Value>,
    expected: &[&str],
    context: &str,
) -> Result<()> {
    if object.len() != expected.len() || object.keys().any(|key| !expected.contains(&key.as_str()))
    {
        return Err(WorkcellError::InvalidDemand(format!(
            "{context} must contain exactly the v1 contract fields"
        )));
    }
    Ok(())
}

fn unix_marker(value: Option<&Value>, field: &str) -> Result<u64> {
    value
        .and_then(Value::as_str)
        .and_then(|value| value.strip_prefix("unix-ms:"))
        .and_then(|value| value.parse::<u64>().ok())
        .ok_or_else(|| WorkcellError::InvalidDemand(format!("{field} must read `unix-ms:<u64>`")))
}

fn validate_metric(metric: Option<&Value>, name: &str, expected_unit: &str) -> Result<()> {
    let metric = metric
        .and_then(Value::as_object)
        .ok_or_else(|| WorkcellError::InvalidDemand(format!("metrics.{name} must be an object")))?;
    let standing = metric
        .get("standing")
        .and_then(Value::as_str)
        .ok_or_else(|| {
            WorkcellError::InvalidDemand(format!("metrics.{name}.standing must be a string"))
        })?;
    match standing {
        "observed" | "provider-reported" | "sampled" | "derived" => {
            if metric
                .get("value")
                .and_then(Value::as_f64)
                .is_none_or(|value| !value.is_finite() || value < 0.0)
                || metric.get("unit").and_then(Value::as_str) != Some(expected_unit)
            {
                return Err(WorkcellError::InvalidDemand(format!(
                    "metrics.{name} with standing `{standing}` requires a value and unit `{expected_unit}`"
                )));
            }
            let provenance_field = if standing == "derived" {
                "derivation"
            } else {
                "evidence"
            };
            if metric
                .get(provenance_field)
                .and_then(Value::as_str)
                .is_none_or(str::is_empty)
            {
                return Err(WorkcellError::InvalidDemand(format!(
                    "metrics.{name} with standing `{standing}` requires `{provenance_field}`"
                )));
            }
            let allowed = ["standing", "value", "unit", provenance_field];
            if metric.keys().any(|key| !allowed.contains(&key.as_str())) {
                return Err(WorkcellError::InvalidDemand(format!(
                    "metrics.{name} carries fields outside its `{standing}` contract"
                )));
            }
        }
        "unavailable" | "unsupported" => {
            if metric
                .get("reason")
                .and_then(Value::as_str)
                .is_none_or(str::is_empty)
                || metric.contains_key("value")
            {
                return Err(WorkcellError::InvalidDemand(format!(
                    "metrics.{name} with standing `{standing}` requires a reason and must not carry a value"
                )));
            }
            exact_keys(metric, &["standing", "reason"], &format!("metrics.{name}"))?;
        }
        _ => {
            return Err(WorkcellError::InvalidDemand(format!(
                "metrics.{name}.standing `{standing}` is not a portable v1 standing"
            )))
        }
    }
    Ok(())
}

fn metric_observed<T: Into<Value>>(value: Option<T>, unit: &str, evidence: &str) -> Value {
    match value {
        Some(value) => json!({
            "standing": "observed",
            "value": value.into(),
            "unit": unit,
            "evidence": evidence,
        }),
        None => json!({
            "standing": "unavailable",
            "reason": format!("{evidence} was not supplied by the local provider"),
        }),
    }
}

fn metric_derived(value: Option<f64>, unit: &str, derivation: &str) -> Value {
    match value {
        Some(value) if value.is_finite() => json!({
            "standing": "derived",
            "value": value,
            "unit": unit,
            "derivation": derivation,
        }),
        _ => json!({
            "standing": "unavailable",
            "reason": "the two samples did not establish a CPU-time delta over a positive wall interval",
        }),
    }
}

fn metric_unsupported(reason: &str) -> Value {
    json!({"standing": "unsupported", "reason": reason})
}

fn select_pid(record: &Value, requested_pid: Option<u32>) -> Result<u32> {
    if record.get("liveness").and_then(Value::as_str) != Some(LIVENESS_LIVE) {
        return Err(WorkcellError::Unavailable(format!(
            "instance `{}` is not live",
            record["instance_ref"].as_str().unwrap_or("unknown")
        )));
    }
    let pids = record
        .get("pids")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(Value::as_u64)
        .filter_map(|pid| u32::try_from(pid).ok())
        .collect::<Vec<_>>();
    match requested_pid {
        Some(pid) if pids.contains(&pid) => Ok(pid),
        Some(pid) => Err(WorkcellError::InvalidDemand(format!(
            "pid {pid} is not bound to instance `{}`",
            record["instance_ref"].as_str().unwrap_or("unknown")
        ))),
        None if pids.len() == 1 => Ok(pids[0]),
        None if pids.is_empty() => Err(WorkcellError::Unavailable(
            "the live instance has no process PID that this provider can observe".into(),
        )),
        None => Err(WorkcellError::InvalidDemand(
            "the instance has multiple live PIDs; select one with --pid".into(),
        )),
    }
}

fn binding_snapshot(record: &Value, pid: u32) -> Result<Value> {
    validate_instance_record(record)?;
    if !record["pids"]
        .as_array()
        .is_some_and(|pids| pids.iter().any(|value| value.as_u64() == Some(pid.into())))
    {
        return Err(WorkcellError::Unavailable(format!(
            "pid {pid} is no longer bound to instance `{}`",
            record["instance_ref"].as_str().unwrap_or("unknown")
        )));
    }
    Ok(json!({
        "instance_ref": record["instance_ref"],
        "harness_ref": record["harness_ref"],
        "workcell_ref": record["workcell_ref"],
        "pid": pid,
        "instance_pids": record["pids"],
        "executable": record["executable"],
        "evidence_grade": record["evidence_grade"],
        "liveness": record["liveness"],
    }))
}

fn sample_process(pid: u32) -> Result<ProcessSample> {
    let output = Command::new("ps")
        .args([
            "-ww",
            "-p",
            &pid.to_string(),
            "-o",
            "pid=",
            "-o",
            "lstart=",
            "-o",
            "time=",
            "-o",
            "rss=",
            "-o",
            "comm=",
        ])
        .output()
        .map_err(|error| WorkcellError::Unavailable(format!("run local ps provider: {error}")))?;
    if !output.status.success() {
        return Err(WorkcellError::Unavailable(format!(
            "local ps provider could not observe pid {pid}"
        )));
    }
    let stdout = String::from_utf8(output.stdout).map_err(|error| {
        WorkcellError::Unavailable(format!(
            "local ps provider returned non-UTF-8 output: {error}"
        ))
    })?;
    parse_ps_sample(&stdout, pid)
}

fn parse_ps_sample(stdout: &str, expected_pid: u32) -> Result<ProcessSample> {
    let fields = stdout.split_whitespace().collect::<Vec<_>>();
    if fields.len() < 9 {
        return Err(WorkcellError::Unavailable(
            "local ps provider omitted required process identity fields".into(),
        ));
    }
    let pid = fields[0].parse::<u32>().map_err(|_| {
        WorkcellError::Unavailable("local ps provider returned an invalid pid".into())
    })?;
    if pid != expected_pid {
        return Err(WorkcellError::Unavailable(format!(
            "local ps provider returned pid {pid} while observing {expected_pid}"
        )));
    }
    let start_marker = fields[1..6].join(" ");
    let cpu_time_millis = parse_cpu_time(fields[6]);
    let rss_bytes = fields[7]
        .parse::<u64>()
        .ok()
        .and_then(|kib| kib.checked_mul(1024));
    let executable = PathBuf::from(fields[8..].join(" "));
    if executable.as_os_str().is_empty() {
        return Err(WorkcellError::Unavailable(
            "local ps provider omitted process executable identity".into(),
        ));
    }
    Ok(ProcessSample {
        pid,
        start_marker,
        executable: observed_executable(pid, &executable),
        cpu_time_millis,
        rss_bytes,
    })
}

fn parse_cpu_time(value: &str) -> Option<u64> {
    let (days, clock) = match value.split_once('-') {
        Some((days, clock)) => (days.parse::<u64>().ok()?, clock),
        None => (0, value),
    };
    let parts = clock.split(':').collect::<Vec<_>>();
    let (hours, minutes, seconds) = match parts.as_slice() {
        [minutes, seconds] => (0, minutes.parse::<u64>().ok()?, *seconds),
        [hours, minutes, seconds] => (
            hours.parse::<u64>().ok()?,
            minutes.parse::<u64>().ok()?,
            *seconds,
        ),
        _ => return None,
    };
    let (seconds, millis) = seconds.split_once('.').map_or_else(
        || Some((seconds.parse::<u64>().ok()?, 0)),
        |(seconds, fraction)| {
            let padded = format!("{fraction:0<3}");
            Some((seconds.parse::<u64>().ok()?, padded[..3].parse().ok()?))
        },
    )?;
    (((days * 24 + hours) * 60 + minutes) * 60 + seconds)
        .checked_mul(1000)?
        .checked_add(millis)
}

#[cfg(target_os = "linux")]
fn observed_executable(pid: u32, fallback: &Path) -> PathBuf {
    fs::read_link(format!("/proc/{pid}/exe")).unwrap_or_else(|_| fallback.to_path_buf())
}

#[cfg(not(target_os = "linux"))]
fn observed_executable(_pid: u32, fallback: &Path) -> PathBuf {
    fallback.to_path_buf()
}

fn validate_process_binding(record: &Value, sample: &ProcessSample) -> Result<()> {
    let expected = record
        .pointer("/executable/path")
        .and_then(Value::as_str)
        .map(PathBuf::from)
        .ok_or_else(|| WorkcellError::InvalidDemand("instance executable path is absent".into()))?;
    if canonical_for_comparison(&expected) != canonical_for_comparison(&sample.executable) {
        return Err(WorkcellError::Unavailable(format!(
            "pid {} executable identity does not match registered instance `{}`",
            sample.pid,
            record["instance_ref"].as_str().unwrap_or("unknown")
        )));
    }
    Ok(())
}

fn canonical_for_comparison(path: &Path) -> PathBuf {
    fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf())
}

fn validate_same_process(first: &ProcessSample, second: &ProcessSample) -> Result<()> {
    if first.pid != second.pid
        || first.start_marker != second.start_marker
        || canonical_for_comparison(&first.executable)
            != canonical_for_comparison(&second.executable)
    {
        return Err(WorkcellError::Unavailable(format!(
            "pid {} was replaced during the resource observation interval",
            first.pid
        )));
    }
    Ok(())
}

fn unix_millis() -> Result<u64> {
    let millis = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|error| WorkcellError::OperationFailed(format!("read system clock: {error}")))?
        .as_millis();
    u64::try_from(millis)
        .map_err(|_| WorkcellError::OperationFailed("system clock exceeds u64 milliseconds".into()))
}

fn usage_ref(instance_ref: &str, pid: u32, start: &str, began: u64, ended: u64) -> String {
    let mut hasher = Sha256::new();
    for material in [
        instance_ref.as_bytes(),
        &pid.to_be_bytes(),
        start.as_bytes(),
        &began.to_be_bytes(),
        &ended.to_be_bytes(),
    ] {
        hasher.update(material);
        hasher.update(b"\0");
    }
    let digest = hasher.finalize();
    let hex = digest
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect::<String>();
    format!("usage:{hex}")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::instance_registry::{
        build_instance_record, InstanceObservation, RegisterOutcome, EVIDENCE_LIVE_PID,
    };
    use epilogos_workcell_core::WorkcellRef;

    fn temp_root(label: &str) -> PathBuf {
        let root = std::env::temp_dir().join(format!(
            "workcell-resource-usage-{label}-{}-{}",
            std::process::id(),
            unix_millis().unwrap()
        ));
        fs::create_dir_all(&root).unwrap();
        root
    }

    fn live_self_registry(label: &str) -> (PathBuf, InstanceRegistry, String) {
        let root = temp_root(label);
        let workcell_ref = WorkcellRef::new("workcell:test-owner-machine").unwrap();
        let registry = InstanceRegistry::new(&root, workcell_ref.clone());
        let executable = std::env::current_exe().unwrap();
        let record = build_instance_record(
            &workcell_ref,
            &InstanceObservation {
                slug: "resource-test".into(),
                executable: executable.clone(),
                executable_sha256: "real-test-process".into(),
                identity_material: executable.display().to_string(),
                pids: vec![std::process::id()],
                evidence_grade: EVIDENCE_LIVE_PID.into(),
                seams: vec![],
            },
        );
        let reference = record["instance_ref"].as_str().unwrap().to_owned();
        assert_eq!(
            registry.register(record).unwrap(),
            RegisterOutcome::Registered
        );
        (root, registry, reference)
    }

    #[test]
    fn real_local_process_produces_bounded_cpu_and_memory_evidence() {
        let (root, registry, reference) = live_self_registry("real");
        let report = observe_resource_usage(
            &registry,
            &reference,
            Some(std::process::id()),
            Duration::from_millis(15),
            vec!["opaque:test-correlation".into()],
        )
        .unwrap()
        .as_json();
        assert_eq!(report["schema"], RESOURCE_USAGE_SCHEMA);
        assert_eq!(report["workcell_ref"], "workcell:test-owner-machine");
        assert_eq!(report["harness_instance_ref"], reference);
        assert_eq!(report["material_binding"]["pid"], std::process::id());
        assert_eq!(report["metrics"]["cpu_time"]["standing"], "observed");
        assert_eq!(report["metrics"]["memory_rss"]["standing"], "observed");
        assert_eq!(
            report["metrics"]["gpu_utilisation"]["standing"],
            "unsupported"
        );
        assert_eq!(report["metrics"]["vram"]["standing"], "unsupported");
        assert_eq!(report["provider"]["privacy"]["argv_collected"], false);
        assert_eq!(
            report["provider"]["privacy"]["environment_collected"],
            false
        );
        assert_eq!(
            report["external_correlation_refs"][0],
            "opaque:test-correlation"
        );
        assert!(report["interval"]["duration_ms"].as_u64().unwrap() >= 15);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn pid_not_bound_to_instance_is_rejected_before_collection() {
        let (root, registry, reference) = live_self_registry("foreign-pid");
        let error = observe_resource_usage(
            &registry,
            &reference,
            Some(std::process::id().saturating_add(1)),
            Duration::ZERO,
            vec![],
        )
        .unwrap_err();
        assert!(error.to_string().contains("is not bound"));
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn replacement_and_malformed_native_samples_are_rejected() {
        let first = ProcessSample {
            pid: 42,
            start_marker: "Wed Sep 9 10:00:00 2026".into(),
            executable: PathBuf::from("/bin/tool"),
            cpu_time_millis: Some(100),
            rss_bytes: Some(1024),
        };
        let mut replacement = first.clone();
        replacement.start_marker = "Wed Sep 9 10:01:00 2026".into();
        assert!(validate_same_process(&first, &replacement)
            .unwrap_err()
            .to_string()
            .contains("replaced"));
        assert!(parse_ps_sample("42 incomplete", 42).is_err());
        assert!(parse_ps_sample("43 Wed Sep 9 10:00:00 2026 00:00.01 1 /bin/tool", 42).is_err());
    }

    #[test]
    fn native_cpu_time_parser_preserves_cumulative_duration() {
        assert_eq!(parse_cpu_time("01:02.34"), Some(62_340));
        assert_eq!(parse_cpu_time("02:01:02"), Some(7_262_000));
        assert_eq!(parse_cpu_time("3-02:01:02"), Some(266_462_000));
        assert_eq!(parse_cpu_time("not-time"), None);
    }

    #[test]
    fn versioned_fixture_roundtrips_and_adversarial_claims_are_rejected() {
        let schema: Value =
            serde_json::from_str(include_str!("../schemas/resource-usage-v1.schema.json")).unwrap();
        assert_eq!(
            schema["$id"],
            "https://workcell.epilogos.dev/schemas/resource-usage-v1.schema.json"
        );
        let fixture: Value =
            serde_json::from_str(include_str!("../fixtures/resource-usage-v1.json")).unwrap();
        validate_resource_usage(&fixture).unwrap();

        let mut missing_as_zero = fixture.clone();
        missing_as_zero["metrics"]["gpu_utilisation"]["value"] = 0.into();
        assert!(validate_resource_usage(&missing_as_zero).is_err());

        let mut wrong_unit = fixture.clone();
        wrong_unit["metrics"]["memory_rss"]["unit"] = "kilobytes".into();
        assert!(validate_resource_usage(&wrong_unit).is_err());

        let mut false_interval = fixture.clone();
        false_interval["interval"]["duration_ms"] = 99.into();
        assert!(validate_resource_usage(&false_interval).is_err());

        let mut exposed_private_source = fixture.clone();
        exposed_private_source["provider"]["privacy"]["argv_collected"] = true.into();
        assert!(validate_resource_usage(&exposed_private_source).is_err());

        let mut false_ok = fixture.clone();
        false_ok["ok"] = false.into();
        assert!(validate_resource_usage(&false_ok).is_err());

        let mut missing_ok = fixture.clone();
        missing_ok.as_object_mut().unwrap().remove("ok");
        assert!(validate_resource_usage(&missing_ok).is_err());

        let mut top_level_argv = fixture.clone();
        top_level_argv["argv"] = json!(["--secret", "material"]);
        assert!(validate_resource_usage(&top_level_argv).is_err());

        let mut top_level_environment = fixture.clone();
        top_level_environment["environment"] = json!({"TOKEN": "material"});
        assert!(validate_resource_usage(&top_level_environment).is_err());

        let mut provider_secret_extra = fixture.clone();
        provider_secret_extra["provider"]["environment"] = json!({"TOKEN": "material"});
        assert!(validate_resource_usage(&provider_secret_extra).is_err());

        let mut binding_argv_extra = fixture.clone();
        binding_argv_extra["material_binding"]["argv"] = json!(["--secret"]);
        assert!(validate_resource_usage(&binding_argv_extra).is_err());
    }

    #[test]
    fn projection_and_revival_cannot_reuse_an_old_process_interval() {
        let (root, registry, reference) = live_self_registry("projection");
        let source = registry.show(&reference).unwrap();
        let target = WorkcellRef::new("workcell:projected-target").unwrap();
        let projected = crate::instance_projection::project_record(&source, &target).unwrap();
        assert_eq!(projected["instance_ref"], source["instance_ref"]);
        assert_eq!(projected["workcell_ref"], target.as_str());
        assert!(select_pid(&projected, None)
            .unwrap_err()
            .to_string()
            .contains("no process PID"));

        let before = binding_snapshot(&source, std::process::id()).unwrap();
        let mut revived = source.clone();
        revived["pids"] = json!([std::process::id().saturating_add(1)]);
        assert!(binding_snapshot(&revived, std::process::id()).is_err());
        assert_ne!(before["pid"], revived["pids"][0]);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn persisted_cross_workcell_record_is_rejected_before_os_collection() {
        let (root, registry, reference) = live_self_registry("cross-workcell-corruption");
        let path = root.join("instances/registry.json");
        let mut file: Value = serde_json::from_str(&fs::read_to_string(&path).unwrap()).unwrap();
        file["instances"][&reference]["workcell_ref"] = "workcell:different".into();
        fs::write(&path, serde_json::to_vec_pretty(&file).unwrap()).unwrap();
        let error = observe_resource_usage(
            &registry,
            &reference,
            Some(std::process::id()),
            Duration::ZERO,
            vec![],
        )
        .unwrap_err();
        assert!(error.to_string().contains("different Workcell"));
        fs::remove_dir_all(root).unwrap();
    }
}
