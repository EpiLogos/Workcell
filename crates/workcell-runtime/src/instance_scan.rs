//! Live harness-instance scanner.
//!
//! Detection law: a workcell is awake to processes. The scanner reconciles
//! three evidence sources into the instance registry:
//!
//! 1. Actuation detection (`actuation harness detect --json`) — executable
//!    receipts (path + sha256) and facet seams. Actuation owns detection
//!    probes; Workcell intakes their records (routing-of-powers stays with
//!    Actuation).
//! 2. The live pid table (`ps -axo pid=,comm=`) — which detected harnesses
//!    are actually executing right now. Install presence is not instance
//!    presence.
//! 3. The ai-kit Agency Gateway well-known endpoint — a carrier answer
//!    upgrades a Hermes instance from `live-pid` to `gateway-confirmed`.
//!
//! Intake law (unchanged from ai-kit `actuation_harness_detection.rs`): a
//! failed detection run is `unavailable {reason}` and changes nothing in the
//! registry — never an empty set read as absence. Only a completed scan may
//! advance liveness (live → stale after N=2 missed scans, disclosed, never
//! silently deleted).

use std::{
    collections::BTreeSet,
    io::{BufRead, BufReader, Write},
    path::{Path, PathBuf},
    process::{Command, Stdio},
    time::Duration,
};

use epilogos_workcell_core::{Result, WorkcellError, WorkcellRef};
use serde_json::{json, Value};

use crate::instance_registry::{
    build_instance_record, InstanceObservation, InstanceRegistry, RegisterOutcome,
    EVIDENCE_GATEWAY_CONFIRMED, EVIDENCE_LIVE_PID, LIVENESS_LIVE, LIVENESS_STALE,
};

pub const STALE_AFTER_MISSED_SCANS: u64 = 2;

/// Executable-name → harness slug, mirroring the Actuation descriptor
/// modules (`Actuation/detection/harnesses/*.mjs` probe executable names).
/// This table is pid-matching vocabulary only; detection probes stay with
/// Actuation.
pub const PID_ALIASES: [(&str, &str); 12] = [
    ("claude", "claude-code"),
    ("codex", "codex"),
    ("gemini", "gemini"),
    ("antigravity", "gemini-antigravity"),
    ("hermes", "hermes"),
    ("kimi", "kimi"),
    ("ollama", "ollama"),
    ("openclaw", "openclaw"),
    ("pi", "pi"),
    ("zcode", "zcode"),
    ("grok", "grok-bot"),
    ("qwen", "qwen"),
];

/// What the scanner observed about one live process this scan.
#[derive(Debug, Clone)]
pub struct ObservedInstance {
    pub record: Value,
}

/// Liveness transitions applied by one completed scan.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ScanTransitions {
    pub registered: usize,
    pub refreshed: usize,
    pub revived: usize,
    pub went_stale: usize,
    pub still_stale: usize,
}

/// One scan report. `status: "unavailable"` names a failed run and carries
/// no transitions; `ok` reports what the scan changed.
#[derive(Debug, Clone)]
pub struct ScanReport {
    pub status: &'static str,
    pub reason: Option<String>,
    pub workcell_ref: String,
    pub live: Vec<Value>,
    pub stale: Vec<Value>,
    pub transitions: ScanTransitions,
    /// pid comms that matched no known alias; named, never silently dropped.
    pub unmatched_processes: Vec<String>,
}

impl ScanReport {
    fn unavailable(workcell_ref: &WorkcellRef, reason: String) -> Self {
        Self {
            status: "unavailable",
            reason: Some(reason),
            workcell_ref: workcell_ref.to_string(),
            live: Vec::new(),
            stale: Vec::new(),
            transitions: ScanTransitions::default(),
            unmatched_processes: Vec::new(),
        }
    }
}

/// Gathered live inputs. `scan_live` fills these from the host; tests feed
/// them directly to `reconcile`.
pub struct ScanInputs {
    /// Parsed Actuation detection document (`actuation.harness-detection/v1`).
    pub detection: Value,
    /// Live pid table: (pid, executable basename).
    pub processes: Vec<(u32, String)>,
    /// Whether the ai-kit Agency Gateway answered its well-known endpoint
    /// this scan. An independent detection seam: Hermes runs under
    /// python/node runtimes its pid comm cannot name, so the carrier answer
    /// is how a Hermes instance is detected at all on such hosts.
    pub gateway_answering: bool,
}

