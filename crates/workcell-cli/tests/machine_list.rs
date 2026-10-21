//! `workcell machine list --json` discloses the control plane's operations
//! beside the declarations, so a caller names what a declaration may allow
//! and a grant may carry from Workcell's own list, never a guess.

use std::process::Command;

use serde_json::{json, Value};

fn workcell(state_root: &std::path::Path, args: &[&str]) -> (bool, Value, String) {
    let output = Command::new(env!("CARGO_BIN_EXE_workcell"))
        .arg("--state-root")
        .arg(state_root)
        .args(args)
        .env_remove("WORKCELL_CONTROL_TOKEN")
        .env_remove("WORKCELL_CONTROL_ENDPOINT")
        .output()
        .unwrap();
    (
        output.status.success(),
        serde_json::from_slice(&output.stdout).unwrap_or(Value::Null),
        String::from_utf8_lossy(&output.stdout).into_owned(),
    )
}

struct Root(std::path::PathBuf);
impl Root {
    fn path(&self) -> &std::path::Path {
        &self.0
    }
}
impl Drop for Root {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

#[test]
fn the_control_operations_stand_beside_the_declarations() {
    let root = std::env::temp_dir().join(format!("workcell-machine-list-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).unwrap();
    let root = Root(root);
    let (ok, listed, _) = workcell(root.path(), &["machine", "list", "--json"]);
    assert!(ok);
    assert_eq!(listed["machines"], json!([]));
    let operations = json!(epilogos_workcell_control::CONTROL_OPERATIONS);
    assert_eq!(listed["control_operations"], operations);
    assert_eq!(listed["control_operations"].as_array().unwrap().len(), 14);

    let (ok, _, _) = workcell(
        root.path(),
        &[
            "machine",
            "add",
            "--label",
            "scratch",
            "--endpoint",
            "127.0.0.1:9",
            "--allow",
            "status",
            "--allow",
            "discover",
            "--json",
        ],
    );
    assert!(ok);
    let (ok, listed, _) = workcell(root.path(), &["machine", "list", "--json"]);
    assert!(ok);
    assert_eq!(
        listed["machines"][0]["operations"],
        json!(["status", "discover"])
    );
    assert_eq!(listed["control_operations"], operations);
    // Every operation a declaration carries is one Workcell lists.
    for operation in listed["machines"][0]["operations"].as_array().unwrap() {
        assert!(listed["control_operations"]
            .as_array()
            .unwrap()
            .contains(operation));
    }

    let (ok, _, plain) = workcell(root.path(), &["machine", "list"]);
    assert!(ok);
    assert!(
        plain.contains("operations a machine may be granted: status, discover, plan"),
        "{plain}"
    );
}
