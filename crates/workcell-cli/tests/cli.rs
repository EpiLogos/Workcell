use std::{
    fs,
    path::{Path, PathBuf},
    process::{Command, Output},
    time::{SystemTime, UNIX_EPOCH},
};

use serde_json::{json, Value};

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

/// A detection document in Actuation's shape whose one row calls `sleep` the
/// executable of harness `fixture-sleeper`, found at `/bin/sleep` or not.
#[cfg(unix)]
fn sleeper_detection(detected: bool) -> Value {
    let mut row = json!({
        "slug": "fixture-sleeper",
        "harness_ref": "harness/fixture-sleeper",
        "native_kind": "harness",
        "state": if detected { "detected" } else { "not-installed" },
        "probes": [{"kind": "executable", "result": "pass", "spec": "sleep",
            "detail": if detected { "/bin/sleep" } else { "not found on PATH" }}],
    });
    if detected {
        row["receipts"] = json!({"executable": "/bin/sleep", "sha256": "ab".repeat(32)});
    }
    json!({
        "schema": "actuation.harness-detection/v1",
        "document": "detection",
        "harnesses": [row],
        "absent": if detected { json!([]) } else { json!(["fixture-sleeper"]) },
        "availability": "complete",
    })
}

/// A directory holding an `actuation` that answers `harness detect --json`
/// with `document`, as Actuation does.
#[cfg(unix)]
fn actuation_answering(label: &str, document: &Value) -> PathBuf {
    use std::os::unix::fs::PermissionsExt;
    let directory = temp_path(label);
    fs::create_dir_all(&directory).unwrap();
    let script = directory.join("actuation");
    fs::write(&script, format!("#!/bin/sh\nprintf '%s\\n' '{document}'\n")).unwrap();
    fs::set_permissions(&script, fs::Permissions::from_mode(0o755)).unwrap();
    directory
}

/// A child process killed and reaped however the test ends.
#[cfg(unix)]
struct Running(std::process::Child);

