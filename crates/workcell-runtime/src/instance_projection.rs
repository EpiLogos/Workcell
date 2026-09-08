//! Projection: candidate predicate, re-placement, re-detection (plan §4).
//!
//! A registered local workcell becomes a **projection candidate** when its
//! registry holds ≥1 detected instance with `evidence_grade ≥ live-pid` and
//! the workcell binding is adopted in Central (`Control/machines/current.json`).
//! The Central half of the predicate belongs to Central (it owns the binding
//! file); this module carries the registry half and the re-placement law.
//!
//! Re-placement invariance (the M4 acceptance criterion): identity is held
//! by contract, not host. A projected instance must re-detect on the target
//! workcell with the same `harness_ref` and the same `instance_ref` identity
//! hash, differing only in `workcell_ref`, `pids`, and `observed_at`.
//! Re-detection on target is the same scanner reconciliation run against the
//! target's state root — success is `detected` with matching identity; a
//! failed run is `unavailable {reason}`, never "projected but unverified",
//! and writes nothing.

use std::path::PathBuf;

use epilogos_workcell_core::{Result, WorkcellError, WorkcellRef};
use serde_json::{json, Value};

use crate::instance_registry::{
    harness_slug, validate_instance_record, InstanceRegistry, EVIDENCE_DECLARED_UNVERIFIED,
    EVIDENCE_GATEWAY_CONFIRMED, EVIDENCE_LIVE_PID, LIVENESS_LIVE,
};
use crate::instance_scan::{reconcile, scan_inputs_live, ScanInputs, ScanReport};

/// §4: the registry half of the projection-candidate predicate — every
/// detected instance with `evidence_grade ≥ live-pid`. `declared-unverified`
/// is conformance intent, not detection, so a prespecified declaration never
/// makes a workcell a candidate. The caller (oi/Central) additionally checks
/// the Central machine binding; this function does not read Central's file.
pub fn projection_candidates(records: &[Value]) -> Vec<Value> {
    records
        .iter()
        .filter(|record| {
            let grade = record.get("evidence_grade").and_then(Value::as_str);
            matches!(
                grade,
                Some(EVIDENCE_LIVE_PID) | Some(EVIDENCE_GATEWAY_CONFIRMED)
            )
        })
        .cloned()
        .collect()
}

/// §4 predicate over a listing: at least one detected instance.
pub fn is_projection_candidate(records: &[Value]) -> bool {
    !projection_candidates(records).is_empty()
}

/// Re-place a record onto a target workcell: identity held by contract, not
/// host. `instance_ref`, `harness_ref`, `executable`, and `seams` are
/// preserved byte-for-byte; only the host-bound observation fields are
/// re-written (`workcell_ref` → target, `pids` emptied, `observed_at`
/// marked pending). The projected value is an *expectation*, never written
/// to any registry — only a completed re-detection scan on the target may
/// land a record there (intake law).
///
/// Only detected instances project: a `declared-unverified` record is a
/// named demand error, not a silently degraded projection.
pub fn project_record(record: &Value, target: &WorkcellRef) -> Result<Value> {
    validate_instance_record(record)?;
    if record.get("evidence_grade").and_then(Value::as_str) == Some(EVIDENCE_DECLARED_UNVERIFIED) {
        return Err(WorkcellError::InvalidDemand(
            "only detected instances (evidence_grade live-pid or stronger) project; \
             a declared-unverified record is conformance intent, not an instance"
                .into(),
        ));
    }
    let mut projected = record.clone();
    projected["workcell_ref"] = target.to_string().into();
    projected["pids"] = json!([]);
    projected["observed_at"] = "pending-redetection".into();
    Ok(projected)
}

