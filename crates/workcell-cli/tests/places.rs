use std::process::{Command, Output};

use serde_json::Value;

fn binary() -> &'static str {
    env!("CARGO_BIN_EXE_workcell")
}

fn run(args: &[&str]) -> Output {
    Command::new(binary()).args(args).output().unwrap()
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
#[test]
fn place_census_cli_runs_read_only_and_discloses_provider_states() {
    let output = run(&["--json", "places"]);
    assert!(output.status.success(), "{}", stderr_text(&output));
    let value = json_stdout(&output);
    assert_eq!(value["schema"], "workcell.place-census/v1");
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
