//! Real-process generation evidence (lane W, Workcell #72).
//!
//! The TM02 repair law: per-execution correlation inside the actual
//! host scope, meaningful deterministic start-marker evidence, and real
//! process replacement — never a continuity claim from a pid number alone.
//! These tests spawn actual `sleep` children and drive the same native
//! scan/observe operations the CLI exposes; the start marker comes from the
//! host pid table (`ps lstart`), exactly what `instances scan` records.

use std::{
    fs,
    path::PathBuf,
    process::{Child, Command, Stdio},
    thread::sleep,
    time::Duration,
};

use epilogos_workcell_core::WorkcellRef;
use epilogos_workcell_runtime::{
    build_instance_record, observe_resource_usage, read_pid_table, InstanceObservation,
    InstanceRegistry, ProcessExecution, EVIDENCE_LIVE_PID,
};

fn temp_root(label: &str) -> PathBuf {
    let root = std::env::temp_dir().join(format!(
        "workcell-instance-generation-{label}-{}",
        std::process::id()
    ));
    let _ = fs::remove_dir_all(&root);
    fs::create_dir_all(&root).unwrap();
    root
}

fn spawn_sleeper() -> Child {
    // Spawned by absolute path so the host's `ps comm` observation is the
    // same executable identity the probe record declares (/bin/sleep).
    Command::new("/bin/sleep")
        .arg("31")
        .stdin(Stdio::null())
        .spawn()
        .expect("spawn sleep child")
}

/// The pid-table start marker the scanner records for one pid.
fn marker_of(pid: u32) -> String {
    read_pid_table()
        .expect("read the host pid table")
        .into_iter()
        .find(|process| process.pid == pid)
        .map(|process| process.start_marker)
        .unwrap_or_else(|| panic!("pid {pid} disappeared from the host pid table"))
}

/// Register one live child as a harness instance bound to `start_marker`,
/// the same shape a completed `instances scan` writes.
fn register_child(
    root: &PathBuf,
    child_pid: u32,
    start_marker: &str,
) -> (InstanceRegistry, String) {
    let workcell_ref = WorkcellRef::new("workcell:local").unwrap();
    let registry = InstanceRegistry::new(root, workcell_ref.clone());
    let record = build_instance_record(
        &workcell_ref,
        &InstanceObservation {
            slug: "sleep-probe".into(),
            executable: PathBuf::from("/bin/sleep"),
            // The registry contract carries the executable receipt; the
            // probe does not depend on its digest, only on the live path
            // identity and the start marker under test.
            executable_sha256: "manual-registration".into(),
            identity_material: "/bin/sleep".into(),
            pids: vec![child_pid],
            executions: vec![ProcessExecution {
                pid: child_pid,
                process_start_marker: start_marker.to_owned(),
            }],
            evidence_grade: EVIDENCE_LIVE_PID.into(),
            seams: vec![],
        },
    );
    let reference = record["instance_ref"].as_str().unwrap().to_owned();
    registry.register(record).unwrap();
    (registry, reference)
}

#[test]
fn real_process_replacement_is_named_and_refused_not_merged() {
    let root = temp_root("replacement");

    // Two simultaneous executions of one executable, started over a second
    // apart so their `lstart` seconds cannot coincide: distinct pids and
    // distinct start markers — per-execution evidence, not one collapsed
    // count.
    let mut first = spawn_sleeper();
    sleep(Duration::from_millis(1100));
    let mut second = spawn_sleeper();
    let first_marker = marker_of(first.id());
    let second_marker = marker_of(second.id());
    assert!(
        !first_marker.is_empty() && !second_marker.is_empty(),
        "the host supplied start markers for both children"
    );
    assert_ne!(
        first_marker, second_marker,
        "two distinct executions carry distinct start evidence"
    );

    // A scan-shaped record observes its own process: the recorded marker
    // matches the live sample, so the bounded usage observation proceeds.
    let (registry, reference) = register_child(&root, second.id(), &second_marker);
    let report = observe_resource_usage(
        &registry,
        &reference,
        Some(second.id()),
        Duration::from_millis(5),
        vec!["opaque:generation-test".into()],
    )
    .unwrap()
    .as_json();
    assert_eq!(
        report["material_binding"]["process_start_marker"],
        second_marker.as_str()
    );

    // The first child dies and a new process is born later: the replacement
    // generation carries a different start marker (start evidence, not pid
    // luck — the markers are for different birth seconds).
    first.kill().expect("kill the first child");
    first.wait().expect("reap the first child");
    sleep(Duration::from_millis(1100));
    let mut third = spawn_sleeper();
    let third_marker = marker_of(third.id());
    assert_ne!(
        third_marker, first_marker,
        "the generation born after the death carries different start evidence"
    );

    // Binding the new pid to the dead generation's recorded marker is a
    // stale binding: the usage observation is refused by name instead of
    // being merged onto a replaced process generation.
    let (stale_registry, stale_reference) = register_child(&root, third.id(), &first_marker);
    let error = observe_resource_usage(
        &stale_registry,
        &stale_reference,
        Some(third.id()),
        Duration::from_millis(5),
        vec![],
    )
    .unwrap_err()
    .to_string();
    assert!(error.contains("stale binding"), "{error}");
    assert!(error.contains(&first_marker), "{error}");

    // The same pid with its own, true marker observes cleanly.
    let (fresh_registry, fresh_reference) = register_child(&root, third.id(), &third_marker);
    observe_resource_usage(
        &fresh_registry,
        &fresh_reference,
        Some(third.id()),
        Duration::from_millis(5),
        vec![],
    )
    .unwrap_or_else(|error| panic!("the true generation marker must observe: {error}"));

    second.kill().expect("kill the second child");
    second.wait().expect("reap the second child");
    third.kill().expect("kill the third child");
    third.wait().expect("reap the third child");
}
