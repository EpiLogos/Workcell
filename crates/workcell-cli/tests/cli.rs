use std::{
    fs,
    path::{Path, PathBuf},
    process::{Command, Output},
    time::{SystemTime, UNIX_EPOCH},
};

use serde_json::Value;

fn binary() -> &'static str {
    env!("CARGO_BIN_EXE_workcell")
}

fn temp_path(label: &str) -> PathBuf {
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    std::env::temp_dir().join(format!(
        "epilogos-workcell-cli-{label}-{}-{nonce}",
        std::process::id()
    ))
}

fn run(args: &[&str]) -> Output {
    Command::new(binary()).args(args).output().unwrap()
}

fn path_arg(path: &Path) -> &str {
    path.to_str().unwrap()
}

fn json_stdout(output: &Output) -> Value {
    serde_json::from_slice(&output.stdout).unwrap_or_else(|error| {
        panic!(
            "invalid JSON stdout: {error}\nstdout={}\nstderr={}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        )
    })
}

#[test]
fn status_and_discovery_are_agent_operable_json() {
    let state = temp_path("status");
    let status = run(&["--state-root", path_arg(&state), "--json", "status"]);
    assert!(
        status.status.success(),
        "{}",
        String::from_utf8_lossy(&status.stderr)
    );
    let status_json = json_stdout(&status);
    assert_eq!(status_json["ok"], true);
    assert_eq!(status_json["workcell_ref"], "workcell:local");
    assert!(status_json["providers"].as_u64().unwrap() >= 3);

    let discovery = run(&["--state-root", path_arg(&state), "--json", "discover"]);
    assert!(
        discovery.status.success(),
        "{}",
        String::from_utf8_lossy(&discovery.stderr)
    );
    let discovery_json = json_stdout(&discovery);
    assert_eq!(discovery_json["health"], "healthy");
    assert!(discovery_json["offers"]
        .as_array()
        .unwrap()
        .iter()
        .any(|offer| offer["affordances"]
            .as_array()
            .unwrap()
            .iter()
            .any(|value| value == "shell")));

    let _ = fs::remove_dir_all(state);
}

#[test]
fn preferred_isolation_degrades_but_required_isolation_fails_with_stable_exit_code() {
    let state = temp_path("degradation");
    let degraded = run(&[
        "--state-root",
        path_arg(&state),
        "--json",
        "plan",
        "--require",
        "shell",
        "--prefer",
        "microvm-snapshot",
    ]);
    assert!(degraded.status.success());
    let degraded_json = json_stdout(&degraded);
    assert_eq!(degraded_json["status"], "degraded");
    assert!(degraded_json["degradations"]
        .as_array()
        .unwrap()
        .iter()
        .any(|item| item["requirement"] == "affordance:microvm-snapshot"));

    let required = run(&[
        "--state-root",
        path_arg(&state),
        "--json",
        "plan",
        "--require",
        "microvm-snapshot",
    ]);
    assert_eq!(required.status.code(), Some(3));
    assert!(String::from_utf8_lossy(&required.stderr).contains("unsatisfied-demand"));

    let _ = fs::remove_dir_all(state);
}

#[test]
fn prepared_world_is_observed_and_released_by_fresh_cli_processes() {
    let source = temp_path("source");
    let state = temp_path("state");
    let receipt = state.join("receipt.json");
    fs::create_dir_all(&source).unwrap();
    fs::write(source.join("source.txt"), "portable\n").unwrap();

    let prepared = run(&[
        "--state-root",
        path_arg(&state),
        "--workspace-source",
        path_arg(&source),
        "--receipt",
        path_arg(&receipt),
        "--json",
        "prepare",
        "--require",
        "shell",
        "--workspace",
        "writable",
    ]);
    assert!(
        prepared.status.success(),
        "{}",
        String::from_utf8_lossy(&prepared.stderr)
    );
    assert!(receipt.exists());
    let receipt_json: Value = serde_json::from_str(&fs::read_to_string(&receipt).unwrap()).unwrap();
    let workspace_path = receipt_json["binding_graph"]["bindings"]
        .as_array()
        .unwrap()
        .iter()
        .find(|binding| binding["port"] == "workspace")
        .and_then(|binding| binding["properties"]["path"].as_str())
        .map(PathBuf::from)
        .unwrap();
    assert_eq!(
        fs::read_to_string(workspace_path.join("source.txt")).unwrap(),
        "portable\n"
    );

    let observed = run(&[
        "--state-root",
        path_arg(&state),
        "--receipt",
        path_arg(&receipt),
        "--json",
        "observe",
    ]);
    assert!(
        observed.status.success(),
        "{}",
        String::from_utf8_lossy(&observed.stderr)
    );
    let observed_json = json_stdout(&observed);
    assert!(observed_json["observations"]
        .as_array()
        .unwrap()
        .iter()
        .any(|observation| observation["logical_ref"] == "affordance:shell"));

    let released = run(&[
        "--state-root",
        path_arg(&state),
        "--receipt",
        path_arg(&receipt),
        "--json",
        "release",
    ]);
    assert!(
        released.status.success(),
        "{}",
        String::from_utf8_lossy(&released.stderr)
    );
    let released_json = json_stdout(&released);
    assert_eq!(released_json["disposition"], "released");
    assert!(!workspace_path.exists());

    let _ = fs::remove_dir_all(source);
    let _ = fs::remove_dir_all(state);
}

#[test]
fn material_reading_composes_receipt_observation_and_exposure_without_rebinding() {
    let state = temp_path("material-reading");
    let receipt = state.join("receipt.json");
    let prepared = run(&[
        "--state-root",
        path_arg(&state),
        "--receipt",
        path_arg(&receipt),
        "--json",
        "prepare",
        "--require",
        "shell",
    ]);
    assert!(prepared.status.success());
    let world_ref = json_stdout(&prepared)["world"]["world_ref"]
        .as_str()
        .unwrap()
        .to_owned();

    let material = run(&[
        "--state-root",
        path_arg(&state),
        "--receipt",
        path_arg(&receipt),
        "--json",
        "material",
    ]);
    assert!(
        material.status.success(),
        "{}",
        String::from_utf8_lossy(&material.stderr)
    );
    let reading = json_stdout(&material);
    assert_eq!(reading["contract"], "workcell.material-reading/v1");
    assert_eq!(reading["backend"], "native-cli");
    assert_eq!(reading["consistency"], "sequential-not-atomic");
    assert_eq!(reading["receipt_world"]["world_ref"], world_ref);
    assert_eq!(reading["observation"]["status"], "supplied");
    assert_eq!(reading["exposure"]["status"], "supplied");
    assert_eq!(reading["bodies"].as_array().unwrap().len(), 0);
    assert_eq!(
        reading["observation"]["reading"]["world_ref"],
        reading["exposure"]["reading"]["world_ref"]
    );

    let released = run(&[
        "--state-root",
        path_arg(&state),
        "--receipt",
        path_arg(&receipt),
        "--json",
        "release",
    ]);
    assert!(released.status.success());
    let after_release = run(&[
        "--state-root",
        path_arg(&state),
        "--receipt",
        path_arg(&receipt),
        "--json",
        "material",
    ]);
    assert!(after_release.status.success());
    let released_reading = json_stdout(&after_release);
    assert_eq!(released_reading["receipt_world"]["world_ref"], world_ref);
    assert_eq!(released_reading["observation"]["status"], "supplied");
    assert_eq!(
        released_reading["observation"]["reading"]["observations"][0]["state"],
        "unavailable"
    );
    assert_eq!(released_reading["exposure"]["status"], "supplied");
    assert_eq!(released_reading["bodies"].as_array().unwrap().len(), 0);

    let _ = fs::remove_dir_all(state);
}
