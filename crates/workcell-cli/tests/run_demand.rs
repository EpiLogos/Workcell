//! A run keeps the demand body it was started with beside its record
//! (`runs/<slug>/demand.json`), and `workcell run show --json` answers it,
//! checked against the record's `demand_digest`. A record without a body
//! answers that honestly; a body that is not the digested one is not carried.
//! Every test runs against a scratch state root.

use std::{
    fs,
    path::{Path, PathBuf},
    process::{Command, Output},
    time::{SystemTime, UNIX_EPOCH},
};

use serde_json::Value;
use sha2::{Digest, Sha256};

fn temp_path(label: &str) -> PathBuf {
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    std::env::temp_dir().join(format!(
        "epilogos-workcell-run-demand-{label}-{}-{nonce}",
        std::process::id()
    ))
}

fn workcell(state: &Path, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_workcell"))
        .arg("--state-root")
        .arg(state)
        .arg("--json")
        .args(args)
        .output()
        .unwrap()
}

fn json_stdout(output: &Output) -> Value {
    assert!(
        output.status.success(),
        "stderr={}",
        String::from_utf8_lossy(&output.stderr)
    );
    serde_json::from_slice(&output.stdout).unwrap_or_else(|error| {
        panic!(
            "invalid JSON stdout: {error}\nstdout={}",
            String::from_utf8_lossy(&output.stdout)
        )
    })
}

fn start(state: &Path, slug: &str) -> Value {
    json_stdout(&workcell(
        state,
        &[
            "run",
            "start",
            "--run",
            slug,
            "--demand-ref",
            "demand:run-demand-test",
            "--require",
            "shell",
            "--subject",
            "run=opaque:factory-run-9",
        ],
    ))
}

fn show(state: &Path, slug: &str) -> Value {
    json_stdout(&workcell(state, &["run", "show", "--run", slug]))
}

fn release(state: &Path, slug: &str) {
    let _ = workcell(state, &["run", "release", "--run", slug]);
}

#[test]
fn a_started_run_keeps_its_demand_and_show_answers_it_against_the_digest() {
    let state = temp_path("kept");
    let started = start(&state, "kept-1");
    let digest = started["run"]["demand_digest"].as_str().unwrap().to_owned();

    let shown = show(&state, "kept-1");
    let demand = &shown["demand"];
    assert_eq!(demand["standing"], "kept", "{shown:#}");
    assert_eq!(demand["digest"], digest.as_str());
    let path = PathBuf::from(demand["path"].as_str().unwrap());
    assert_eq!(path, state.join("runs").join("kept-1").join("demand.json"));
    // The kept bytes are the digested bytes.
    let bytes = fs::read(&path).unwrap();
    assert_eq!(format!("sha256:{:x}", Sha256::digest(&bytes)), digest);
    assert_eq!(demand["body"]["demand_ref"], "demand:run-demand-test");
    assert_eq!(demand["body"]["affordances"]["required"][0], "shell");
    assert_eq!(demand["body"]["subjects"]["run"], "opaque:factory-run-9");
    // The record itself is unchanged by keeping the body.
    assert_eq!(shown["run"]["demand_digest"], digest.as_str());

    // The kept body is a demand `workcell plan` takes as it is.
    let plan = json_stdout(&workcell(
        &state,
        &["plan", "--demand-json", path.to_str().unwrap()],
    ));
    assert_eq!(plan["demand_ref"], "demand:run-demand-test");
    assert_eq!(plan["status"], "satisfiable");

    // The run directory beside the record is not listed as a run.
    let listed = json_stdout(&workcell(&state, &["run", "list"]));
    assert_eq!(listed["runs"].as_array().unwrap().len(), 1);

    release(&state, "kept-1");
    let _ = fs::remove_dir_all(state);
}

#[test]
fn a_record_without_a_body_says_so_and_nothing_is_reconstructed() {
    let state = temp_path("not-kept");
    start(&state, "legacy-1");
    // A record written before Workcell kept demand bodies has none beside it.
    fs::remove_dir_all(state.join("runs").join("legacy-1")).unwrap();
    let shown = show(&state, "legacy-1");
    let demand = &shown["demand"];
    assert_eq!(demand["standing"], "not-kept", "{shown:#}");
    assert!(demand.get("body").is_none());
    assert_eq!(demand["digest"], shown["run"]["demand_digest"]);
    assert!(demand["detail"].as_str().unwrap().contains("demand_digest"));
    release(&state, "legacy-1");
    let _ = fs::remove_dir_all(state);
}

#[test]
fn a_body_that_is_not_the_digested_one_is_named_and_not_carried() {
    let state = temp_path("mismatch");
    start(&state, "changed-1");
    let path = state.join("runs").join("changed-1").join("demand.json");
    fs::write(&path, br#"{"demand_ref":"demand:someone-else"}"#).unwrap();
    let shown = show(&state, "changed-1");
    let demand = &shown["demand"];
    assert_eq!(demand["standing"], "mismatch", "{shown:#}");
    assert!(demand.get("body").is_none());
    assert_ne!(demand["body_digest"], demand["digest"]);

    // A body that cannot be read is named unreadable, with its error.
    fs::remove_file(&path).unwrap();
    fs::create_dir(&path).unwrap();
    let shown = show(&state, "changed-1");
    assert_eq!(shown["demand"]["standing"], "unreadable", "{shown:#}");
    assert!(shown["demand"].get("body").is_none());
    release(&state, "changed-1");
    let _ = fs::remove_dir_all(state);
}
