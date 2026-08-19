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
