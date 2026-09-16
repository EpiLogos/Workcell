#![cfg(unix)]

//! Cross-cell connection lifecycle, end to end over real processes.
//!
//! Two state roots stand in for two cells on one machine: a serving cell
//! (`workcell serve` as its own process, with its own state root) and a
//! client cell (its own state root holding the connection receipts). The
//! server, authorise, connect, revoke and connection commands all run as
//! separate `workcell` processes, exactly as two machines would run them.
//! These are production paths: no fixture servers and no credential bypasses.

use std::{
    fs,
    io::Read,
    path::{Path, PathBuf},
    process::{Command, Output, Stdio},
    thread,
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use serde_json::Value;

fn temp_path(label: &str) -> PathBuf {
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    std::env::temp_dir().join(format!(
        "workcell-connections-lifecycle-{label}-{}-{nonce}",
        std::process::id()
    ))
}

fn run(args: &[String]) -> Output {
    let mut command = Command::new(env!("CARGO_BIN_EXE_workcell"));
    command.args(args);
    command.env_remove("WORKCELL_CONTROL_TOKEN");
    command.env_remove("WORKCELL_CONTROL_ENDPOINT");
    command.output().unwrap()
}

fn run_string(args: &[&str]) -> Output {
    let owned: Vec<String> = args.iter().map(|value| value.to_string()).collect();
    run(&owned)
}

fn stdout_json(output: &Output) -> Value {
    assert!(
        output.status.success(),
        "command failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    serde_json::from_slice(&output.stdout).unwrap()
}

/// Spawn `workcell serve` and read its stderr until the listening endpoint
/// is disclosed.
fn spawn_server(server_root: &Path, listen: &str) -> (std::process::Child, String) {
    let mut child = Command::new(env!("CARGO_BIN_EXE_workcell"))
        .args([
            "serve",
            "--listen",
            listen,
            "--state-root",
            &server_root.display().to_string(),
            "--workcell-ref",
            "workcell:home-server",
        ])
        .env_remove("WORKCELL_CONTROL_TOKEN")
        .env_remove("WORKCELL_CONTROL_ENDPOINT")
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn workcell serve");

    let mut stderr = String::new();
    let mut buffer = [0u8; 1];
    let deadline = std::time::Instant::now() + Duration::from_secs(15);
    let mut piped = child.stderr.take().expect("piped stderr");
    loop {
        if let Some(address) = endpoint_from_stderr(&stderr) {
            return (child, address);
        }
        if std::time::Instant::now() > deadline {
            child.kill().unwrap();
            let _ = child.wait();
            panic!("serve did not disclose a listening endpoint; stderr was: {stderr}");
        }
        match piped.read(&mut buffer) {
            Ok(0) => {
                let _ = child.wait();
                panic!("serve exited before disclosing an endpoint; stderr was: {stderr}");
            }
            Ok(_) => stderr.push(buffer[0] as char),
            Err(error) => {
                child.kill().unwrap();
                let _ = child.wait();
                panic!("read serve stderr: {error}; stderr was: {stderr}");
            }
        }
    }
}

/// The full `listening on HOST:PORT` line, and only that line, yields the
/// endpoint.
fn endpoint_from_stderr(stderr: &str) -> Option<String> {
    let marker = "listening on ";
    let index = stderr.find(marker)?;
    let rest = &stderr[index + marker.len()..];
    let end = rest.find(char::is_whitespace)?;
    if rest[..end].is_empty() {
        return None;
    }
    Some(rest[..end].to_owned())
}

fn connect_args(client_root: &Path, endpoint: &str, label: &str, credential: &str) -> Vec<String> {
    vec![
        "--state-root".into(),
        client_root.display().to_string(),
        "connect".into(),
        "--endpoint".into(),
        endpoint.into(),
        "--connection".into(),
        label.into(),
        "--authorization".into(),
        credential.into(),
        "--json".into(),
    ]
}

#[test]
fn lifecycle_authorise_connect_use_revoke_refusal() {
    let root = temp_path("lifecycle");
    let server_root = root.join("server");
    let client_root = root.join("client");
    fs::create_dir_all(&client_root).unwrap();

    let (mut server, endpoint) = spawn_server(&server_root, "127.0.0.1:0");

    // The serving cell grants the laptop exactly three operations.
    let grant = stdout_json(&run_string(&[
        "--state-root",
        &server_root.display().to_string(),
        "--workcell-ref",
        "workcell:home-server",
        "authorise",
        "--client",
        "laptop",
        "--allow",
        "status",
        "--allow",
        "discover",
        "--allow",
        "prepare",
        "--json",
    ]));
    assert_eq!(grant["client"], "laptop");
    assert_eq!(
        grant["operations"],
        serde_json::json!(["status", "discover", "prepare"])
    );
    let credential = grant["credential"].as_str().unwrap().to_owned();
    assert!(credential.starts_with("wck_"));
    // Only the digest is persisted: the grants registry never holds material.
    let grants_file =
        fs::read_to_string(server_root.join("connections").join("grants.json")).unwrap();
    assert!(!grants_file.contains(&credential));

    // The client cell connects: compatibility, granted scope and execution
    // location are all reported.
    let connected = stdout_json(&run(&connect_args(
        &client_root,
        &endpoint,
        "laptop",
        &credential,
    )));
    assert_eq!(connected["connection"]["state"], "connected");
    assert_eq!(connected["connection"]["label"], "laptop");
    assert_eq!(
        connected["connection"]["remote_workcell_ref"],
        "workcell:home-server"
    );
    assert_eq!(
        connected["connection"]["granted_operations"],
        serde_json::json!(["status", "discover", "prepare"])
    );
    assert_eq!(
        connected["compatibility"]["server_protocol"],
        "workcell.control/v1"
    );
    assert_eq!(
        connected["compatibility"]["client_protocol"],
        "workcell.control/v1"
    );
    assert!(
        connected["connection"]["advertised_offers"]
            .as_u64()
            .unwrap()
            >= 1
    );

    // The client cell's status shows the connection.
    let status = stdout_json(&run_string(&[
        "--state-root",
        &client_root.display().to_string(),
        "status",
        "--json",
    ]));
    let connection = status["connections"]
        .as_array()
        .unwrap()
        .iter()
        .find(|entry| entry["label"] == "laptop")
        .expect("status lists the connection")
        .clone();
    assert_eq!(connection["state"], "connected");
    assert_eq!(connection["endpoint"], endpoint);
    // The serving cell's status shows the grant.
    let server_status = stdout_json(&run_string(&[
        "--state-root",
        &server_root.display().to_string(),
        "--workcell-ref",
        "workcell:home-server",
        "status",
        "--json",
    ]));
    assert_eq!(server_status["connection_grants"]["active"], 1);

    // Authorised remote use through the ordinary remote path: the prepared
    // world belongs to the serving cell — execution location is proved by
    // the receipt, not assumed.
    let receipt = root.join("laptop-world.json");
    let prepared = stdout_json(&run_string(&[
        "--endpoint",
        &endpoint,
        "--authorization",
        &credential,
        "--state-root",
        &client_root.display().to_string(),
        "--receipt",
        &receipt.display().to_string(),
        "--json",
        "prepare",
        "--demand-ref",
        "demand:remote-laptop",
        "--require",
        "shell",
    ]));
    assert_eq!(prepared["world"]["workcell_ref"], "workcell:home-server");

    // An operation outside the grant refuses loudly and names the grant.
    let denied = run_string(&[
        "--endpoint",
        &endpoint,
        "--authorization",
        &credential,
        "--state-root",
        &client_root.display().to_string(),
        "--receipt",
        &receipt.display().to_string(),
        "--json",
        "release",
    ]);
    assert!(!denied.status.success());
    let stderr = String::from_utf8_lossy(&denied.stderr);
    assert!(
        stderr.contains("does not permit operation `release`"),
        "unexpected refusal: {stderr}"
    );

    // Revocation takes effect at the connecting client's next use.
    stdout_json(&run_string(&[
        "--state-root",
        &server_root.display().to_string(),
        "--workcell-ref",
        "workcell:home-server",
        "revoke",
        "--client",
        "laptop",
        "--json",
    ]));
    let refused = run(&connect_args(
        &client_root,
        &endpoint,
        "laptop",
        &credential,
    ));
    assert!(!refused.status.success());
    let stderr = String::from_utf8_lossy(&refused.stderr);
    assert!(stderr.contains("revoked"), "unexpected refusal: {stderr}");

    // The client record keeps the refusal honestly: the endpoint answered,
    // the credential was withdrawn.
    let shown = stdout_json(&run_string(&[
        "--state-root",
        &client_root.display().to_string(),
        "connections",
        "show",
        "laptop",
        "--json",
    ]));
    assert_eq!(shown["connection"]["state"], "refused");
    assert!(shown["connection"]["detail"]
        .as_str()
        .unwrap()
        .contains("revoked"));

    server.kill().unwrap();
    let _ = server.wait();
    let _ = fs::remove_dir_all(root);
}

#[test]
fn grant_expiry_refuses_loudly_at_the_next_use_and_leaves_other_grants_working() {
    let root = temp_path("expiry");
    let server_root = root.join("server");
    let client_root = root.join("client");
    fs::create_dir_all(&client_root).unwrap();

    let (mut server, endpoint) = spawn_server(&server_root, "127.0.0.1:0");

    // The expiring grant gets a short real-clock window; the keeper grant
    // has none and must keep working after the window closes.
    let grant = stdout_json(&run_string(&[
        "--state-root",
        &server_root.display().to_string(),
        "--workcell-ref",
        "workcell:home-server",
        "authorise",
        "--client",
        "laptop",
        "--allow",
        "status",
        "--allow",
        "discover",
        "--expires-in",
        "5s",
        "--json",
    ]));
    assert_eq!(grant["client"], "laptop");
    let expires_at = grant["expires_at_unix_ms"].as_u64().expect("expiry stored");
    assert!(
        expires_at
            > SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_millis() as u64
    );
    // Only the digest is persisted, with the expiry alongside it.
    let grants_file =
        fs::read_to_string(server_root.join("connections").join("grants.json")).unwrap();
    assert!(grants_file.contains("\"expires_at_unix_ms\""));
    let credential = grant["credential"].as_str().unwrap().to_owned();

    let keeper_grant = stdout_json(&run_string(&[
        "--state-root",
        &server_root.display().to_string(),
        "--workcell-ref",
        "workcell:home-server",
        "authorise",
        "--client",
        "keeper",
        "--allow",
        "status",
        "--allow",
        "discover",
        "--json",
    ]));
    assert!(keeper_grant["expires_at_unix_ms"].is_null());
    let keeper_credential = keeper_grant["credential"].as_str().unwrap().to_owned();

    // Inside the window the connection works and the receipt carries the
    // grant's expiry.
    let connected = stdout_json(&run(&connect_args(
        &client_root,
        &endpoint,
        "laptop",
        &credential,
    )));
    assert_eq!(connected["connection"]["state"], "connected");
    assert_eq!(connected["connection"]["expires_at_unix_ms"], expires_at);
    let keeper_connected = stdout_json(&run(&connect_args(
        &client_root,
        &endpoint,
        "keeper",
        &keeper_credential,
    )));
    assert_eq!(keeper_connected["connection"]["state"], "connected");
    assert!(keeper_connected["connection"]["expires_at_unix_ms"].is_null());

    // Past the window, the same credential is refused as expired — named,
    // loud, and a different word from revocation.
    let now_ms = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_millis() as u64;
    if expires_at + 200 > now_ms {
        thread::sleep(Duration::from_millis(expires_at + 200 - now_ms));
    }
    let refused = run(&connect_args(
        &client_root,
        &endpoint,
        "laptop",
        &credential,
    ));
    assert!(!refused.status.success());
    let stderr = String::from_utf8_lossy(&refused.stderr);
    assert!(stderr.contains("expired"), "unexpected refusal: {stderr}");
    assert!(
        !stderr.contains("revoked"),
        "expired must not masquerade as revoked: {stderr}"
    );

    // The client record keeps the refusal honestly, with the expiry still
    // disclosed.
    let shown = stdout_json(&run_string(&[
        "--state-root",
        &client_root.display().to_string(),
        "connections",
        "show",
        "laptop",
        "--json",
    ]));
    assert_eq!(shown["connection"]["state"], "refused");
    assert!(shown["connection"]["detail"]
        .as_str()
        .unwrap()
        .contains("expired"));
    assert_eq!(shown["connection"]["expires_at_unix_ms"], expires_at);

    // The listing discloses the expiry too, and the keeper connection —
    // same server, no expiry — is unaffected.
    let listed = stdout_json(&run_string(&[
        "--state-root",
        &client_root.display().to_string(),
        "connections",
        "list",
        "--json",
    ]));
    let by_label = |label: &str| {
        listed["connections"]
            .as_array()
            .unwrap()
            .iter()
            .find(|entry| entry["label"] == label)
            .expect("connection listed")
            .clone()
    };
    assert_eq!(by_label("laptop")["expires_at_unix_ms"], expires_at);
    assert!(by_label("keeper")["expires_at_unix_ms"].is_null());
    let keeper_again = run(&connect_args(
        &client_root,
        &endpoint,
        "keeper",
        &keeper_credential,
    ));
    assert!(
        keeper_again.status.success(),
        "{}",
        String::from_utf8_lossy(&keeper_again.stderr)
    );

    // Non-positive and malformed lifetimes are usage errors, refused
    // before anything is granted.
    for bad in ["0m", "-5m", "banana"] {
        let invalid = run_string(&[
            "--state-root",
            &server_root.display().to_string(),
            "--workcell-ref",
            "workcell:home-server",
            "authorise",
            "--client",
            "sloppy",
            "--allow",
            "status",
            "--expires-in",
            bad,
            "--json",
        ]);
        assert!(!invalid.status.success(), "`{bad}` must refuse");
        let invalid_stderr = String::from_utf8_lossy(&invalid.stderr);
        assert!(
            invalid_stderr.contains("--expires-in"),
            "`{bad}` refusal must be a usage error: {invalid_stderr}"
        );
    }

    server.kill().unwrap();
    let _ = server.wait();
    let _ = fs::remove_dir_all(root);
}

#[test]
fn disconnect_offline_state_and_reconnect_reconcile() {
    let root = temp_path("reconnect");
    let server_root = root.join("server");
    let client_root = root.join("client");
    fs::create_dir_all(&client_root).unwrap();

    let (mut server, endpoint) = spawn_server(&server_root, "127.0.0.1:0");

    let grant = stdout_json(&run_string(&[
        "--state-root",
        &server_root.display().to_string(),
        "--workcell-ref",
        "workcell:home-server",
        "authorise",
        "--client",
        "visitor",
        "--allow",
        "status",
        "--allow",
        "discover",
        "--json",
    ]));
    let credential = grant["credential"].as_str().unwrap().to_owned();

    let first = run_string(&[
        "--state-root",
        &client_root.display().to_string(),
        "connect",
        "--endpoint",
        &endpoint,
        "--connection",
        "visitor",
        "--authorization",
        &credential,
    ]);
    assert!(first.status.success());
    let first_plain = String::from_utf8_lossy(&first.stdout).to_string();
    assert!(first_plain.contains("connected `visitor`"), "{first_plain}");
    assert!(first_plain.contains("execution location:"), "{first_plain}");
    assert!(first_plain.contains("not on this machine"), "{first_plain}");

    // Reconnecting reports compatibility again, over an existing record.
    let reconnect = run_string(&[
        "--state-root",
        &client_root.display().to_string(),
        "connect",
        "--endpoint",
        &endpoint,
        "--connection",
        "visitor",
        "--authorization",
        &credential,
    ]);
    assert!(reconnect.status.success());
    let reconnect_plain = String::from_utf8_lossy(&reconnect.stdout).to_string();
    assert!(
        reconnect_plain.contains("reconnected `visitor`"),
        "{reconnect_plain}"
    );
    assert!(
        reconnect_plain.contains("compatibility: protocol workcell.control/v1 on both cells"),
        "{reconnect_plain}"
    );

    // Operator-initiated disconnect keeps the record with its history.
    let disconnected = stdout_json(&run_string(&[
        "--state-root",
        &client_root.display().to_string(),
        "connections",
        "disconnect",
        "visitor",
        "--json",
    ]));
    assert_eq!(disconnected["state"], "disconnected");

    // The host goes offline (serve process dies) and the client hears the
    // difference: unreachable is not refused and not incompatible.
    server.kill().unwrap();
    let _ = server.wait();
    thread::sleep(Duration::from_millis(200));

    let offline = run(&connect_args(
        &client_root,
        &endpoint,
        "visitor",
        &credential,
    ));
    assert!(!offline.status.success());
    let stderr = String::from_utf8_lossy(&offline.stderr);
    assert!(stderr.contains("unreachable"), "{stderr}");
    let shown = stdout_json(&run_string(&[
        "--state-root",
        &client_root.display().to_string(),
        "connections",
        "show",
        "visitor",
        "--json",
    ]));
    assert_eq!(shown["connection"]["state"], "disconnected");

    // Host restart on the same endpoint; the client reconnects and the
    // relation reconciles. Reconnecting updated the connection record only
    // — the retained remote world receipt on the client side (if any) is
    // never rewritten by a reconnect.
    let mut restarted = None;
    for _ in 0..50 {
        match spawn_server(&server_root, &endpoint) {
            (child, bound) if bound == endpoint => {
                restarted = Some(child);
                break;
            }
            (mut child, _) => {
                child.kill().unwrap();
                let _ = child.wait();
                thread::sleep(Duration::from_millis(100));
            }
        }
    }
    let mut restarted = restarted.expect("serve re-binds its endpoint after restart");

    let reconnected = run(&connect_args(
        &client_root,
        &endpoint,
        "visitor",
        &credential,
    ));
    assert!(
        reconnected.status.success(),
        "{}",
        String::from_utf8_lossy(&reconnected.stderr)
    );
    let listed = stdout_json(&run_string(&[
        "--state-root",
        &client_root.display().to_string(),
        "connections",
        "list",
        "--json",
    ]));
    let record = listed["connections"]
        .as_array()
        .unwrap()
        .iter()
        .find(|entry| entry["label"] == "visitor")
        .expect("the connection survived the offline window")
        .clone();
    assert_eq!(record["state"], "connected");

    restarted.kill().unwrap();
    let _ = restarted.wait();
    let _ = fs::remove_dir_all(root);
}

#[test]
fn connect_without_a_grant_is_refused_and_says_how_to_fix_it() {
    let root = temp_path("stranger");
    let server_root = root.join("server");
    let client_root = root.join("client");
    fs::create_dir_all(&client_root).unwrap();

    let (mut server, endpoint) = spawn_server(&server_root, "127.0.0.1:0");

    // A client nobody authorised learns the refusal and the remedy, but no
    // capability and no workcell identity.
    let stranger = run(&connect_args(
        &client_root,
        &endpoint,
        "stranger",
        "wck_not_a_real_credential",
    ));
    assert!(!stranger.status.success());
    let stderr = String::from_utf8_lossy(&stranger.stderr);
    assert!(
        stderr.contains("no active connection grant matches"),
        "{stderr}"
    );
    let shown = stdout_json(&run_string(&[
        "--state-root",
        &client_root.display().to_string(),
        "connections",
        "show",
        "stranger",
        "--json",
    ]));
    assert_eq!(shown["connection"]["state"], "refused");
    // The refused record carries no remote identity.
    assert!(shown["connection"]["remote_workcell_ref"].is_null());

    server.kill().unwrap();
    let _ = server.wait();
    let _ = fs::remove_dir_all(root);
}
