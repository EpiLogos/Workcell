#![cfg(unix)]

use std::{
    fs,
    path::{Path, PathBuf},
    process::Command,
    thread,
    time::{SystemTime, UNIX_EPOCH},
};

use epilogos_workcell_control::{ControlService, TcpControlServer};
use epilogos_workcell_core::WorkcellRef;
use epilogos_workcell_runtime::{CollapsedLocalConfig, CollapsedLocalWorkcell};
use serde_json::Value;

fn temp_path(label: &str) -> PathBuf {
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    std::env::temp_dir().join(format!(
        "epilogos-workcell-remote-cli-{label}-{}-{nonce}",
        std::process::id()
    ))
}

fn local(root: &Path) -> CollapsedLocalWorkcell {
    CollapsedLocalWorkcell::new(CollapsedLocalConfig::new(
        WorkcellRef::new("workcell:remote-cli-test").unwrap(),
        root,
    ))
    .unwrap()
}

fn run_json(args: &[String], token: Option<&str>) -> std::process::Output {
    let mut command = Command::new(env!("CARGO_BIN_EXE_workcell"));
    command.args(args);
    if let Some(token) = token {
        command.env("WORKCELL_CONTROL_TOKEN", token);
    } else {
        command.env_remove("WORKCELL_CONTROL_TOKEN");
    }
    command.env_remove("WORKCELL_CONTROL_ENDPOINT");
    command.output().unwrap()
}

