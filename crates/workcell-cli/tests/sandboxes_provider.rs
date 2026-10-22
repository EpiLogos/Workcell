//! `workcell sandboxes reconcile --provider <ref>`: the lifecycle server and
//! its key environment come from a deployment this cell declares, so a key
//! only ever reaches Workcell's own declared server; and `workcell doctor`
//! names the declared deployments a caller can pass. Every test runs on a
//! scratch state root against a loopback stub, never a real server.

use std::{
    fs,
    io::{Read, Write},
    net::TcpListener,
    path::{Path, PathBuf},
    process::{Command, Output},
    thread::JoinHandle,
    time::{SystemTime, UNIX_EPOCH},
};

use serde_json::{json, Value};

const KEY_ENV: &str = "WORKCELL_TEST_DECLARED_SANDBOX_KEY";

fn temp_path(label: &str) -> PathBuf {
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    std::env::temp_dir().join(format!(
        "epilogos-workcell-sandboxes-{label}-{}-{nonce}",
        std::process::id()
    ))
}

/// The CLI with no Actuation on PATH (doctor's instance scan then reports
/// itself unavailable instead of probing this machine) and the given key
/// environment.
fn run(args: &[&str], key: Option<&str>) -> Output {
    let mut command = Command::new(env!("CARGO_BIN_EXE_workcell"));
    command
        .args(args)
        .env("PATH", "/usr/bin:/bin")
        .env_remove(KEY_ENV);
    if let Some(key) = key {
        command.env(KEY_ENV, key);
    }
    command.output().unwrap()
}

fn path_arg(path: &Path) -> &str {
    path.to_str().unwrap()
}

fn ok_json(output: &Output) -> Value {
    assert!(
        output.status.success(),
        "stderr={}\nstdout={}",
        String::from_utf8_lossy(&output.stderr),
        String::from_utf8_lossy(&output.stdout)
    );
    serde_json::from_slice(&output.stdout).unwrap()
}

fn refusal(output: &Output) -> Value {
    assert!(!output.status.success(), "expected a refusal");
    serde_json::from_slice(&output.stderr).unwrap_or_else(|error| {
        panic!(
            "invalid JSON stderr: {error}\nstderr={}",
            String::from_utf8_lossy(&output.stderr)
        )
    })
}

fn declare(state: &Path, lifecycle_base_url: &str) {
    fs::create_dir_all(state).unwrap();
    fs::write(
        state.join("services.json"),
        json!({"schema": "workcell.service-declaration/v1", "execution": [{"kind": "opensandbox",
            "lifecycle_base_url": lifecycle_base_url, "api_key_env": KEY_ENV}]})
        .to_string(),
    )
    .unwrap();
}

/// A loopback lifecycle server answering one sandbox listing; it hands back
/// the request line and the key header it received.
fn lifecycle_stub() -> (String, JoinHandle<(String, Option<String>)>) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let base = format!("http://{}/v1", listener.local_addr().unwrap());
    let peer = std::thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        let mut head = Vec::new();
        let mut byte = [0u8; 1];
        while !head.ends_with(b"\r\n\r\n") {
            if stream.read(&mut byte).unwrap() == 0 {
                break;
            }
            head.push(byte[0]);
        }
        let head = String::from_utf8_lossy(&head).into_owned();
        let key = head.lines().find_map(|line| {
            let (name, value) = line.split_once(':')?;
            name.eq_ignore_ascii_case("open-sandbox-api-key")
                .then(|| value.trim().to_owned())
        });
        let payload = json!({"items": [{"id": "sb-1", "status": {"state": "Running"},
            "expiresAt": "2099-01-01T00:00:00Z"}]})
        .to_string();
        write!(
            stream,
            "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{payload}",
            payload.len()
        )
        .unwrap();
        (head.lines().next().unwrap_or_default().to_owned(), key)
    });
    (base, peer)
}

