#![cfg(unix)]

//! `workcell placement` over real processes: this cell and a connected
//! serving cell (two state roots on loopback, as connections_lifecycle does),
//! the remote discovered through its grant and read back by the control
//! codec, then ranked by the placement law. Nothing is prepared, and no
//! machine beyond loopback is touched.

use std::{
    fs,
    io::Read,
    path::{Path, PathBuf},
    process::{Child, Command, Output, Stdio},
    thread,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

use serde_json::Value;

fn temp_path(label: &str) -> PathBuf {
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    std::env::temp_dir().join(format!(
        "workcell-placement-{label}-{}-{nonce}",
        std::process::id()
    ))
}

fn run(args: &[&str], token: Option<&str>) -> Output {
    let mut command = Command::new(env!("CARGO_BIN_EXE_workcell"));
    command.args(args);
    command.env_remove("WORKCELL_CONTROL_TOKEN");
    command.env_remove("WORKCELL_CONTROL_ENDPOINT");
    if let Some(token) = token {
        command.env("WORKCELL_CONTROL_TOKEN", token);
    }
    command.output().unwrap()
}

fn json(output: &Output) -> Value {
    assert!(
        output.status.success(),
        "command failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    serde_json::from_slice(&output.stdout).unwrap()
}

/// The serving cell, stopped when the test ends however it ends.
struct Server(Child);
impl Drop for Server {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

fn serve(root: &Path) -> (Server, String) {
    let mut child = Command::new(env!("CARGO_BIN_EXE_workcell"))
        .args(["serve", "--listen", "127.0.0.1:0", "--state-root"])
        .arg(root)
        .args(["--workcell-ref", "workcell:home-server"])
        .env_remove("WORKCELL_CONTROL_TOKEN")
        .env_remove("WORKCELL_CONTROL_ENDPOINT")
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let mut piped = child.stderr.take().unwrap();
    let server = Server(child);
    let mut stderr = String::new();
    let mut byte = [0u8; 1];
    let deadline = Instant::now() + Duration::from_secs(15);
    loop {
        if let Some(index) = stderr.find("listening on ") {
            let rest = &stderr[index + "listening on ".len()..];
            if let Some(end) = rest.find(char::is_whitespace) {
                let endpoint = rest[..end].to_owned();
                thread::spawn(move || {
                    let mut sink = [0u8; 1024];
                    while let Ok(read) = piped.read(&mut sink) {
                        if read == 0 {
                            break;
                        }
                    }
                });
                return (server, endpoint);
            }
        }
        assert!(
            Instant::now() < deadline,
            "serve disclosed no endpoint: {stderr}"
        );
        match piped.read(&mut byte) {
            Ok(0) => panic!("serve exited: {stderr}"),
            Ok(_) => stderr.push(byte[0] as char),
            Err(error) => panic!("read serve stderr: {error}"),
        }
    }
}

fn authorise(server_root: &Path, client: &str, operations: &[&str]) -> String {
    let root = server_root.display().to_string();
    let mut args = vec![
        "--state-root",
        &root,
        "--workcell-ref",
        "workcell:home-server",
        "authorise",
        "--client",
        client,
    ];
    for operation in operations {
        args.extend(["--allow", operation]);
    }
    args.push("--json");
    json(&run(&args, None))["credential"]
        .as_str()
        .unwrap()
        .to_owned()
}

fn connect(client_root: &Path, endpoint: &str, label: &str, credential: &str) {
    let root = client_root.display().to_string();
    json(&run(
        &[
            "--state-root",
            &root,
            "connect",
            "--endpoint",
            endpoint,
            "--connection",
            label,
            "--authorization",
            credential,
            "--json",
        ],
        None,
    ));
}

fn placement(client_root: &Path, extra: &[&str], token: Option<&str>) -> Output {
    let root = client_root.display().to_string();
    let mut args = vec![
        "--state-root",
        &root,
        "--workcell-ref",
        "workcell:laptop",
        "--json",
        "placement",
        "--demand-ref",
        "demand:placement",
        "--require",
        "shell",
    ];
    args.extend(extra.iter().copied());
    run(&args, token)
}

fn eligible_refs(reading: &Value) -> Vec<String> {
    reading["eligible"]
        .as_array()
        .unwrap()
        .iter()
        .map(|decision| decision["workcell_ref"].as_str().unwrap().to_owned())
        .collect()
}

#[test]
fn this_cell_and_a_connected_cell_are_planned_and_ranked() {
    let root = temp_path("ranked");
    let server_root = root.join("server");
    let client_root = root.join("client");
    fs::create_dir_all(&client_root).unwrap();

    // Alone, this cell is the only place.
    let alone = json(&placement(&client_root, &[], None));
    assert_eq!(alone["schema"], "workcell.placement-evaluation/v1");
    assert_eq!(eligible_refs(&alone), ["workcell:laptop"]);
    assert_eq!(alone["selected"], "workcell:laptop");
    assert_eq!(alone["eligible"][0]["provenance"]["locality_cost"], 0);
    assert_eq!(
        alone["eligible"][0]["provenance"]["transport"]["transport"],
        "in-process"
    );

    let (server, endpoint) = serve(&server_root);
    let credential = authorise(&server_root, "laptop", &["status", "discover"]);
    connect(&client_root, &endpoint, "laptop", &credential);
    let viewer = authorise(&server_root, "viewer", &["status"]);
    connect(&client_root, &endpoint, "viewer", &viewer);

    // Both cells carry it; the remote is reached through its grant.
    let both = json(&placement(&client_root, &[], Some(&credential)));
    let refs = eligible_refs(&both);
    assert!(refs.contains(&"workcell:laptop".to_owned()), "{both}");
    assert!(refs.contains(&"workcell:home-server".to_owned()), "{both}");
    let remote = both["eligible"]
        .as_array()
        .unwrap()
        .iter()
        .find(|decision| decision["workcell_ref"] == "workcell:home-server")
        .unwrap();
    assert_eq!(remote["provenance"]["source_ref"], "connection:laptop");
    assert_eq!(remote["provenance"]["locality_cost"], 1);
    assert_eq!(remote["provenance"]["transport"]["endpoint"], endpoint);
    assert_eq!(remote["plan"]["status"], "satisfiable");
    // A connection whose grant does not allow discover is skipped, said why.
    let skipped = both["skipped"].as_array().unwrap();
    assert_eq!(skipped.len(), 1, "{both}");
    assert_eq!(skipped[0]["connection"], "viewer");
    assert!(skipped[0]["reason"].as_str().unwrap().contains("discover"));

    // Locality first chooses this cell; a locality ceiling of 0 rejects the remote.
    let local_first = json(&placement(
        &client_root,
        &["--prefer-locality"],
        Some(&credential),
    ));
    assert_eq!(local_first["selected"], "workcell:laptop");
    let near = json(&placement(
        &client_root,
        &["--max-locality-cost", "0"],
        Some(&credential),
    ));
    assert_eq!(eligible_refs(&near), ["workcell:laptop"]);
    assert!(near["diagnostics"]
        .as_array()
        .unwrap()
        .iter()
        .any(|d| d["source_ref"] == "connection:laptop" && d["kind"] == "policy-rejected"));

    // Without the credential the remote refuses discover: named, not dropped.
    let unauthorised = json(&placement(&client_root, &["--connection", "laptop"], None));
    assert_eq!(eligible_refs(&unauthorised), ["workcell:laptop"]);
    let diagnostic = &unauthorised["diagnostics"][0];
    assert_eq!(diagnostic["source_ref"], "connection:laptop");
    assert_eq!(diagnostic["kind"], "transport-unavailable");
    assert!(
        diagnostic["detail"]
            .as_str()
            .unwrap()
            .contains("refused discover"),
        "{diagnostic}"
    );

    // The serving cell gone: unreachable, this cell still carries it.
    drop(server);
    let gone = json(&placement(
        &client_root,
        &["--connection", "laptop"],
        Some(&credential),
    ));
    assert_eq!(eligible_refs(&gone), ["workcell:laptop"]);
    assert!(gone["diagnostics"][0]["detail"]
        .as_str()
        .unwrap()
        .contains("unreachable"));

    // A connection that does not exist is refused before anything is asked.
    assert!(!placement(&client_root, &["--connection", "nobody"], None)
        .status
        .success());
    let _ = fs::remove_dir_all(&root);
}