/// Run one full live scan against the registry under `state_root`.
///
/// Intake law: if Actuation detection or the pid table cannot be read, the
/// report is `unavailable {reason}` and the registry is untouched.
pub fn scan_live(
    state_root: impl Into<PathBuf>,
    workcell_ref: &WorkcellRef,
    actuation_command: &str,
) -> ScanReport {
    let registry = InstanceRegistry::new(state_root, workcell_ref.clone());

    let detection = match run_actuation_detection(actuation_command) {
        Ok(detection) => detection,
        Err(error) => return ScanReport::unavailable(workcell_ref, error),
    };
    let processes = match read_pid_table() {
        Ok(processes) => processes,
        Err(error) => return ScanReport::unavailable(workcell_ref, error),
    };
    let gateway = gateway_answering(&default_gateway_socket());

    reconcile(
        registry,
        workcell_ref,
        ScanInputs {
            detection,
            processes,
            gateway_answering: gateway,
        },
    )
}

/// Pure reconciliation core: merge detection + pid evidence into the
/// registry. Separated from `scan_live` so the law is testable without a
/// host.
pub fn reconcile(
    registry: InstanceRegistry,
    workcell_ref: &WorkcellRef,
    inputs: ScanInputs,
) -> ScanReport {
    let detection = match validate_detection_document(&inputs.detection) {
        Ok(()) => inputs.detection,
        Err(error) => {
            return ScanReport::unavailable(
                workcell_ref,
                format!("invalid detection document: {error}"),
            )
        }
    };

    let observed = match observations_from_detection(
        &detection,
        &inputs.processes,
        workcell_ref,
        inputs.gateway_answering,
    ) {
        Ok(mut observed) => {
            // Gateway seam: when the carrier answers but no pid-matched
            // hermes observation exists (hermes runs under runtimes comm
            // cannot name), the carrier answer itself is the detection.
            if inputs.gateway_answering
                && !observed.iter().any(|instance| {
                    instance.record["harness_ref"]
                        .as_str()
                        .unwrap_or_default()
                        .trim_start_matches("harness/")
                        .starts_with("hermes")
                })
            {
                observed.push(gateway_only_hermes_observation(workcell_ref, &detection));
            }
            observed
        }
        Err(error) => {
            return ScanReport::unavailable(workcell_ref, format!("intake detection: {error}"))
        }
    };

    let observed_refs: BTreeSet<String> = observed
        .iter()
        .map(|instance| {
            instance.record["instance_ref"]
                .as_str()
                .unwrap_or_default()
                .to_owned()
        })
        .collect();

    let mut transitions = ScanTransitions::default();
    for instance in &observed {
        let reference = instance.record["instance_ref"]
            .as_str()
            .unwrap_or_default()
            .to_owned();
        let prior = registry.show(&reference).ok();
        let was_stale = prior
            .as_ref()
            .and_then(|existing| existing.get("liveness").and_then(Value::as_str))
            == Some(LIVENESS_STALE);
        let was_present = prior.is_some();
        match registry.register(instance.record.clone()) {
            Ok(RegisterOutcome::Registered) => {
                if !was_present {
                    transitions.registered += 1;
                } else if was_stale {
                    transitions.revived += 1;
                } else {
                    transitions.refreshed += 1;
                }
            }
            Ok(RegisterOutcome::Unchanged) => {
                if !was_present {
                    transitions.registered += 1;
                }
            }
            Ok(RegisterOutcome::Conflict { .. }) => {
                // A live observation conflicting with the registered identity
                // is named and left for the owner; the registry keeps its
                // existing record (register() never auto-resolves).
            }
            Err(error) => {
                return ScanReport::unavailable(workcell_ref, format!("register instance: {error}"))
            }
        }
    }

    // Advance liveness only for records that belong to this workcell and
    // were not observed this scan.
    let mut live = Vec::new();
    let mut stale = Vec::new();
    let mut miss_updates: Vec<(String, u64, &'static str)> = Vec::new();
    if let Ok(records) = registry.list() {
        for mut record in records {
            let reference = record["instance_ref"]
                .as_str()
                .unwrap_or_default()
                .to_owned();
            if observed_refs.contains(&reference) {
                record["consecutive_misses"] = 0u64.into();
                record["liveness"] = LIVENESS_LIVE.into();
                miss_updates.push((reference.clone(), 0, LIVENESS_LIVE));
                live.push(record);
            } else {
                let misses = record
                    .get("consecutive_misses")
                    .and_then(Value::as_u64)
                    .unwrap_or(0)
                    + 1;
                let liveness = if misses >= STALE_AFTER_MISSED_SCANS {
                    LIVENESS_STALE
                } else {
                    LIVENESS_LIVE
                };
                let was_stale =
                    record.get("liveness").and_then(Value::as_str) == Some(LIVENESS_STALE);
                record["consecutive_misses"] = misses.into();
                record["liveness"] = liveness.into();
                miss_updates.push((reference.clone(), misses, liveness));
                if liveness == LIVENESS_STALE && !was_stale {
                    transitions.went_stale += 1;
                    stale.push(record);
                } else if liveness == LIVENESS_STALE {
                    transitions.still_stale += 1;
                    stale.push(record);
                } else {
                    live.push(record);
                }
            }
        }
    }
    if let Err(error) = registry.apply_liveness(&miss_updates) {
        return ScanReport::unavailable(workcell_ref, format!("apply liveness: {error}"));
    }

    // A detected harness with no live pid is installed, not instanced — it
    // stays out of the live registry (install knowledge is Actuation's).
    let matched_comms: BTreeSet<String> = inputs
        .processes
        .iter()
        .map(|(_, comm)| comm.clone())
        .collect();
    let known_aliases: BTreeSet<&str> = PID_ALIASES.iter().map(|(alias, _)| *alias).collect();
    let unmatched_processes = matched_comms
        .into_iter()
        .filter(|comm| known_aliases.contains(comm.as_str()))
        .filter(|comm| {
            !observed.iter().any(|instance| {
                instance.record["harness_ref"]
                    .as_str()
                    .unwrap_or_default()
                    .trim_start_matches("harness/")
                    == alias_slug(comm)
            })
        })
        .collect();

    ScanReport {
        status: "ok",
        reason: None,
        workcell_ref: workcell_ref.to_string(),
        live,
        stale,
        transitions,
        unmatched_processes,
    }
}

fn alias_slug(comm: &str) -> &str {
    PID_ALIASES
        .iter()
        .find(|(alias, _)| alias == &comm)
        .map(|(_, slug)| *slug)
        .unwrap_or(comm)
}

/// Build instance observations from a validated detection document plus the
/// live pid table. One observation per (detected harness, observed pid).
fn observations_from_detection(
    detection: &Value,
    processes: &[(u32, String)],
    workcell_ref: &WorkcellRef,
    gateway_answer: bool,
) -> Result<Vec<ObservedInstance>> {
    let mut observed = Vec::new();
    let harnesses = detection
        .get("harnesses")
        .and_then(Value::as_array)
        .ok_or_else(|| {
            WorkcellError::OperationFailed("detection document lacks harnesses".into())
        })?;

    for harness in harnesses {
        if harness.get("state").and_then(Value::as_str) != Some("detected") {
            continue;
        }
        let slug = harness
            .get("slug")
            .and_then(Value::as_str)
            .ok_or_else(|| WorkcellError::OperationFailed("detected harness lacks slug".into()))?;
        let receipts = harness.get("receipts");
        let executable_path = receipts
            .and_then(|receipts| receipts.get("executable"))
            .and_then(Value::as_str)
            .unwrap_or_default();
        let executable_sha = receipts
            .and_then(|receipts| receipts.get("sha256"))
            .and_then(Value::as_str)
            .unwrap_or_default();
        let executable_name = Path::new(executable_path)
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or_default()
            .to_owned();
        let seams = harness
            .get("facets")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();

        let pids: Vec<u32> = processes
            .iter()
            .filter(|(_, comm)| comm == &executable_name || alias_matches_slug(comm, slug))
            .map(|(pid, _)| *pid)
            .collect();
        if pids.is_empty() {
            continue; // installed, not executing — not an instance
        }

        let mut evidence_grade = EVIDENCE_LIVE_PID;
        if (slug == "hermes" || slug == "hermes-acp") && gateway_answer {
            evidence_grade = EVIDENCE_GATEWAY_CONFIRMED;
        }
        let identity_material = if executable_path.is_empty() {
            format!("pid-observed:{executable_name}")
        } else {
            executable_path.to_owned()
        };
        let record = build_instance_record(
            workcell_ref,
            &InstanceObservation {
                slug: slug.to_owned(),
                executable: PathBuf::from(executable_path),
                executable_sha256: executable_sha.to_owned(),
                identity_material,
                pids,
                evidence_grade: evidence_grade.to_owned(),
                seams: seams.clone(),
            },
        );
        observed.push(ObservedInstance { record });
    }
    Ok(observed)
}

fn alias_matches_slug(comm: &str, slug: &str) -> bool {
    PID_ALIASES
        .iter()
        .any(|(alias, alias_slug)| alias == &comm && alias_slug == &slug)
}

/// A Hermes instance detected only through its carrier answer: no pid match
/// (the runtime comm cannot name it) but the gateway protocol replied.
fn gateway_only_hermes_observation(
    workcell_ref: &WorkcellRef,
    detection: &Value,
) -> ObservedInstance {
    let hermes = detection
        .get("harnesses")
        .and_then(Value::as_array)
        .and_then(|harnesses| {
            harnesses.iter().find(|harness| {
                harness
                    .get("slug")
                    .and_then(Value::as_str)
                    .map(|slug| slug.starts_with("hermes"))
                    .unwrap_or(false)
                    && harness.get("state").and_then(Value::as_str) == Some("detected")
            })
        });
    let receipts = hermes.and_then(|harness| harness.get("receipts"));
    let executable_path = receipts
        .and_then(|receipts| receipts.get("executable"))
        .and_then(Value::as_str)
        .unwrap_or("/unknown/hermes")
        .to_owned();
    let executable_sha = receipts
        .and_then(|receipts| receipts.get("sha256"))
        .and_then(Value::as_str)
        .unwrap_or("unavailable-gateway-only")
        .to_owned();
    let seams = hermes
        .and_then(|harness| harness.get("facets"))
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    ObservedInstance {
        record: build_instance_record(
            workcell_ref,
            &InstanceObservation {
                slug: "hermes".into(),
                executable: PathBuf::from(&executable_path),
                executable_sha256: executable_sha.clone(),
                identity_material: format!("gateway:{}", default_gateway_socket().display()),
                pids: vec![],
                evidence_grade: EVIDENCE_GATEWAY_CONFIRMED.into(),
                seams,
            },
        ),
    }
}

pub fn default_gateway_socket() -> PathBuf {
    std::env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_default()
        .join(".aikit/state/gateway.sock")
}

/// One-line protocol ping at the ai-kit Agency Gateway well-known endpoint.
/// `{"type":"protocol"}\n` in, one envelope line out, `ok` wins. Any failure
/// is a plain `false` — the caller downgrades, never errors.
#[cfg(unix)]
pub fn gateway_answering(socket: &Path) -> bool {
    if !socket.exists() {
        return false;
    }
    use std::os::unix::net::UnixStream;
    let Ok(mut stream) = UnixStream::connect(socket) else {
        return false;
    };
    let _ = stream.set_read_timeout(Some(Duration::from_secs(2)));
    if stream.write_all(b"{\"type\":\"protocol\"}\n").is_err() {
        return false;
    }
    let mut reader = BufReader::new(stream);
    let mut line = String::new();
    if reader.read_line(&mut line).is_err() {
        return false;
    }
    line.parse::<Value>()
        .ok()
        .and_then(|envelope| envelope.get("ok").and_then(Value::as_bool))
        .unwrap_or(false)
}

#[cfg(not(unix))]
pub fn gateway_answering(_socket: &Path) -> bool {
    false
}

fn run_actuation_detection(command: &str) -> std::result::Result<Value, String> {
    let output = Command::new(command)
        .args(["harness", "detect", "--json"])
        .stdin(Stdio::null())
        .output()
        .map_err(|error| format!("run `{command} harness detect --json`: {error}"))?;
    if !output.status.success() {
        return Err(format!(
            "`{command} harness detect --json` exited {}",
            output.status
        ));
    }
    serde_json::from_slice(&output.stdout)
        .map_err(|error| format!("parse `{command} harness detect --json` output: {error}"))
}

/// Read the live pid table as (pid, executable basename) pairs.
pub fn read_pid_table() -> std::result::Result<Vec<(u32, String)>, String> {
    let output = Command::new("ps")
        .args(["-axo", "pid=,comm="])
        .stdin(Stdio::null())
        .output()
        .map_err(|error| format!("run `ps -axo pid=,comm=`: {error}"))?;
    if !output.status.success() {
        return Err(format!("`ps -axo pid=,comm=` exited {}", output.status));
    }
    let mut processes = Vec::new();
    for line in String::from_utf8_lossy(&output.stdout).lines() {
        let mut parts = line.split_whitespace();
        let Some(pid) = parts.next().and_then(|value| value.parse::<u32>().ok()) else {
            continue;
        };
        let comm = parts
            .next()
            .map(|value| {
                Path::new(value)
                    .file_name()
                    .and_then(|name| name.to_str())
                    .unwrap_or(value)
                    .to_owned()
            })
            .unwrap_or_default();
        processes.push((pid, comm));
    }
    Ok(processes)
}

/// Minimal shape validation of the Actuation detection document. Full
/// validation stays with Actuation's contract tests; here we only refuse
/// documents that cannot drive intake.
fn validate_detection_document(document: &Value) -> Result<()> {
    if document.get("schema").and_then(Value::as_str) != Some("actuation.harness-detection/v1") {
        return Err(WorkcellError::OperationFailed(
            "detection document schema must be actuation.harness-detection/v1".into(),
        ));
    }
    if document
        .get("harnesses")
        .and_then(Value::as_array)
        .is_none()
    {
        return Err(WorkcellError::OperationFailed(
            "detection document lacks a harnesses array".into(),
        ));
    }
    Ok(())
}

/// Render a report as a JSON value (the `--json` CLI shape).
pub fn report_json(report: &ScanReport) -> Value {
    json!({
        "ok": report.status == "ok",
        "status": report.status,
        "reason": report.reason,
        "workcell_ref": report.workcell_ref,
        "live": report.live.len(),
        "stale": report.stale.len(),
        "transitions": {
            "registered": report.transitions.registered,
            "refreshed": report.transitions.refreshed,
            "revived": report.transitions.revived,
            "went_stale": report.transitions.went_stale,
            "still_stale": report.transitions.still_stale,
        },
        "unmatched_processes": report.unmatched_processes,
        "instances": report.live.iter().chain(report.stale.iter()).cloned().collect::<Vec<_>>(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::instance_registry::{InstanceRegistry, LIVENESS_STALE};
    use epilogos_workcell_core::WorkcellRef;
    use std::fs;

    fn temp_root(name: &str) -> PathBuf {
        let root =
            std::env::temp_dir().join(format!("workcell-scan-test-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).unwrap();
        root
    }

    fn detection(slugs: &[(&str, bool)]) -> Value {
        json!({
            "schema": "actuation.harness-detection/v1",
            "document": "detection",
            "catalog_revision": 4,
            "harnesses": slugs.iter().map(|(slug, detected)| json!({
                "slug": slug,
                "harness_ref": format!("harness/{slug}"),
                "state": if *detected { "detected" } else { "not-installed" },
                "receipts": if *detected {
                    json!({
                        "executable": format!("/usr/local/bin/{slug}"),
                        "sha256": "ab".repeat(32),
                    })
                } else { json!({}) },
                "facets": [json!({"kind": "skills",
                    "path": format!("~/.{slug}/skills"), "exists": true, "count": 3})],
            })).collect::<Vec<_>>(),
        })
    }

    fn scan(registry: InstanceRegistry, inputs: ScanInputs) -> ScanReport {
        reconcile(
            registry,
            &WorkcellRef::new("workcell:local").unwrap(),
            inputs,
        )
    }

    fn inputs(
        detection: Value,
        processes: Vec<(u32, String)>,
        gateway_answering: bool,
    ) -> ScanInputs {
        ScanInputs {
            detection,
            processes,
            gateway_answering,
        }
    }

    #[test]
    fn failed_detection_run_is_unavailable_and_changes_nothing() {
        let root = temp_root("unavailable");
        let registry = InstanceRegistry::new(&root, WorkcellRef::new("workcell:local").unwrap());
        registry
            .register(crate::instance_registry::build_instance_record(
                &WorkcellRef::new("workcell:local").unwrap(),
                &InstanceObservation {
                    slug: "hermes".into(),
                    executable: PathBuf::from("/usr/local/bin/hermes"),
                    executable_sha256: "ab".repeat(32),
                    identity_material: "/usr/local/bin/hermes".into(),
                    pids: vec![1],
                    evidence_grade: EVIDENCE_LIVE_PID.into(),
                    seams: vec![],
                },
            ))
            .unwrap();

        let report = scan_live(
            &root,
            &WorkcellRef::new("workcell:local").unwrap(),
            "definitely-not-a-command",
        );
        assert_eq!(report.status, "unavailable");
        assert!(report.reason.unwrap().contains("definitely-not-a-command"));
        // Intake law: the registry is untouched by a failed run.
        let records = registry.list().unwrap();
        assert_eq!(records.len(), 1);
        assert_eq!(records[0]["liveness"], "live");
    }

    #[test]
    fn detected_with_pid_registers_live() {
        let root = temp_root("live");
        let registry = InstanceRegistry::new(&root, WorkcellRef::new("workcell:local").unwrap());
        let report = scan(
            registry.clone(),
            ScanInputs {
                detection: detection(&[("hermes", true)]),
                processes: vec![(4242, "hermes".into())],
                gateway_answering: false,
            },
        );
        assert_eq!(report.status, "ok");
        assert_eq!(report.transitions.registered, 1);
        let records = registry.list().unwrap();
        assert_eq!(records.len(), 1);
        assert_eq!(records[0]["evidence_grade"], EVIDENCE_LIVE_PID);
        assert_eq!(records[0]["pids"], json!([4242]));
        assert_eq!(records[0]["liveness"], "live");
        assert_eq!(records[0]["seams"][0]["kind"], "skills");
    }

    #[test]
    fn installed_without_pid_is_not_an_instance() {
        let root = temp_root("installed");
        let registry = InstanceRegistry::new(&root, WorkcellRef::new("workcell:local").unwrap());
        let report = scan(
            registry.clone(),
            ScanInputs {
                detection: detection(&[("claude-code", true), ("codex", false)]),
                processes: vec![],
                gateway_answering: false,
            },
        );
        assert_eq!(report.status, "ok");
        assert_eq!(report.transitions.registered, 0);
        assert_eq!(registry.list().unwrap().len(), 0);
    }

    #[test]
    fn liveness_transitions_after_two_missed_scans_and_revives() {
        let root = temp_root("liveness");
        let registry = InstanceRegistry::new(&root, WorkcellRef::new("workcell:local").unwrap());
        let first = scan(
            registry.clone(),
            inputs(
                detection(&[("hermes", true)]),
                vec![(7, "hermes".into())],
                false,
            ),
        );
        assert_eq!(first.transitions.registered, 1);

        let second = scan(
            registry.clone(),
            inputs(detection(&[("hermes", true)]), vec![], false),
        );
        assert_eq!(second.transitions.went_stale, 0, "one miss stays live");
        let records = registry.list().unwrap();
        assert_eq!(records[0]["liveness"], "live");
        assert_eq!(records[0]["consecutive_misses"], 1);

        let third = scan(
            registry.clone(),
            inputs(detection(&[("hermes", true)]), vec![], false),
        );
        assert_eq!(third.transitions.went_stale, 1, "two misses go stale");
        let records = registry.list().unwrap();
        assert_eq!(records[0]["liveness"], LIVENESS_STALE);
        // Stale is disclosed, never deleted.
        assert_eq!(registry.list().unwrap().len(), 1);

        let fourth = scan(
            registry.clone(),
            inputs(
                detection(&[("hermes", true)]),
                vec![(9, "hermes".into())],
                false,
            ),
        );
        assert_eq!(fourth.transitions.revived, 1);
        let records = registry.list().unwrap();
        assert_eq!(records[0]["liveness"], "live");
        assert_eq!(records[0]["consecutive_misses"], 0);
        assert_eq!(records[0]["pids"], json!([9]));
    }

    #[test]
    fn invalid_detection_document_is_unavailable() {
        let root = temp_root("invalid");
        let registry = InstanceRegistry::new(&root, WorkcellRef::new("workcell:local").unwrap());
        let report = scan(
            registry,
            ScanInputs {
                detection: json!({"schema": "something-else"}),
                processes: vec![],
                gateway_answering: false,
            },
        );
        assert_eq!(report.status, "unavailable");
        assert!(report
            .reason
            .unwrap()
            .contains("invalid detection document"));
    }

    #[test]
    fn unknown_alias_pids_are_named_unmatched() {
        let root = temp_root("unmatched");
        let registry = InstanceRegistry::new(&root, WorkcellRef::new("workcell:local").unwrap());
        let report = scan(
            registry,
            ScanInputs {
                detection: detection(&[]),
                processes: vec![(100, "mystery".into()), (200, "claude".into())],
                gateway_answering: false,
            },
        );
        assert_eq!(report.status, "ok");
        // `claude` is a known alias but its harness was not detected, so the
        // pid cannot be tied to a receipt; `mystery` is not even an alias.
        assert_eq!(report.unmatched_processes, vec!["claude".to_string()]);
    }

    #[test]
    fn report_json_carries_the_transitions() {
        let root = temp_root("report");
        let registry = InstanceRegistry::new(&root, WorkcellRef::new("workcell:local").unwrap());
        let report = scan(
            registry,
            ScanInputs {
                detection: detection(&[("hermes", true)]),
                processes: vec![(1, "hermes".into())],
                gateway_answering: false,
            },
        );
        let value = report_json(&report);
        assert_eq!(value["ok"], true);
        assert_eq!(value["status"], "ok");
        assert_eq!(value["transitions"]["registered"], 1);
        assert_eq!(value["live"], 1);
        assert_eq!(value["stale"], 0);
    }

    #[test]
    fn gateway_answer_detects_hermes_the_pid_table_cannot_name() {
        // Symmetry law: Hermes-with-gateway is detected through its carrier
        // answer even when the runtime comm (python/node) carries no alias.
        let root = temp_root("gateway-seam");
        let registry = InstanceRegistry::new(&root, WorkcellRef::new("workcell:local").unwrap());
        let report = scan(
            registry.clone(),
            inputs(
                detection(&[("hermes", true), ("claude-code", true)]),
                vec![(500, "python3.11".into()), (600, "claude".into())],
                true,
            ),
        );
        assert_eq!(report.status, "ok");
        assert_eq!(report.transitions.registered, 2);
        let records = registry.list().unwrap();
        let hermes = records
            .iter()
            .find(|record| record["harness_ref"] == "harness/hermes")
            .expect("hermes registered via the gateway seam");
        assert_eq!(hermes["evidence_grade"], EVIDENCE_GATEWAY_CONFIRMED);
        assert_eq!(hermes["liveness"], "live");
        assert!(hermes["pids"].as_array().unwrap().is_empty());
        let claude = records
            .iter()
            .find(|record| record["harness_ref"] == "harness/claude-code")
            .expect("claude registered via pid as usual");
        assert_eq!(claude["evidence_grade"], EVIDENCE_LIVE_PID);
        assert_eq!(claude["pids"], json!([600]));
    }

    #[test]
    fn multi_execution_groups_into_one_identity() {
        // Multi-execution is the default paradigm: N simultaneous processes
        // of one executable are one contract identity with N named pids.
        let root = temp_root("multi-exec");
        let registry = InstanceRegistry::new(&root, WorkcellRef::new("workcell:local").unwrap());
        let report = scan(
            registry.clone(),
            inputs(
                detection(&[("claude-code", true)]),
                vec![
                    (11, "claude".into()),
                    (12, "claude".into()),
                    (13, "claude".into()),
                ],
                false,
            ),
        );
        assert_eq!(report.status, "ok");
        assert_eq!(report.transitions.registered, 1);
        let records = registry.list().unwrap();
        assert_eq!(records.len(), 1);
        assert_eq!(records[0]["pids"], json!([11, 12, 13]));
    }

    #[test]
    fn no_gateway_answer_no_gateway_seam_instance() {
        let root = temp_root("no-gateway");
        let registry = InstanceRegistry::new(&root, WorkcellRef::new("workcell:local").unwrap());
        let report = scan(
            registry.clone(),
            inputs(
                detection(&[("hermes", true)]),
                vec![(500, "python3.11".into())],
                false,
            ),
        );
        assert_eq!(report.status, "ok");
        assert_eq!(registry.list().unwrap().len(), 0);
    }
}
