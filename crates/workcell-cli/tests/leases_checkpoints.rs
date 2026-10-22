//! `workcell --receipt R lease | checkpoint | checkpoints`: a prepared world's
//! lifetime as its providers hold it. Every test runs on a scratch state root;
//! the sandbox cases answer from a stub lifecycle server on loopback, never a
//! real one.

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

fn temp_path(label: &str) -> PathBuf {
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    std::env::temp_dir().join(format!(
        "epilogos-workcell-leases-{label}-{}-{nonce}",
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

fn json_of(output: &Output) -> Value {
    serde_json::from_slice(&output.stdout).unwrap_or_else(|error| {
        panic!(
            "invalid JSON stdout: {error}\nstdout={}\nstderr={}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        )
    })
}

/// A refusal: a failed exit with the error document on stderr.
fn refusal(output: &Output) -> Value {
    assert!(!output.status.success(), "expected a refusal");
    serde_json::from_slice(&output.stderr).unwrap_or_else(|error| {
        panic!(
            "invalid JSON stderr: {error}\nstderr={}",
            String::from_utf8_lossy(&output.stderr)
        )
    })
}

fn ok_json(output: &Output) -> Value {
    assert!(
        output.status.success(),
        "stderr={}\nstdout={}",
        String::from_utf8_lossy(&output.stderr),
        String::from_utf8_lossy(&output.stdout)
    );
    json_of(output)
}

/// A world prepared on this cell's host process; its receipt path.
fn prepared_world(state: &Path) -> PathBuf {
    let prepared = ok_json(&run(&[
        "--state-root",
        path_arg(state),
        "--json",
        "prepare",
        "--demand-ref",
        "demand:leases-test",
        "--require",
        "shell",
    ]));
    PathBuf::from(prepared["receipt"].as_str().unwrap())
}

fn world_op(state: &Path, receipt: &Path, args: &[&str]) -> Output {
    let mut full = vec![
        "--state-root",
        path_arg(state),
        "--receipt",
        path_arg(receipt),
        "--json",
    ];
    full.extend_from_slice(args);
    run(&full)
}

/// A stub OpenSandbox lifecycle server on loopback: it answers each request in
/// turn with the next (status, body) and hands back the request lines it saw.
fn lifecycle_stub(answers: Vec<(u16, Value)>) -> (String, JoinHandle<Vec<String>>) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let base = format!("http://{}/v1", listener.local_addr().unwrap());
    let peer = std::thread::spawn(move || {
        let mut seen = Vec::new();
        for (status, body) in answers {
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
            let length = head
                .lines()
                .find_map(|line| {
                    let (name, value) = line.split_once(':')?;
                    name.eq_ignore_ascii_case("content-length")
                        .then(|| value.trim().parse::<usize>().ok())?
                })
                .unwrap_or(0);
            let mut request_body = vec![0u8; length];
            stream.read_exact(&mut request_body).unwrap();
            seen.push(format!(
                "{} {}",
                head.lines().next().unwrap_or_default(),
                String::from_utf8_lossy(&request_body)
            ));
            let payload = body.to_string();
            write!(
                stream,
                "HTTP/1.1 {status} OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{payload}",
                payload.len()
            )
            .unwrap();
        }
        seen
    });
    (base, peer)
}

/// The prepared world's one execution binding rewritten as an OpenSandbox
/// allocation `sb-1`, with that deployment declared in the state root.
fn sandbox_world(state: &Path, lifecycle_base_url: &str) -> PathBuf {
    let local = prepared_world(state);
    let mut world: Value = serde_json::from_str(&fs::read_to_string(&local).unwrap()).unwrap();
    let binding = &mut world["binding_graph"]["bindings"][0];
    assert_eq!(binding["port"], "execution");
    binding["provider_ref"] = json!("provider:opensandbox");
    binding["offer_ref"] = json!("offer:provider:opensandbox:sandbox");
    binding["material_ref"] = json!("sb-1");
    binding["properties"] = json!({});
    binding["provenance"] = json!({"provider_ref": "provider:opensandbox"});
    let receipt = state.join("sandbox-world.json");
    fs::write(&receipt, serde_json::to_vec_pretty(&world).unwrap()).unwrap();
    fs::write(
        state.join("services.json"),
        json!({"schema": "workcell.service-declaration/v1", "execution": [{"kind": "opensandbox",
            "lifecycle_base_url": lifecycle_base_url, "api_key_env": null}]})
        .to_string(),
    )
    .unwrap();
    receipt
}

#[test]
fn a_host_world_holds_no_lease_and_takes_no_checkpoint() {
    let state = temp_path("host");
    let receipt = prepared_world(&state);

    let leases = ok_json(&world_op(&state, &receipt, &["lease"]));
    assert_eq!(leases["schema"], "workcell.material-leases/v1");
    let lease = &leases["leases"][0];
    assert_eq!(lease["logical_ref"], "affordance:shell");
    assert_eq!(lease["port"], "execution");
    assert_eq!(lease["presence"], "present");
    assert_eq!(lease["standing"], "no-lease");
    assert!(lease.get("expires_at").is_none());

    let refused = refusal(&world_op(&state, &receipt, &["checkpoint"]));
    assert_eq!(refused["error"]["kind"], "unsupported");
    assert!(refused["error"]["message"]
        .as_str()
        .unwrap()
        .contains("held by a provider that takes checkpoints"));
    let named = refusal(&world_op(
        &state,
        &receipt,
        &["checkpoint", "--binding", "affordance:shell"],
    ));
    assert!(named["error"]["message"]
        .as_str()
        .unwrap()
        .contains("provider:collapsed-local-host-process"));
    let unknown = refusal(&world_op(
        &state,
        &receipt,
        &["checkpoint", "--binding", "affordance:none"],
    ));
    assert_eq!(unknown["error"]["kind"], "not-found");

    let kept = ok_json(&world_op(&state, &receipt, &["checkpoints"]));
    assert_eq!(kept["schema"], "workcell.material-checkpoints/v1");
    assert_eq!(kept["checkpoints"], json!([]));
    assert!(
        !state.join("checkpoints").exists(),
        "reading the ledger creates nothing"
    );
    assert!(!world_op(&state, &receipt, &["lease", "--renew"])
        .status
        .success());
    let _ = fs::remove_dir_all(state);
}

#[test]
fn a_sandbox_world_reads_its_lease_and_keeps_the_checkpoint_it_takes() {
    let state = temp_path("sandbox");
    fs::create_dir_all(&state).unwrap();
    let (base, peer) = lifecycle_stub(vec![
        (
            200,
            json!({"id": "sb-1", "status": {"state": "Running"}, "expiresAt": "2026-10-22T12:00:00Z"}),
        ),
        (200, json!({"id": "snap-1", "status": {"state": "Ready"}})),
        (200, json!({"id": "sb-1", "status": {"state": "Running"}})),
    ]);
    let receipt = sandbox_world(&state, &base);

    let leases = ok_json(&world_op(&state, &receipt, &["lease"]));
    let lease = &leases["leases"][0];
    assert_eq!(lease["provider_ref"], "provider:opensandbox");
    assert_eq!(lease["material_ref"], "sb-1");
    assert_eq!(lease["standing"], "leased");
    assert_eq!(lease["expires_at"], "2026-10-22T12:00:00Z");
    assert_eq!(lease["renewable"], true);

    let taken = ok_json(&world_op(
        &state,
        &receipt,
        &["checkpoint", "--name", "before"],
    ));
    assert_eq!(taken["schema"], "workcell.material-checkpoint/v1");
    assert_eq!(taken["checkpoint_ref"], "snap-1");
    assert_eq!(taken["source_material_ref"], "sb-1");
    assert_eq!(taken["logical_ref"], "affordance:shell");
    assert_eq!(taken["state"], "ready");
    assert_eq!(taken["reusable"], true);
    assert_eq!(taken["name"], "before");

    let kept = ok_json(&world_op(&state, &receipt, &["checkpoints"]));
    let checkpoints = kept["checkpoints"].as_array().unwrap();
    assert_eq!(checkpoints.len(), 1);
    assert_eq!(checkpoints[0]["checkpoint_ref"], "snap-1");
    assert_eq!(checkpoints[0]["world_ref"], taken["world_ref"]);
    assert_eq!(kept["unreadable"], json!([]));

    // A sandbox whose server reports no expiry is unexpiring, not leased.
    let leases = ok_json(&world_op(&state, &receipt, &["lease"]));
    assert_eq!(leases["leases"][0]["standing"], "unexpiring");

    let seen = peer.join().unwrap();
    assert!(seen[0].starts_with("GET /v1/sandboxes/sb-1 "), "{seen:?}");
    assert!(
        seen[1].starts_with("POST /v1/sandboxes/sb-1/snapshots ")
            && seen[1].ends_with(r#"{"name":"before"}"#),
        "{seen:?}"
    );

    // A torn ledger line is named, and the good record still reads.
    let ledger = PathBuf::from(kept["ledger"].as_str().unwrap());
    let mut torn = fs::read_to_string(&ledger).unwrap();
    torn.push_str("{not json\n");
    fs::write(&ledger, torn).unwrap();
    let kept = ok_json(&world_op(&state, &receipt, &["checkpoints"]));
    assert_eq!(kept["checkpoints"].as_array().unwrap().len(), 1);
    assert_eq!(kept["unreadable"][0]["line"], 2);
    let _ = fs::remove_dir_all(state);
}

#[test]
fn an_unreachable_lease_provider_is_named_with_its_error() {
    let state = temp_path("unreachable");
    fs::create_dir_all(&state).unwrap();
    // Nothing listens here once the listener is dropped.
    let base = {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        format!("http://{}/v1", listener.local_addr().unwrap())
    };
    let receipt = sandbox_world(&state, &base);
    let leases = ok_json(&world_op(&state, &receipt, &["lease"]));
    let lease = &leases["leases"][0];
    assert_eq!(lease["standing"], "unobserved");
    assert!(!lease["error"].as_str().unwrap().is_empty());
    let _ = fs::remove_dir_all(state);
}