fn stdout_json(output: &std::process::Output) -> Value {
    assert!(
        output.status.success(),
        "command failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    serde_json::from_slice(&output.stdout).unwrap()
}

#[test]
fn native_workcell_cli_selects_service_backend_without_identity_translation() {
    let root = temp_path("parity");
    let server_root = root.join("server");
    let receipt = root.join("client-world.json");
    let service = ControlService::new(local(&server_root));
    let mut server = TcpControlServer::bind("127.0.0.1:0", service).unwrap();
    let endpoint = server.local_addr().unwrap().to_string();

    let client_endpoint = endpoint.clone();
    let client_receipt = receipt.clone();
    let client_thread = thread::spawn(move || {
        let base = || {
            vec![
                "--endpoint".into(),
                client_endpoint.clone(),
                "--json".into(),
            ]
        };

        let mut args = base();
        args.push("status".into());
        let status = stdout_json(&run_json(&args, None));
        assert_eq!(status["workcell_ref"], "workcell:remote-cli-test");

        let mut args = base();
        args.extend([
            "plan".into(),
            "--demand-ref".into(),
            "demand:remote-cli".into(),
            "--require".into(),
            "shell".into(),
        ]);
        let plan = stdout_json(&run_json(&args, None));
        assert_eq!(plan["demand_ref"], "demand:remote-cli");
        assert_eq!(plan["status"], "satisfiable");

        let mut args = base();
        args.extend([
            "--receipt".into(),
            client_receipt.display().to_string(),
            "prepare".into(),
            "--demand-ref".into(),
            "demand:remote-cli".into(),
            "--require".into(),
            "shell".into(),
        ]);
        let prepared = stdout_json(&run_json(&args, None));
        assert_eq!(prepared["world"]["demand_ref"], "demand:remote-cli");
        let world_ref = prepared["world"]["world_ref"].as_str().unwrap().to_owned();
        assert!(client_receipt.exists());

        let mut args = base();
        args.extend([
            "--receipt".into(),
            client_receipt.display().to_string(),
            "observe".into(),
        ]);
        let observed = stdout_json(&run_json(&args, None));
        assert_eq!(observed["world_ref"], world_ref);

        let mut args = base();
        args.extend([
            "--receipt".into(),
            client_receipt.display().to_string(),
            "release".into(),
        ]);
        let released = stdout_json(&run_json(&args, None));
        assert_eq!(released["world_ref"], world_ref);
        assert_eq!(released["disposition"], "released");

        let mut args = base();
        args.extend([
            "--receipt".into(),
            client_receipt.display().to_string(),
            "observe".into(),
        ]);
        let after_release = stdout_json(&run_json(&args, None));
        assert_eq!(after_release["world_ref"], world_ref);
        assert!(after_release["observations"]
            .as_array()
            .unwrap()
            .iter()
            .all(|observation| observation["detail"]["lifecycle"] == "released"));
    });

    server.serve_n(6).unwrap();
    client_thread.join().unwrap();
    let _ = fs::remove_dir_all(root);
}

#[test]
fn native_remote_cli_keeps_authentication_failure_distinct_and_accepts_env_token() {
    let root = temp_path("auth");
    let service = ControlService::new(local(&root)).with_authorization("remote-secret");
    let mut server = TcpControlServer::bind("127.0.0.1:0", service).unwrap();
    let endpoint = server.local_addr().unwrap().to_string();

    let client_endpoint = endpoint.clone();
    let client_thread = thread::spawn(move || {
        let args = vec![
            "--endpoint".into(),
            client_endpoint.clone(),
            "--json".into(),
            "discover".into(),
        ];
        let denied = run_json(&args, None);
        assert_eq!(denied.status.code(), Some(10));
        let denied_json: Value = serde_json::from_slice(&denied.stderr).unwrap();
        assert_eq!(denied_json["error"]["kind"], "authentication-failed");

        let allowed = stdout_json(&run_json(&args, Some("remote-secret")));
        assert_eq!(allowed["workcell_ref"], "workcell:remote-cli-test");
    });

    server.serve_n(2).unwrap();
    client_thread.join().unwrap();
    let _ = fs::remove_dir_all(root);
}

#[test]
fn remote_system_merges_remote_sections_under_remote_provenance_and_stays_honest_when_absent() {
    use std::sync::Arc;

    use serde_json::json;

    let root = temp_path("system-merge");
    let endpoint_root = root.join("endpoint-state");
    fs::create_dir_all(&endpoint_root).unwrap();

    // A serving cell that exposes the host-owned settings disclosure.
    let disclosure_root = endpoint_root.clone();
    let disclosed = ControlService::new(local(&endpoint_root)).with_system_disclosure(Arc::new(
        move || {
            Ok(json!({
                "schema": "oi.product-settings-disclosure/v2",
                "product_id": "workcell",
                "sections": [{
                    "id": "storage",
                    "title": "Storage / artifacts",
                    "settings": [{
                        "id": "storage.state_root",
                        "title": "Workcell state root",
                        "kind": "scalar",
                        "declared": {"path": disclosure_root.display().to_string()},
                        "effective": {"path": disclosure_root.display().to_string()},
                        "active": {"path": disclosure_root.display().to_string()},
                    }],
                }],
                "availability": {"state": "available", "reason": null},
                "degradations": [],
                "obligations": [],
                "owner": {"owner_ref": "workcell:remote-test"},
            }))
        },
    ));
    let mut server = TcpControlServer::bind("127.0.0.1:0", disclosed).unwrap();
    let endpoint = server.local_addr().unwrap().to_string();

    let state_root = root.join("local-state");
    fs::create_dir_all(&state_root).unwrap();
    let state_arg = state_root.display().to_string();
    let client_endpoint = endpoint.clone();
    let client_thread = thread::spawn(move || {
        let args = vec![
            "--endpoint".into(),
            client_endpoint.clone(),
            "--state-root".into(),
            state_arg.clone(),
            "--json".into(),
            "system".into(),
        ];
        let merged = stdout_json(&run_json(&args, None));
        assert_eq!(merged["schema"], "oi.product-settings-disclosure/v2");

        // Local sections remain, remote sections are merged under the
        // `remote:<endpoint>:` provenance header.
        let sections = merged["sections"].as_array().unwrap();
        let remote: Vec<&Value> = sections
            .iter()
            .filter(|section| {
                section["id"]
                    .as_str()
                    .unwrap_or("")
                    .starts_with(&format!("remote:{client_endpoint}:"))
            })
            .collect();
        assert!(
            !remote.is_empty(),
            "remote sections must be merged with remote: provenance"
        );
        assert_eq!(remote[0]["provenance"], format!("remote:{client_endpoint}"));
        assert!(
            sections.iter().any(|section| section["id"] == "storage"
                && !section["id"]
                    .as_str()
                    .unwrap_or("")
                    .starts_with("remote:")),
            "the local reading must survive the merge"
        );

        // The merge names the remote as available with its digest evidence.
        let degradation = merged["degradations"]
            .as_array()
            .unwrap()
            .iter()
            .find(|entry| entry["subject_ref"] == format!("remote:{client_endpoint}"))
            .unwrap_or_else(|| panic!("remote degradation entry missing"));
        assert_eq!(degradation["state"], "available");
    });
    server.serve_n(1).unwrap();
    client_thread.join().unwrap();

    // A serving cell WITHOUT a disclosure refuses `system`; the CLI stays
    // honest: an unavailable remote section, never a fabricated reading.
    let bare = ControlService::new(local(&root.join("bare-state")));
    let mut bare_server = TcpControlServer::bind("127.0.0.1:0", bare).unwrap();
    let bare_endpoint = bare_server.local_addr().unwrap().to_string();
    let bare_state = root.join("bare-client");
    fs::create_dir_all(&bare_state).unwrap();
    let bare_thread = thread::spawn(move || {
        let args = vec![
            "--endpoint".into(),
            bare_endpoint.clone(),
            "--state-root".into(),
            bare_state.display().to_string(),
            "--json".into(),
            "system".into(),
        ];
        let merged = stdout_json(&run_json(&args, None));
        let remote: Vec<&Value> = merged["sections"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|section| {
                section["id"]
                    .as_str()
                    .unwrap_or("")
                    .starts_with(&format!("remote:{bare_endpoint}"))
            })
            .collect();
        assert_eq!(remote.len(), 1, "exactly one honest remote section");
        let setting = &remote[0]["settings"][0];
        assert_eq!(setting["effective"]["state"], "unavailable");
        assert!(setting["effective"]["reason"]
            .as_str()
            .unwrap_or("")
            .contains("did not supply a settings disclosure"));
    });
    bare_server.serve_n(1).unwrap();
    bare_thread.join().unwrap();

    let _ = fs::remove_dir_all(root);
}