/// One projection attempt. `status` is one of:
/// - `ok` — the target scan completed and re-detected the projected identity
///   live; `redetected` holds the target-side record.
/// - `unavailable` — the target scan failed to run; named reason, and the
///   target registry is untouched (never an empty set read as absence).
/// - `unmatched` — the target scan completed but the projected identity did
///   not re-detect there; named, never auto-resolved.
#[derive(Debug, Clone)]
pub struct ProjectionReport {
    pub status: &'static str,
    pub reason: Option<String>,
    pub source_workcell_ref: String,
    pub target_workcell_ref: String,
    /// The identity re-placement requires to hold on the target.
    pub expected_instance_ref: String,
    /// The re-placed expectation (identity preserved, host fields pending).
    pub expected: Value,
    /// The re-detected record on the target, when status is `ok`.
    pub redetected: Option<Value>,
    /// The target-side scan report, for transitions and named conflicts.
    pub scan: ScanReport,
}

impl ProjectionReport {
    fn unavailable(
        source_ref: &WorkcellRef,
        target: &WorkcellRef,
        expected: Value,
        expected_instance_ref: String,
        reason: String,
    ) -> Self {
        Self {
            status: "unavailable",
            reason: Some(reason),
            source_workcell_ref: source_ref.to_string(),
            target_workcell_ref: target.to_string(),
            expected_instance_ref,
            expected,
            redetected: None,
            scan: ScanReport::unavailable_for(target),
        }
    }
}

/// Project one registered instance onto a target workcell and re-detect it
/// there with the target's own scan inputs.
///
/// Demand errors (unknown `source_ref`, undeclared/unverifiable source) are
/// returned as `Err`; everything the host does is disclosed in the report.
pub fn project_instance(
    source: &InstanceRegistry,
    source_ref: &str,
    target_state_root: impl Into<PathBuf>,
    target: &WorkcellRef,
    inputs: ScanInputs,
) -> Result<ProjectionReport> {
    let record = source.show(source_ref)?;
    let expected = project_record(&record, target)?;
    let expected_instance_ref = expected["instance_ref"]
        .as_str()
        .unwrap_or_default()
        .to_owned();
    let source_workcell_ref = record["workcell_ref"]
        .as_str()
        .unwrap_or("unknown")
        .to_owned();
    let source_ref = WorkcellRef::new(&source_workcell_ref)
        .unwrap_or_else(|_| WorkcellRef::new("workcell:unknown").expect("static ref parses"));

    let target_registry = InstanceRegistry::new(target_state_root, target.clone());
    let scan = reconcile(target_registry.clone(), target, inputs);
    if scan.status != "ok" {
        return Ok(ProjectionReport::unavailable(
            &source_ref,
            target,
            expected,
            expected_instance_ref,
            scan.reason.unwrap_or_else(|| "scan failed".to_owned()),
        ));
    }

    match target_registry.show(&expected_instance_ref) {
        Ok(redetected)
            if redetected.get("liveness").and_then(Value::as_str) == Some(LIVENESS_LIVE) =>
        {
            Ok(ProjectionReport {
                status: "ok",
                reason: None,
                source_workcell_ref: source_ref.to_string(),
                target_workcell_ref: target.to_string(),
                expected_instance_ref,
                expected,
                redetected: Some(redetected),
                scan,
            })
        }
        Ok(other) => {
            let liveness = other
                .get("liveness")
                .and_then(Value::as_str)
                .unwrap_or("unknown");
            let reason = format!(
                "identity `{expected_instance_ref}` holds on {target} but was not re-observed \
                 live this scan (liveness {liveness})"
            );
            Ok(unmatched_report(
                &source_ref,
                target,
                expected,
                expected_instance_ref,
                &target_registry,
                scan,
                reason,
            ))
        }
        Err(_) => {
            let reason = format!(
                "projected identity `{expected_instance_ref}` did not re-detect on {target}"
            );
            Ok(unmatched_report(
                &source_ref,
                target,
                expected,
                expected_instance_ref,
                &target_registry,
                scan,
                reason,
            ))
        }
    }
}