#[test]
fn doctor_names_the_declared_sandbox_deployments() {
    let bare = temp_path("doctor-bare");
    let doctor = ok_json(&run(
        &["--state-root", path_arg(&bare), "--json", "doctor"],
        None,
    ));
    assert_eq!(doctor["declared_sandbox_deployments"], json!([]));

    let declared = temp_path("doctor-declared");
    declare(&declared, "http://127.0.0.1:9/v1");
    let doctor = ok_json(&run(
        &["--state-root", path_arg(&declared), "--json", "doctor"],
        Some("never-read-by-doctor"),
    ));
    assert_eq!(
        doctor["declared_sandbox_deployments"],
        json!([{"provider_ref": "provider:opensandbox", "kind": "opensandbox",
            "lifecycle_base_url": "http://127.0.0.1:9/v1", "api_key_env": KEY_ENV}])
    );
    assert!(
        !String::from_utf8_lossy(
            &run(
                &["--state-root", path_arg(&declared), "--json", "doctor"],
                Some("never-read-by-doctor")
            )
            .stdout
        )
        .contains("never-read-by-doctor"),
        "doctor names the key environment, never its value"
    );
    let _ = fs::remove_dir_all(bare);
    let _ = fs::remove_dir_all(declared);
}

#[test]
fn provider_reconciles_the_declared_server_with_the_declared_key() {
    let state = temp_path("provider");
    let (base, peer) = lifecycle_stub();
    declare(&state, &base);
    let report = ok_json(&run(
        &[
            "--state-root",
            path_arg(&state),
            "--json",
            "sandboxes",
            "reconcile",
            "--provider",
            "provider:opensandbox",
        ],
        Some("stub-only-marker"),
    ));
    assert_eq!(report["provider_ref"], "provider:opensandbox");
    assert_eq!(report["server"], base);
    assert_eq!(report["release_orphans"], false);
    assert_eq!(report["sandboxes"][0]["id"], "sb-1");
    assert_eq!(report["sandboxes"][0]["lease"], "live");
    let (request, key) = peer.join().unwrap();
    assert!(request.starts_with("GET /v1/sandboxes?page=1"), "{request}");
    assert_eq!(key.as_deref(), Some("stub-only-marker"));
    let _ = fs::remove_dir_all(state);
}

#[test]
fn provider_refuses_what_the_cell_does_not_declare_and_mixed_flags() {
    let state = temp_path("refusals");
    declare(&state, "http://127.0.0.1:9/v1");
    let root = path_arg(&state);

    let undeclared = refusal(&run(
        &[
            "--state-root",
            root,
            "--json",
            "sandboxes",
            "reconcile",
            "--provider",
            "provider:other",
        ],
        None,
    ));
    assert_eq!(undeclared["error"]["kind"], "unavailable");
    let message = undeclared["error"]["message"].as_str().unwrap();
    assert!(message.contains("no OpenSandbox deployment `provider:other` is declared"));
    assert!(
        message.contains("declared: `provider:opensandbox`"),
        "{message}"
    );

    let empty = temp_path("refusals-empty");
    let none = refusal(&run(
        &[
            "--state-root",
            path_arg(&empty),
            "--json",
            "sandboxes",
            "reconcile",
            "--provider",
            "provider:opensandbox",
        ],
        None,
    ));
    assert!(none["error"]["message"]
        .as_str()
        .unwrap()
        .contains("declared: none"));

    let both = refusal(&run(
        &[
            "--state-root",
            root,
            "--json",
            "sandboxes",
            "reconcile",
            "--provider",
            "provider:opensandbox",
            "--server",
            "http://127.0.0.1:9/v1",
        ],
        None,
    ));
    assert!(both["error"]["message"]
        .as_str()
        .unwrap()
        .contains("pass --server or --provider, not both"));

    let key_env = refusal(&run(
        &[
            "--state-root",
            root,
            "--json",
            "sandboxes",
            "reconcile",
            "--provider",
            "provider:opensandbox",
            "--api-key-env",
            "ELSEWHERE",
        ],
        None,
    ));
    assert!(key_env["error"]["message"]
        .as_str()
        .unwrap()
        .contains("drop --api-key-env"));
    let _ = fs::remove_dir_all(state);
    let _ = fs::remove_dir_all(empty);
}