#[cfg(unix)]
impl Drop for Running {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

/// `instances scan` knows a harness process only by the executable names
/// Actuation's detection document gives. Here the document calls `sleep` a
/// harness — a name no list kept in Workcell could hold — and the scan binds
/// a running `sleep` to it, or names it unmatched when the row is not
/// detected.
#[cfg(unix)]
#[test]
fn instance_scan_cli_reads_harness_names_from_the_detection_document() {
    let sleeper = Running(Command::new("/bin/sleep").arg("60").spawn().unwrap());
    let pid = u64::from(sleeper.0.id());
    let home = temp_path("scan-vocabulary-home");
    fs::create_dir_all(&home).unwrap();

    let scan = |detected: bool| -> Value {
        let label = format!("scan-vocabulary-{detected}");
        let actuation =
            actuation_answering(&format!("{label}-actuation"), &sleeper_detection(detected));
        let state = temp_path(&format!("{label}-state"));
        let output = Command::new(binary())
            .args([
                "--state-root",
                path_arg(&state),
                "--json",
                "instances",
                "scan",
            ])
            .env("PATH", format!("{}:/usr/bin:/bin", actuation.display()))
            .env("HOME", &home)
            .output()
            .unwrap();
        let _ = fs::remove_dir_all(&actuation);
        let _ = fs::remove_dir_all(&state);
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        json_stdout(&output)
    };
    let sleeper_instances = |report: &Value| -> Vec<Value> {
        report["instances"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|record| record["harness_ref"] == "harness/fixture-sleeper")
            .cloned()
            .collect()
    };

    let detected = scan(true);
    assert_eq!(detected["status"], "ok", "{detected}");
    assert!(
        sleeper_instances(&detected)
            .iter()
            .any(|record| record["pids"].as_array().unwrap().contains(&json!(pid))),
        "the running sleep {pid} is bound to harness/fixture-sleeper: {detected}"
    );

    let listed_only = scan(false);
    assert_eq!(listed_only["status"], "ok", "{listed_only}");
    assert!(sleeper_instances(&listed_only).is_empty(), "{listed_only}");
    assert!(
        listed_only["unmatched_processes"]
            .as_array()
            .unwrap()
            .contains(&json!("sleep")),
        "a listed, undetected harness name that is running is named: {listed_only}"
    );

    let _ = fs::remove_dir_all(home);
}

#[test]
fn instance_usage_cli_observes_a_real_local_process_without_private_process_data() {
    let state = temp_path("resource-usage");
    let executable = std::env::current_exe().unwrap();
    let pid = std::process::id().to_string();
    let registered = run(&[
        "--state-root",
        path_arg(&state),
        "--workcell-ref",
        "workcell:owner-machine-test",
        "--json",
        "instances",
        "register",
        "--harness",
        "cli-resource-test",
        "--executable",
        path_arg(&executable),
        "--sha256",
        "real-cli-test-process",
        "--pid",
        &pid,
    ]);
    assert!(
        registered.status.success(),
        "{}",
        String::from_utf8_lossy(&registered.stderr)
    );

    let listed = run(&[
        "--state-root",
        path_arg(&state),
        "--workcell-ref",
        "workcell:owner-machine-test",
        "--json",
        "instances",
        "list",
    ]);
    let instance_ref = json_stdout(&listed)["instances"][0]["instance_ref"]
        .as_str()
        .unwrap()
        .to_owned();
    let usage = run(&[
        "--state-root",
        path_arg(&state),
        "--workcell-ref",
        "workcell:owner-machine-test",
        "--json",
        "instances",
        "usage",
        &instance_ref,
        "--pid",
        &pid,
        "--interval-ms",
        "15",
        "--correlation-ref",
        "opaque:factory-test-run",
    ]);
    assert!(
        usage.status.success(),
        "{}",
        String::from_utf8_lossy(&usage.stderr)
    );
    let reading = json_stdout(&usage);
    assert_eq!(reading["schema"], "workcell.resource-usage/v1");
    assert_eq!(reading["ok"], true);
    assert_eq!(reading["workcell_ref"], "workcell:owner-machine-test");
    assert_eq!(reading["harness_instance_ref"], instance_ref);
    assert_eq!(reading["material_binding"]["pid"], std::process::id());
    assert_eq!(reading["metrics"]["cpu_time"]["standing"], "observed");
    assert_eq!(reading["metrics"]["memory_rss"]["standing"], "observed");
    assert_eq!(
        reading["metrics"]["network_bytes"]["standing"],
        "unsupported"
    );
    assert_eq!(reading["provider"]["privacy"]["argv_collected"], false);
    assert_eq!(
        reading["provider"]["privacy"]["environment_collected"],
        false
    );
    assert_eq!(
        reading["external_correlation_refs"][0],
        "opaque:factory-test-run"
    );
    assert!(reading.get("argv").is_none());
    assert!(reading.get("environment").is_none());

    let _ = fs::remove_dir_all(state);
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

#[test]
fn unknown_flags_are_named_as_options_not_commands() {
    let flag = run(&["--bogus-flag"]);
    assert!(!flag.status.success());
    let stderr = String::from_utf8(flag.stderr).unwrap();
    assert!(
        stderr.contains("unknown option `--bogus-flag`"),
        "a leading-dash token is an option, not a command: {stderr}"
    );

    let command = run(&["bogus-command"]);
    assert!(!command.status.success());
    let stderr = String::from_utf8(command.stderr).unwrap();
    assert!(
        stderr.contains("unknown command `bogus-command`"),
        "a bare token stays a command: {stderr}"
    );
}

#[test]
fn workcell_target_secret_projection_is_refused_at_record_time() {
    let state = temp_path("secret-project-workcell-refused");
    let refused = run(&[
        "--state-root",
        path_arg(&state),
        "--workcell-ref",
        "workcell:owner-machine-test",
        "secret",
        "project",
        "--name",
        "blocked-workcell-projection",
        "--to-workcell",
        "workcell:remote-cell",
        "--connection",
        "cell-relation-test",
        "--credential-ref",
        "secret-ref:test-credential",
        "--source-provider",
        "provider:keychain",
        "--class",
        "credential-broker",
        "--purpose",
        "cross-cell-relation-test",
        "--scope",
        "scope:test",
        "--by",
        "agent:test",
    ]);
    assert!(
        !refused.status.success(),
        "a workcell-target projection must refuse instead of recording a grant"
    );
    let stderr = String::from_utf8_lossy(&refused.stderr);
    assert!(
        stderr.contains("not yet materialisable"),
        "the refusal names the target as not yet materialisable: {stderr}"
    );
    assert!(
        stderr.contains("--to-sandbox"),
        "the refusal names sandbox-target projections as the materialisable path: {stderr}"
    );
    assert!(
        stderr.contains("revoke-projection"),
        "the refusal points at the ledger/revoke surface: {stderr}"
    );
    assert!(
        !state.join("secrets").join("projections.json").exists(),
        "no durable grant may be recorded for a workcell target"
    );

    let _ = fs::remove_dir_all(state);
}

#[test]
fn sandboxes_reconcile_no_longer_accepts_a_raw_api_key() {
    // The raw literal is refused as an unknown flag, before any connection.
    let refused = run(&[
        "sandboxes",
        "reconcile",
        "--server",
        "http://127.0.0.1:9",
        "--api-key",
        "test-only-marker-not-a-secret",
    ]);
    assert!(!refused.status.success());
    let stderr = String::from_utf8_lossy(&refused.stderr);
    assert!(
        stderr.contains("unknown sandboxes reconcile flag `--api-key`"),
        "the raw credential flag is gone: {stderr}"
    );

    // The usage no longer advertises a literal key, and keeps the env form.
    let usage = run(&["sandboxes", "reconcile"]);
    assert!(!usage.status.success());
    let usage_text = String::from_utf8_lossy(&usage.stderr);
    assert!(
        !usage_text.contains("--api-key <key>"),
        "usage must not advertise a raw key literal: {usage_text}"
    );
    assert!(
        usage_text.contains("--api-key-env <ENV>"),
        "usage keeps the environment-variable form: {usage_text}"
    );

    // The environment form still parses (failure here is the missing server,
    // not an unknown flag).
    let env_form = run(&[
        "sandboxes",
        "reconcile",
        "--api-key-env",
        "WORKCELL_TEST_KEY_ENV",
    ]);
    assert!(!env_form.status.success());
    let env_text = String::from_utf8_lossy(&env_form.stderr);
    assert!(
        env_text.contains("--server <url>"),
        "`--api-key-env` still parses; the failure is the missing server: {env_text}"
    );
}

// ---- Projection correlation -------------------------------------------
//
// `workcell correlate-projection` carries an AIKit git-projection verdict as a
// correlated observation on a prepared world's checkout subject, running no git.

fn write_json(label: &str, body: &str) -> PathBuf {
    let path = temp_path(label);
    fs::write(&path, body).unwrap();
    path
}

/// A `workcell.material-world/v1` receipt carrying an opaque `checkout:workcell`
/// subject — exactly what `prepare --subject checkout:workcell=<ref>` persists.
fn checkout_world_receipt() -> PathBuf {
    write_json(
        "corr-world",
        r#"{
          "version": "workcell.material-world/v1",
          "world_ref": "world:dev-environment",
          "workcell_ref": "workcell:local",
          "demand_ref": "demand:dev-environment",
          "subjects": { "checkout:workcell": "dev-environment:/Users/dev/worktrees/env-1/workcell" },
          "binding_graph": {"bindings": [], "relations": []},
          "planned_exposures": [],
          "planned_constraints": [],
          "plan_degradations": [],
          "plan_omissions": [],
          "persistence": null,
          "retention": "release",
          "state": "healthy",
          "provenance": {}
        }"#,
    )
}

#[test]
fn correlate_projection_carries_a_projected_verdict_attributed_to_aikit() {
    let receipt = checkout_world_receipt();
    // A full `aikit worktree project --json` reply envelope: SuiteProjection
    // under `data`, all checkouts ending at the target.
    let projection = write_json(
        "corr-projected",
        r#"{
          "ok": true,
          "data": {
            "version": "aikit.worktree-projection/v1",
            "target": "origin/main",
            "applied": true,
            "entries": [
              { "key": "workcell", "action": { "action": "fast-forwarded" } },
              { "key": "central", "action": { "action": "already-projected" } }
            ],
            "summary": ["2/2 checkouts projected onto origin/main (apply)"]
          },
          "warnings": []
        }"#,
    );