/// Live wrapper: gather the target's scan inputs from the host, then project.
/// A gather failure is `unavailable` with the named reason — never an empty
/// set, never a write.
pub fn project_instance_live(
    source: &InstanceRegistry,
    source_ref: &str,
    target_state_root: impl Into<PathBuf>,
    target: &WorkcellRef,
    actuation_command: &str,
) -> Result<ProjectionReport> {
    let record = source.show(source_ref)?;
    let expected = project_record(&record, target)?;
    let expected_instance_ref = expected["instance_ref"]
        .as_str()
        .unwrap_or_default()
        .to_owned();
    let source_workcell = {
        let source_workcell_ref = record["workcell_ref"]
            .as_str()
            .unwrap_or("unknown")
            .to_owned();
        WorkcellRef::new(&source_workcell_ref)
            .unwrap_or_else(|_| WorkcellRef::new("workcell:unknown").expect("static ref parses"))
    };

    let inputs = match scan_inputs_live(actuation_command) {
        Ok(inputs) => inputs,
        Err(reason) => {
            return Ok(ProjectionReport::unavailable(
                &source_workcell,
                target,
                expected,
                expected_instance_ref,
                reason,
            ))
        }
    };
    project_instance(source, source_ref, target_state_root, target, inputs)
}

fn unmatched_report(
    source_ref: &WorkcellRef,
    target: &WorkcellRef,
    expected: Value,
    expected_instance_ref: String,
    target_registry: &InstanceRegistry,
    scan: ScanReport,
    mut reason: String,
) -> ProjectionReport {
    // Name any same-slug differing identity on the target: a conflict is a
    // finding for the owner, never auto-resolved.
    if let Ok(slug) = harness_slug(&expected) {
        let held: Vec<String> = target_registry
            .list()
            .unwrap_or_default()
            .iter()
            .filter(|record| harness_slug(record).map(|s| s == slug).unwrap_or(false))
            .filter_map(|record| {
                record["instance_ref"]
                    .as_str()
                    .filter(|reference| *reference != expected_instance_ref)
                    .map(str::to_owned)
            })
            .collect();
        for reference in held {
            reason.push_str(&format!(
                "; target holds differing identity `{reference}` for harness/{slug} \
                 (named conflict, never auto-resolved)"
            ));
        }
    }
    ProjectionReport {
        status: "unmatched",
        reason: Some(reason),
        source_workcell_ref: source_ref.to_string(),
        target_workcell_ref: target.to_string(),
        expected_instance_ref,
        expected,
        redetected: None,
        scan,
    }
}

