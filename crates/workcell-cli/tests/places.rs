use std::process::{Command, Output};

use serde_json::{json, Value};

fn binary() -> &'static str {
    env!("CARGO_BIN_EXE_workcell")
}

fn run(args: &[&str]) -> Output {
    Command::new(binary()).args(args).output().unwrap()
}

fn run_with_path(args: &[&str], path: &str) -> Output {
    Command::new(binary())
        .args(args)
        .env("PATH", path)
        .output()
        .unwrap()
}

/// A directory holding an `actuation` that answers `harness detect --json`
/// with `document`, as Actuation does. Only shell builtins run in it, so it
/// answers on any PATH.
#[cfg(unix)]
fn actuation_answering(label: &str, document: &Value) -> std::path::PathBuf {
    use std::os::unix::fs::PermissionsExt;
    let directory = std::env::temp_dir().join(format!(
        "epilogos-workcell-places-{label}-{}",
        std::process::id()
    ));
    std::fs::create_dir_all(&directory).unwrap();
    let script = directory.join("actuation");
    std::fs::write(&script, format!("#!/bin/sh\nprintf '%s\\n' '{document}'\n")).unwrap();
    std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();
    directory
}

/// A detection document in Actuation's shape: one harness row, not found on
/// this machine, whose probe names its executable.
#[cfg(unix)]
fn detection_document() -> Value {
    json!({
        "schema": "actuation.harness-detection/v1",
        "document": "detection",
        "harnesses": [{
            "slug": "claude-code",
            "harness_ref": "harness/claude-code",
            "native_kind": "harness",
            "state": "not-installed",
            "probes": [{"kind": "executable", "result": "pass", "spec": "claude",
                "detail": "not found on PATH"}],
        }],
        "absent": ["claude-code"],
        "availability": "complete",
    })
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

fn stdout_text(output: &Output) -> String {
    String::from_utf8_lossy(&output.stdout).into_owned()
}

fn stderr_text(output: &Output) -> String {
    String::from_utf8_lossy(&output.stderr).into_owned()
}

/// The place census runs read-only on any machine: with or without tmux or a
/// Herdr server, it exits successfully and discloses each provider's honest
/// state instead of failing.
#[cfg(unix)]
#[test]
fn place_census_cli_runs_read_only_and_discloses_provider_states() {
    let actuation = actuation_answering("detection", &detection_document());
    let path = format!(
        "{}:{}",
        actuation.display(),
        std::env::var("PATH").unwrap_or_default()
    );
    let output = run_with_path(&["--json", "places"], &path);
    let _ = std::fs::remove_dir_all(&actuation);
    assert!(output.status.success(), "{}", stderr_text(&output));
    let value = json_stdout(&output);
    assert_eq!(value["schema"], "workcell.place-census/v1");
    assert_eq!(value["ok"], true, "{value}");
    assert_eq!(value["status"], "ok");
    assert_eq!(value["machine"].as_str().map(str::is_empty), Some(false));

    let providers = value["providers"].as_array().expect("providers array");
    let names: Vec<&str> = providers
        .iter()
        .filter_map(|provider| provider["provider"].as_str())
        .collect();
    assert!(names.contains(&"tmux"), "tmux provider always reported");
    assert!(names.contains(&"herdr"), "herdr provider always reported");

    let known_statuses = [
        "ok",
        "absent",
        "present-no-server",
        "present-server-unreachable",
        "present-cli-unparsed",
        "error",
    ];
    for provider in providers {
        let status = provider["status"].as_str().expect("provider status");
        assert!(
            known_statuses.contains(&status),
            "status `{status}` must be one of the disclosed provider states"
        );
    }

    assert!(value["panes"].is_array());
    assert!(value["place_reuse_findings"].is_array());
    assert!(value["summary"]["panes"].is_u64());
}

/// Which panes run harnesses is Actuation's census. Without its detection
/// the census is unavailable and says why; it never classifies against a
/// list of its own.
#[test]
fn place_census_cli_without_actuation_is_unavailable_with_the_reason() {
    let output = run_with_path(&["--json", "places"], "/usr/bin:/bin");
    assert!(output.status.success(), "{}", stderr_text(&output));
    let value = json_stdout(&output);
    assert_eq!(value["schema"], "workcell.place-census/v1");
    assert_eq!(value["ok"], false);
    assert_eq!(value["status"], "unavailable");
    assert!(
        value["reason"]
            .as_str()
            .unwrap()
            .contains("actuation harness detect --json"),
        "{value}"
    );
    assert_eq!(value["providers"], json!([]));
    assert_eq!(value["panes"], json!([]));
}

/// The same intake law holds for the pid table: a census that cannot read
/// it is unavailable, never a census of unobserved panes.
#[cfg(unix)]
#[test]
fn place_census_cli_without_the_pid_table_is_unavailable_with_the_reason() {
    let actuation = actuation_answering("no-pid-table", &detection_document());
    // Only the answering `actuation` is on PATH: `ps` cannot be found.
    let output = run_with_path(&["--json", "places"], &actuation.display().to_string());
    let _ = std::fs::remove_dir_all(&actuation);
    assert!(output.status.success(), "{}", stderr_text(&output));
    let value = json_stdout(&output);
    assert_eq!(value["ok"], false);
    assert_eq!(value["status"], "unavailable");
    assert!(
        value["reason"].as_str().unwrap().contains("ps -axo"),
        "{value}"
    );
    assert_eq!(value["panes"], json!([]));
}

#[test]
fn place_request_cli_refuses_invalid_demand_by_name() {
    // Missing --name.
    let output = run(&["--json", "place", "request", "--provider", "tmux"]);
    assert_eq!(output.status.code(), Some(2));
    assert!(stderr_text(&output).contains("--name"));

    // Unknown provider.
    let output = run(&[
        "--json",
        "place",
        "request",
        "--provider",
        "teleport",
        "--name",
        "agent-place-cli-test",
    ]);
    assert_eq!(output.status.code(), Some(2));
    assert!(stderr_text(&output).contains("auto, herdr or tmux"));

    // A name outside [a-z0-9-]{1,64}: typed refusal document plus exit 2.
    let output = run(&[
        "--json",
        "place",
        "request",
        "--provider",
        "tmux",
        "--name",
        "Not A Place",
    ]);
    assert_eq!(output.status.code(), Some(2));
    let value = json_stdout(&output);
    assert_eq!(value["ok"], false);
    assert_eq!(value["refusal"]["kind"], "invalid-name");
}

#[test]
fn place_release_cli_refuses_a_ref_that_addresses_nothing() {
    let output = run(&[
        "--json",
        "place",
        "release",
        "--place-ref",
        "not-a-place-ref",
        "--pid",
        "1",
        "--start-marker",
        "Mon Jan 1 00:00:00 2001",
    ]);
    assert_eq!(output.status.code(), Some(2));
    let value = json_stdout(&output);
    assert_eq!(value["refusal"]["kind"], "invalid-place-ref");

    // A well-formed ref whose pid cannot be in the pid table: the
    // process-generation law refuses the stale binding, never kills.
    let output = run(&[
        "--json",
        "place",
        "release",
        "--place-ref",
        "workcell:place:tmux:default:workcell-cli-release-test",
        "--pid",
        "2147483647",
        "--start-marker",
        "Mon Jan 1 00:00:00 2001",
    ]);
    assert!(!output.status.success());
    let value = json_stdout(&output);
    assert_eq!(value["refusal"]["kind"], "stale-binding");
    assert!(value["refusal"]["evidence"]["granted_start_marker"].is_string());

    // Missing proof parts are usage errors.
    let output = run(&[
        "--json",
        "place",
        "release",
        "--place-ref",
        "workcell:place:tmux:default:x",
    ]);
    assert_eq!(output.status.code(), Some(2));
    assert!(stderr_text(&output).contains("usage: workcell place release"));
}

/// End-to-end tmux round trip through the CLI, only when `TMUX_TEST_LIVE` is
/// set (a live tmux server is required; the guard keeps ordinary test runs
/// free of machine effects).
#[test]
fn place_request_and_release_round_trip_through_the_cli_live() {
    if std::env::var("TMUX_TEST_LIVE").is_err() {
        return;
    }
    let name = format!("workcell-cli-place-{}", std::process::id());
    let requested = run(&[
        "--json",
        "place",
        "request",
        "--provider",
        "tmux",
        "--name",
        &name,
    ]);
    assert!(
        requested.status.success(),
        "{} {}",
        stdout_text(&requested),
        stderr_text(&requested)
    );
    let grant = json_stdout(&requested);
    assert_eq!(grant["provider"], "tmux");
    let place_ref = grant["place_ref"].as_str().unwrap().to_owned();
    let pid = grant["pane_pid"].as_u64().unwrap().to_string();
    let marker = grant["process_start_marker"].as_str().unwrap().to_owned();

    // A duplicate request must refuse, never silently adopt the session.
    let duplicate = run(&[
        "--json",
        "place",
        "request",
        "--provider",
        "tmux",
        "--name",
        &name,
    ]);
    assert!(!duplicate.status.success());
    assert_eq!(json_stdout(&duplicate)["refusal"]["kind"], "already-exists");

    // Release only through the proven generation.
    let released = run(&[
        "--json",
        "place",
        "release",
        "--place-ref",
        &place_ref,
        "--pid",
        &pid,
        "--start-marker",
        &marker,
    ]);
    assert!(
        released.status.success(),
        "{} {}",
        stdout_text(&released),
        stderr_text(&released)
    );
    let value = json_stdout(&released);
    assert_eq!(value["released"], true);
    assert_eq!(value["action"], "tmux kill-session");
}