    let output = run(&[
        "--json",
        "--receipt",
        path_arg(&receipt),
        "correlate-projection",
        "--projection",
        path_arg(&projection),
        "--subject",
        "checkout:workcell",
    ]);
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let reading = json_stdout(&output);
    assert_eq!(reading["ok"], true);
    assert_eq!(reading["version"], "workcell.correlated-observation/v1");
    assert_eq!(reading["world_ref"], "world:dev-environment");
    assert_eq!(reading["subject_key"], "checkout:workcell");
    // The opaque checkout ref is preserved verbatim from the world's subject.
    assert_eq!(
        reading["subject"],
        "dev-environment:/Users/dev/worktrees/env-1/workcell"
    );
    // The verdict is attributed to AIKit and carried, not computed by Workcell.
    assert_eq!(reading["attributed_to"], "aikit.worktree-projection/v1");
    assert_eq!(reading["correlation"]["projected"], true);
    assert_eq!(reading["correlation"]["applied"], true);
    assert_eq!(reading["correlation"]["target"], "origin/main");
    assert!(reading["correlation"]["surfaced"]
        .as_array()
        .unwrap()
        .is_empty());
}

#[test]
fn correlate_projection_carries_surfaced_drift_verbatim() {
    let receipt = checkout_world_receipt();
    let projection = write_json(
        "corr-surfaced",
        r#"{
          "data": {
            "version": "aikit.worktree-projection/v1",
            "target": "origin/main",
            "applied": false,
            "entries": [
              { "key": "workcell", "action": { "action": "already-projected" } },
              { "key": "o-i", "action": { "action": "surfaced", "reason": "ahead of origin/main" } },
              { "key": "factory", "action": { "action": "failed", "reason": "could not be read" } }
            ],
            "summary": ["1/3 checkouts projected onto origin/main (observe)"]
          }
        }"#,
    );

    let output = run(&[
        "--json",
        "--receipt",
        path_arg(&receipt),
        "correlate-projection",
        "--projection",
        path_arg(&projection),
        "--subject",
        "checkout:workcell",
    ]);
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let reading = json_stdout(&output);
    assert_eq!(reading["correlation"]["projected"], false);
    let surfaced = reading["correlation"]["surfaced"]
        .as_array()
        .unwrap()
        .iter()
        .map(|value| value.as_str().unwrap().to_owned())
        .collect::<Vec<_>>();
    // AIKit's surfaced/failed keys are carried verbatim; Workcell derives nothing.
    assert_eq!(surfaced, vec!["o-i".to_owned(), "factory".to_owned()]);
}

#[test]
fn correlate_projection_refuses_a_subject_absent_from_the_world() {
    let receipt = checkout_world_receipt();
    let projection = write_json(
        "corr-absent",
        r#"{
          "data": {
            "version": "aikit.worktree-projection/v1",
            "target": "origin/main",
            "applied": false,
            "entries": [ { "key": "workcell", "action": { "action": "already-projected" } } ],
            "summary": ["1/1 checkouts projected onto origin/main (observe)"]
          }
        }"#,
    );

    let output = run(&[
        "--json",
        "--receipt",
        path_arg(&receipt),
        "correlate-projection",
        "--projection",
        path_arg(&projection),
        "--subject",
        "checkout:not-on-this-world",
    ]);
    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("not a subject of world"),
        "expected a subject-refusal, got: {stderr}"
    );
}