/// Render a projection report as a JSON value (the `--json` CLI shape).
pub fn projection_report_json(report: &ProjectionReport) -> Value {
    json!({
        "ok": report.status == "ok",
        "status": report.status,
        "reason": report.reason,
        "source_workcell_ref": report.source_workcell_ref,
        "target_workcell_ref": report.target_workcell_ref,
        "expected_instance_ref": report.expected_instance_ref,
        "expected": report.expected,
        "redetected": report.redetected,
        "scan": crate::instance_scan::report_json(&report.scan),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::instance_registry::{build_instance_record, InstanceObservation};
    use crate::instance_scan::{self, ScanTransitions};
    use std::fs;
    use std::path::{Path, PathBuf};

    const SHA: &str = "ababcdababcdababcdababcdababcdab"; // 32 bytes, hex-ish

    fn temp_root(name: &str) -> PathBuf {
        let root = std::env::temp_dir().join(format!(
            "workcell-projection-test-{name}-{}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).unwrap();
        root
    }

    /// Detection document naming one harness at a fixed receipt.
    fn detection(slug: &str, sha: &str) -> Value {
        json!({
            "schema": "actuation.harness-detection/v1",
            "harnesses": [{
                "slug": slug,
                "harness_ref": format!("harness/{slug}"),
                "state": "detected",
                "receipts": {
                    "executable": format!("/usr/local/bin/{slug}"),
                    "sha256": sha,
                },
                "facets": [json!({"kind": "skills",
                    "path": format!("~/.{slug}/skills"), "exists": true, "count": 3})],
            }],
        })
    }

    fn scan_inputs(slug: &str, sha: &str, pid: u32) -> ScanInputs {
        ScanInputs {
            detection: detection(slug, sha),
            processes: vec![(pid, slug.to_owned())],
            gateway_answering: false,
        }
    }

    /// Register a live instance on a source workcell via its scanner, the
    /// same path a real registration takes.
    fn source_with_instance(
        root: &Path,
        workcell: &str,
        slug: &str,
        sha: &str,
        pid: u32,
    ) -> InstanceRegistry {
        let workcell_ref = WorkcellRef::new(workcell).unwrap();
        let registry = InstanceRegistry::new(root, workcell_ref.clone());
        let report =
            instance_scan::reconcile(registry.clone(), &workcell_ref, scan_inputs(slug, sha, pid));
        assert_eq!(report.status, "ok");
        assert_eq!(
            report.transitions,
            ScanTransitions {
                registered: 1,
                ..ScanTransitions::default()
            }
        );
        registry
    }

    #[test]
    fn candidate_predicate_counts_only_detected_instances() {
        let local = WorkcellRef::new("workcell:local").unwrap();
        let live = build_instance_record(
            &local,
            &InstanceObservation {
                slug: "hermes".into(),
                executable: PathBuf::from("/usr/local/bin/hermes"),
                executable_sha256: SHA.to_owned(),
                identity_material: "/usr/local/bin/hermes".into(),
                pids: vec![1],
                evidence_grade: EVIDENCE_LIVE_PID.into(),
                seams: vec![],
            },
        );
        let gateway = build_instance_record(
            &local,
            &InstanceObservation {
                slug: "codex".into(),
                executable: PathBuf::from("/usr/local/bin/codex"),
                executable_sha256: SHA.to_owned(),
                identity_material: "/usr/local/bin/codex".into(),
                pids: vec![],
                evidence_grade: EVIDENCE_GATEWAY_CONFIRMED.into(),
                seams: vec![],
            },
        );
        let declared = build_instance_record(
            &local,
            &InstanceObservation {
                slug: "pi".into(),
                executable: PathBuf::from("/usr/local/bin/pi"),
                executable_sha256: SHA.to_owned(),
                identity_material: "declared:pi".into(),
                pids: vec![],
                evidence_grade: EVIDENCE_DECLARED_UNVERIFIED.into(),
                seams: vec![],
            },
        );
        let records = vec![live, gateway, declared.clone()];
        assert!(is_projection_candidate(&records));
        let candidates = projection_candidates(&records);
        assert_eq!(candidates.len(), 2, "declared-unverified is not detection");
        assert!(!candidates
            .iter()
            .any(|record| record["evidence_grade"] == EVIDENCE_DECLARED_UNVERIFIED));
        assert!(!is_projection_candidate(&[declared]));
    }

    #[test]
    fn replacement_invariance_record_roundtrips_across_two_state_roots() {
        // The M4 acceptance test: an instance record moved to a non-local
        // workcell re-detects there with the same harness_ref and the same
        // instance_ref identity hash, differing only in workcell_ref, pids,
        // and observed_at.
        let source_root = temp_root("source");
        let target_root = temp_root("target");
        let target = WorkcellRef::new("workcell:target").unwrap();

        let source = source_with_instance(&source_root, "workcell:local", "hermes", SHA, 101);
        let source_record = source.list().unwrap().remove(0);
        let source_ref = source_record["instance_ref"].as_str().unwrap().to_owned();

        let report = project_instance(
            &source,
            &source_ref,
            &target_root,
            &target,
            scan_inputs("hermes", SHA, 202),
        )
        .unwrap();
        assert_eq!(report.status, "ok", "reason: {:?}", report.reason);
        assert_eq!(
            report.expected_instance_ref, source_ref,
            "identity hash is host-invariant"
        );

        let redetected = report
            .redetected
            .expect("ok carries the re-detected record");
        assert_eq!(redetected["instance_ref"], source_record["instance_ref"]);
        assert_eq!(redetected["harness_ref"], source_record["harness_ref"]);
        assert_eq!(redetected["executable"], source_record["executable"]);
        assert_eq!(redetected["seams"], source_record["seams"]);
        assert_eq!(
            redetected["evidence_grade"],
            source_record["evidence_grade"]
        );
        assert_eq!(redetected["liveness"], LIVENESS_LIVE);
        // Host-bound observation fields differ — and nothing else may.
        assert_eq!(redetected["workcell_ref"], "workcell:target");
        assert_eq!(redetected["pids"], json!([202]));
        assert!(redetected["observed_at"]
            .as_str()
            .unwrap()
            .starts_with("unix:"));
        let drift: Vec<&str> = redetected
            .as_object()
            .unwrap()
            .keys()
            .filter(|field| redetected[*field] != source_record[*field])
            .map(String::as_str)
            .collect();
        for field in &drift {
            assert!(
                matches!(*field, "workcell_ref" | "pids" | "observed_at"),
                "re-placement invariance: `{field}` must not drift"
            );
        }
        assert!(
            drift.contains(&"workcell_ref"),
            "the host binding must change"
        );
        assert!(
            drift.contains(&"pids"),
            "the pid observation must be the target's own"
        );

        // The target registry now holds the record; the source is untouched.
        let target_registry = InstanceRegistry::new(&target_root, target);
        assert_eq!(target_registry.list().unwrap().len(), 1);
        assert_eq!(
            target_registry.show(&source_ref).unwrap()["pids"],
            json!([202])
        );
        assert_eq!(source.show(&source_ref).unwrap()["pids"], json!([101]));
    }

    #[test]
    fn projected_identity_repeats_across_multiple_target_scans() {
        // Re-detection is not a one-shot: the identity holds on every
        // subsequent target scan (pid changes, hash does not).
        let source_root = temp_root("repeat-source");
        let target_root = temp_root("repeat-target");
        let target = WorkcellRef::new("workcell:target").unwrap();
        let source = source_with_instance(&source_root, "workcell:local", "codex", SHA, 1);
        let source_ref = source.list().unwrap()[0]["instance_ref"]
            .as_str()
            .unwrap()
            .to_owned();

        let target_registry = InstanceRegistry::new(&target_root, target.clone());
        for pid in [10u32, 20, 30] {
            let report = project_instance(
                &source,
                &source_ref,
                &target_root,
                &target,
                scan_inputs("codex", SHA, pid),
            )
            .unwrap();
            assert_eq!(report.status, "ok");
            assert_eq!(report.redetected.unwrap()["pids"], json!([pid]));
        }
        let records = target_registry.list().unwrap();
        assert_eq!(records.len(), 1, "one identity, refreshed in place");
        assert_eq!(records[0]["instance_ref"], source_ref);
    }

    #[test]
    fn failed_target_scan_is_unavailable_and_writes_nothing() {
        // Intake law on the target: a failed run is unavailable, never an
        // empty set read as absence — and never "projected but unverified".
        let source_root = temp_root("unavailable-source");
        let target_root = temp_root("unavailable-target");
        let target = WorkcellRef::new("workcell:target").unwrap();
        let source = source_with_instance(&source_root, "workcell:local", "hermes", SHA, 5);
        let source_ref = source.list().unwrap()[0]["instance_ref"]
            .as_str()
            .unwrap()
            .to_owned();

        let report = project_instance(
            &source,
            &source_ref,
            &target_root,
            &target,
            ScanInputs {
                detection: json!({"schema": "something-else"}),
                processes: vec![],
                gateway_answering: false,
            },
        )
        .unwrap();
        assert_eq!(report.status, "unavailable");
        assert!(report
            .reason
            .unwrap()
            .contains("invalid detection document"));
        assert!(report.redetected.is_none());
        let target_registry = InstanceRegistry::new(&target_root, target);
        assert_eq!(
            target_registry.list().unwrap().len(),
            0,
            "a failed run writes nothing to the target"
        );
        // The source record is untouched by the failed projection.
        assert_eq!(source.show(&source_ref).unwrap()["pids"], json!([5]));
    }

    #[test]
    fn identity_absent_on_target_is_named_unmatched_with_conflict_evidence() {
        // The scan completed but the projected identity did not re-detect,
        // and the target already holds a differing identity for the slug:
        // named, with the conflict evidence, never auto-resolved.
        let source_root = temp_root("unmatched-source");
        let target_root = temp_root("unmatched-target");
        let target = WorkcellRef::new("workcell:target").unwrap();
        let source = source_with_instance(&source_root, "workcell:local", "hermes", SHA, 6);
        let source_ref = source.list().unwrap()[0]["instance_ref"]
            .as_str()
            .unwrap()
            .to_owned();

        // Target already runs a different hermes executable identity.
        source_with_instance(
            &target_root,
            "workcell:target",
            "hermes",
            &"cd".repeat(32),
            7,
        );

        let report = project_instance(
            &source,
            &source_ref,
            &target_root,
            &target,
            scan_inputs("hermes", &"cd".repeat(32), 7),
        )
        .unwrap();
        assert_eq!(report.status, "unmatched");
        let reason = report.reason.unwrap();
        assert!(reason.contains(&source_ref), "{reason}");
        assert!(reason.contains("differing identity"), "{reason}");
        assert!(reason.contains("never auto-resolved"), "{reason}");
        // The target keeps its own record; nothing was overwritten.
        let target_registry = InstanceRegistry::new(&target_root, target);
        assert_eq!(target_registry.list().unwrap().len(), 1);
        assert_ne!(
            target_registry.list().unwrap()[0]["instance_ref"],
            source_ref
        );
    }

    #[test]
    fn declared_unverified_source_is_a_named_demand_error() {
        let root = temp_root("declared-source");
        let local = WorkcellRef::new("workcell:local").unwrap();
        let registry = InstanceRegistry::new(&root, local.clone());
        registry.declare("pi", None, "declared:pi").unwrap();
        let declared_ref = registry.list().unwrap()[0]["instance_ref"]
            .as_str()
            .unwrap()
            .to_owned();
        let target = WorkcellRef::new("workcell:target").unwrap();
        let error = project_instance(
            &registry,
            &declared_ref,
            temp_root("declared-target"),
            &target,
            scan_inputs("pi", SHA, 1),
        )
        .unwrap_err()
        .to_string();
        assert!(error.contains("declared-unverified"), "{error}");
    }

    #[test]
    fn unknown_source_ref_is_a_named_demand_error() {
        let root = temp_root("unknown-source");
        let local = WorkcellRef::new("workcell:local").unwrap();
        let registry = InstanceRegistry::new(&root, local);
        let target = WorkcellRef::new("workcell:target").unwrap();
        let error = project_instance(
            &registry,
            "instance:ghost:0000",
            temp_root("unknown-target"),
            &target,
            scan_inputs("ghost", SHA, 1),
        )
        .unwrap_err()
        .to_string();
        assert!(error.contains("unknown instance"), "{error}");
    }

    #[test]
    fn projection_report_json_discloses_all_three_states() {
        let source_root = temp_root("json-source");
        let target_root = temp_root("json-target");
        let target = WorkcellRef::new("workcell:target").unwrap();
        let source = source_with_instance(&source_root, "workcell:local", "hermes", SHA, 8);
        let source_ref = source.list().unwrap()[0]["instance_ref"]
            .as_str()
            .unwrap()
            .to_owned();

        let ok = project_instance(
            &source,
            &source_ref,
            &target_root,
            &target,
            scan_inputs("hermes", SHA, 9),
        )
        .unwrap();
        let value = projection_report_json(&ok);
        assert_eq!(value["ok"], true);
        assert_eq!(value["status"], "ok");
        assert_eq!(value["expected_instance_ref"], source_ref);
        assert_eq!(value["redetected"]["pids"], json!([9]));
        assert_eq!(value["scan"]["status"], "ok");
    }
}
