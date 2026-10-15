//! A machine names its cell in its state root, and every local caller that
//! passes no `--workcell-ref` answers as that cell.
//!
//! The case this closes: a Mac renamed its cell `workcell:local` →
//! `workcell:mac`, served as `workcell:mac` and bound its connection grants
//! there, while every caller that passes no flag (O:I's status read,
//! Central's NOW field) still answered as `workcell:local` and was refused by
//! the grants registry on every call. Each test runs the real binary against
//! its own state root.

use std::{
    fs,
    path::{Path, PathBuf},
    process::{Command, Output},
    time::{SystemTime, UNIX_EPOCH},
};

use serde_json::Value;

struct Root(PathBuf);

impl Drop for Root {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn root(label: &str) -> Root {
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let path = std::env::temp_dir().join(format!(
        "workcell-identity-cli-{label}-{}-{nonce}",
        std::process::id()
    ));
    fs::create_dir_all(&path).unwrap();
    Root(path)
}

fn workcell(state_root: &Path, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_workcell"))
        .arg("--state-root")
        .arg(state_root)
        .args(args)
        .env_remove("WORKCELL_HOME")
        .env_remove("WORKCELL_CONTROL_TOKEN")
        .env_remove("WORKCELL_CONTROL_ENDPOINT")
        .output()
        .unwrap()
}

fn ok_json(output: &Output) -> Value {
    assert!(
        output.status.success(),
        "command failed ({}): {} / {}",
        output.status,
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    serde_json::from_slice(&output.stdout).unwrap()
}

fn refusal(output: &Output) -> String {
    assert!(!output.status.success(), "expected a refusal");
    String::from_utf8_lossy(&output.stderr).into_owned()
}

/// Bind the grants registry to `workcell:mac`, as `workcell --workcell-ref
/// workcell:mac authorise` does on the renamed machine.
fn grants_bound_to_mac(state_root: &Path) {
    let granted = workcell(
        state_root,
        &[
            "--workcell-ref",
            "workcell:mac",
            "--json",
            "authorise",
            "--client",
            "omarchy",
            "--allow",
            "status",
        ],
    );
    assert!(
        granted.status.success(),
        "{}",
        String::from_utf8_lossy(&granted.stderr)
    );
}

#[test]
fn a_declared_identity_answers_unflagged_status_on_the_renamed_machine() {
    let root = root("status");
    let state = &root.0;

    // Undeclared: every unflagged caller is workcell:local, as before.
    let before = ok_json(&workcell(state, &["status", "--json"]));
    assert_eq!(before["workcell_ref"], "workcell:local");
    assert_eq!(before["workcell_ref_source"], "default");

    // The renamed machine: grants bound to workcell:mac refuse the unflagged
    // status read — the refusal O:I met on every boot.
    grants_bound_to_mac(state);
    let refused = refusal(&workcell(state, &["status", "--json"]));
    assert!(
        refused.contains("belong to workcell:mac, not workcell:local"),
        "{refused}"
    );

    // Declare the machine's cell; the same unflagged argv now answers.
    let declared = ok_json(&workcell(
        state,
        &["--json", "identity", "set", "workcell:mac"],
    ));
    assert_eq!(declared["outcome"], "declared");
    assert_eq!(declared["workcell_ref"], "workcell:mac");
    assert_eq!(declared["source"], "declared");
    let status = ok_json(&workcell(state, &["status", "--json"]));
    assert_eq!(status["workcell_ref"], "workcell:mac");
    assert_eq!(status["workcell_ref_source"], "declared");
    assert_eq!(status["connection_grants"]["active"], 1);

    // The reading names the declaration and each store's binding.
    let reading = ok_json(&workcell(state, &["identity", "--json"]));
    assert_eq!(reading["schema"], "workcell.identity-reading/v1");
    assert_eq!(reading["declaration"]["reading"]["state"], "present");
    let grants = reading["stores"]
        .as_array()
        .unwrap()
        .iter()
        .find(|store| store["store"] == "connections/grants.json")
        .unwrap();
    assert_eq!(grants["bound_to"], "workcell:mac");
    assert_eq!(grants["agrees"], true);

    // Re-declaring the same cell changes nothing; an explicit flag still wins.
    let again = ok_json(&workcell(
        state,
        &["--json", "identity", "set", "workcell:mac"],
    ));
    assert_eq!(again["outcome"], "unchanged");
    let flagged = ok_json(&workcell(
        state,
        &["--workcell-ref", "workcell:elsewhere", "identity", "--json"],
    ));
    assert_eq!(flagged["workcell_ref"], "workcell:elsewhere");
    assert_eq!(flagged["source"], "flag");
}

#[test]
fn a_split_state_root_is_refused_until_rebind_carries_it_with_its_prior_bytes() {
    let root = root("split");
    let state = &root.0;

    // The Mac's actual split: harness instances observed while the cell was
    // still workcell:local, grants bound after the rename to workcell:mac.
    let declared_instance = ok_json(&workcell(
        state,
        &["--json", "instances", "declare", "--harness", "codex"],
    ));
    assert_eq!(declared_instance["ok"], true, "{declared_instance}");
    grants_bound_to_mac(state);
    let registry = state.join("instances/registry.json");
    let prior = fs::read(&registry).unwrap();

    // Declaring workcell:mac would strand the instance registry: refused,
    // naming the store, and nothing is written.
    let stranded = refusal(&workcell(state, &["identity", "set", "workcell:mac"]));
    assert!(
        stranded.contains("instances/registry.json (bound to workcell:local)"),
        "{stranded}"
    );
    assert!(stranded.contains("--rebind"), "{stranded}");
    assert!(!state.join("identity.json").exists());
    assert_eq!(fs::read(&registry).unwrap(), prior);

    // --rebind carries the registry; its exact prior bytes stay beside it.
    let rebound = ok_json(&workcell(
        state,
        &["--json", "identity", "set", "workcell:mac", "--rebind"],
    ));
    let entries = rebound["rebound"].as_array().unwrap();
    assert_eq!(entries.len(), 1, "{rebound}");
    assert_eq!(entries[0]["store"], "instances/registry.json");
    assert_eq!(entries[0]["from"], "workcell:local");
    let backup = PathBuf::from(entries[0]["backup"].as_str().unwrap());
    assert_eq!(fs::read(&backup).unwrap(), prior);
    let declaration: Value =
        serde_json::from_slice(&fs::read(state.join("identity.json")).unwrap()).unwrap();
    assert_eq!(declaration["schema"], "workcell.identity/v1");
    assert_eq!(
        declaration["provenance"]["rebound"][0]["from"],
        "workcell:local"
    );

    // Every unflagged read now answers as workcell:mac, instances included,
    // with each record's identity carried and its instance ref kept.
    let instances = ok_json(&workcell(state, &["--json", "instances", "list"]));
    assert_eq!(instances["workcell_ref"], "workcell:mac", "{instances}");
    let before: Value = serde_json::from_slice(&prior).unwrap();
    let after: Value = serde_json::from_slice(&fs::read(&registry).unwrap()).unwrap();
    assert_eq!(
        before["instances"]
            .as_object()
            .unwrap()
            .keys()
            .collect::<Vec<_>>(),
        after["instances"]
            .as_object()
            .unwrap()
            .keys()
            .collect::<Vec<_>>()
    );
    for record in after["instances"].as_object().unwrap().values() {
        assert_eq!(record["workcell_ref"], "workcell:mac");
    }
    let status = ok_json(&workcell(state, &["status", "--json"]));
    assert_eq!(status["workcell_ref"], "workcell:mac");
    let reading = ok_json(&workcell(state, &["identity", "--json"]));
    assert!(reading["stores"]
        .as_array()
        .unwrap()
        .iter()
        .all(|store| store["agrees"] == true));
}

#[test]
fn a_world_receipt_of_another_cell_is_never_carried_by_a_rename() {
    let root = root("receipt");
    let state = &root.0;
    fs::create_dir_all(state.join("worlds")).unwrap();
    fs::write(
        state.join("worlds/world_probe.json"),
        r#"{"workcell_ref":"workcell:local"}"#,
    )
    .unwrap();
    let refused = refusal(&workcell(
        state,
        &["identity", "set", "workcell:mac", "--rebind"],
    ));
    assert!(
        refused.contains("worlds/world_probe.json (bound to workcell:local)"),
        "{refused}"
    );
    assert!(refused.contains("not renaming the host"), "{refused}");
    assert!(!state.join("identity.json").exists());
}

#[test]
fn an_unreadable_declaration_refuses_by_name_and_identity_set_repairs_it() {
    let root = root("unreadable");
    let state = &root.0;
    fs::write(state.join("identity.json"), "{not json").unwrap();
    let refused = refusal(&workcell(state, &["status", "--json"]));
    assert!(refused.contains("parse Workcell identity"), "{refused}");
    let reading = ok_json(&workcell(state, &["identity", "--json"]));
    assert_eq!(reading["declaration"]["reading"]["state"], "unreadable");
    ok_json(&workcell(
        state,
        &["--json", "identity", "set", "workcell:mac"],
    ));
    let status = ok_json(&workcell(state, &["status", "--json"]));
    assert_eq!(status["workcell_ref"], "workcell:mac");
}

#[test]
fn rebind_carries_grants_bound_to_the_old_name_keeping_every_grant() {
    let root = root("grants");
    let state = &root.0;
    // Grants authorised before the rename, as workcell:local.
    let granted = workcell(
        state,
        &[
            "--json",
            "authorise",
            "--client",
            "omarchy",
            "--allow",
            "status",
        ],
    );
    assert!(
        granted.status.success(),
        "{}",
        String::from_utf8_lossy(&granted.stderr)
    );
    let grants = state.join("connections/grants.json");
    let prior = fs::read(&grants).unwrap();
    let rebound = ok_json(&workcell(
        state,
        &["--json", "identity", "set", "workcell:mac", "--rebind"],
    ));
    assert_eq!(rebound["rebound"][0]["store"], "connections/grants.json");
    let backup = PathBuf::from(rebound["rebound"][0]["backup"].as_str().unwrap());
    assert_eq!(fs::read(backup).unwrap(), prior);
    let before: Value = serde_json::from_slice(&prior).unwrap();
    let after: Value = serde_json::from_slice(&fs::read(&grants).unwrap()).unwrap();
    assert_eq!(after["workcell_ref"], "workcell:mac");
    assert_eq!(
        before["grants"], after["grants"],
        "every grant is kept exactly"
    );
    let status = ok_json(&workcell(state, &["status", "--json"]));
    assert_eq!(status["workcell_ref"], "workcell:mac");
    assert_eq!(status["connection_grants"]["active"], 1);
}
