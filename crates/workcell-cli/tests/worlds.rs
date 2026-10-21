//! `workcell worlds`: the material-world receipts a cell keeps under its state
//! root, read back through the wire decoder. Every test runs against a scratch
//! state root; nothing touches a real cell.

use std::{
    fs,
    path::{Path, PathBuf},
    process::{Command, Output},
    time::{SystemTime, UNIX_EPOCH},
};

use serde_json::Value;

fn temp_path(label: &str) -> PathBuf {
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    std::env::temp_dir().join(format!(
        "epilogos-workcell-worlds-{label}-{}-{nonce}",
        std::process::id()
    ))
}

fn run(args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_workcell"))
        .args(args)
        .output()
        .unwrap()
}

fn path_arg(path: &Path) -> &str {
    path.to_str().unwrap()
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

fn worlds(state: &Path) -> Value {
    json_stdout(&run(&["--state-root", path_arg(state), "--json", "worlds"]))
}

#[test]
fn an_empty_cell_lists_no_worlds_and_says_where_it_looked() {
    let state = temp_path("empty");
    let reading = worlds(&state);
    assert_eq!(reading["ok"], true);
    assert_eq!(reading["schema"], "workcell.world-receipts/v1");
    assert_eq!(reading["worlds"], serde_json::json!([]));
    assert_eq!(reading["unreadable"], serde_json::json!([]));
    assert_eq!(
        reading["receipt_root"],
        state.join("worlds").display().to_string()
    );
    assert!(!state.exists(), "a read must not create the state root");
}

#[test]
fn a_prepared_world_is_listed_with_its_ref_demand_and_subjects() {
    let state = temp_path("prepared");
    let prepared = json_stdout(&run(&[
        "--state-root",
        path_arg(&state),
        "--json",
        "prepare",
        "--demand-ref",
        "demand:worlds-test",
        "--require",
        "shell",
        "--subject",
        "run=opaque:factory-run-7",
    ]));
    let world_ref = prepared["world"]["world_ref"].as_str().unwrap().to_owned();
    let receipt = prepared["receipt"].as_str().unwrap().to_owned();

    let reading = worlds(&state);
    let listed = reading["worlds"].as_array().unwrap();
    assert_eq!(listed.len(), 1, "{reading:#}");
    let world = &listed[0];
    assert_eq!(world["world_ref"], world_ref.as_str());
    assert_eq!(world["receipt"], receipt.as_str());
    assert_eq!(world["demand_ref"], "demand:worlds-test");
    assert_eq!(world["workcell_ref"], prepared["world"]["workcell_ref"]);
    assert_eq!(world["subjects"], prepared["world"]["subjects"]);
    assert!(
        world["subjects"]
            .to_string()
            .contains("opaque:factory-run-7"),
        "{world:#}"
    );
    assert_eq!(world["state"], prepared["world"]["state"]);
    assert_eq!(world["retention"], prepared["world"]["retention"]);
    assert_eq!(
        world["bindings"].as_u64().unwrap() as usize,
        prepared["world"]["binding_graph"]["bindings"]
            .as_array()
            .unwrap()
            .len()
    );
    assert_eq!(reading["unreadable"], serde_json::json!([]));

    // The status count and the listing agree on what the cell keeps.
    let status = json_stdout(&run(&[
        "--state-root",
        path_arg(&state),
        "--json",
        "status",
    ]));
    assert_eq!(status["persisted_world_receipts"], 1, "{status:#}");

    let released = run(&[
        "--state-root",
        path_arg(&state),
        "--receipt",
        &receipt,
        "--json",
        "release",
    ]);
    assert!(released.status.success());
    let _ = fs::remove_dir_all(state);
}

#[test]
fn a_receipt_written_elsewhere_is_not_listed_and_the_answer_says_so() {
    let state = temp_path("elsewhere");
    let receipt = temp_path("elsewhere-receipt").with_extension("json");
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
    let reading = worlds(&state);
    assert_eq!(reading["worlds"], serde_json::json!([]));
    assert!(reading["note"].as_str().unwrap().contains("--receipt"));
    let _ = run(&[
        "--state-root",
        path_arg(&state),
        "--receipt",
        path_arg(&receipt),
        "--json",
        "release",
    ]);
    let _ = fs::remove_file(receipt);
    let _ = fs::remove_dir_all(state);
}

#[test]
fn a_torn_receipt_is_named_unreadable_beside_the_good_ones() {
    let state = temp_path("torn");
    let directory = state.join("worlds");
    fs::create_dir_all(&directory).unwrap();
    fs::write(directory.join("torn.json"), "{\"world_ref\":").unwrap();
    fs::write(directory.join("notes.txt"), "not a receipt").unwrap();
    let reading = worlds(&state);
    assert_eq!(reading["worlds"], serde_json::json!([]));
    let unreadable = reading["unreadable"].as_array().unwrap();
    assert_eq!(unreadable.len(), 1, "{reading:#}");
    assert_eq!(
        unreadable[0]["receipt"],
        directory.join("torn.json").display().to_string()
    );
    assert!(!unreadable[0]["error"].as_str().unwrap().is_empty());
    let _ = fs::remove_dir_all(state);
}

#[test]
fn worlds_takes_no_arguments() {
    let state = temp_path("args");
    let output = run(&[
        "--state-root",
        path_arg(&state),
        "--json",
        "worlds",
        "--all",
    ]);
    assert!(!output.status.success());
}
