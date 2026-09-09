//! Logical service connectivity through the programs Workcell actually ships.
//!
//! `crates/workcell-runtime` already proves the service lifecycle in-process.
//! These tests exercise the same lifecycle through the `workcell` binary and
//! through the Control Service composition, because a provider that only exists
//! inside the runtime's own tests cannot satisfy anyone's demand on a real
//! machine.
//!
//! Nothing here installs, mentions or assumes a model-serving engine. The
//! declared service is this test binary re-invoked as a socket listener, so what
//! is being proved is the material contract — offer, plan, start, readiness,
//! observation, release — and not the presence of any particular vendor.

#![cfg(unix)]

use std::{
    env, fs,
    io::ErrorKind,
    net::{TcpListener, TcpStream},
    path::{Path, PathBuf},
    process::{Command, Output},
    thread,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

use epilogos_workcell_cli::DurableCollapsedLocalWorkcell;
use epilogos_workcell_control::{ControlService, TcpControlServer};
use epilogos_workcell_core::WorkcellRef;
use epilogos_workcell_runtime::CollapsedLocalConfig;
use serde_json::{json, Value};

const LISTEN_CHILD_ENV: &str = "WORKCELL_CLI_SERVICE_CHILD";
const LISTEN_ADDR_ENV: &str = "WORKCELL_CLI_SERVICE_CHILD_ADDR";
const PROBE_ADDR_ENV: &str = "WORKCELL_CLI_SERVICE_PROBE_ADDR";

/// The logical service ref a caller owns. Workcell never parses it.
const LOGICAL_INFERENCE_SERVICE: &str = "inference:caller-owned-service";

fn binary() -> &'static str {
    env!("CARGO_BIN_EXE_workcell")
}

fn temp_path(label: &str) -> PathBuf {
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    env::temp_dir().join(format!(
        "epilogos-workcell-service-cli-{label}-{}-{nonce}",
        std::process::id()
    ))
}

fn path_arg(path: &Path) -> &str {
    path.to_str().unwrap()
}

fn run(args: &[&str]) -> Output {
    Command::new(binary())
        .args(args)
        .env_remove("WORKCELL_CONTROL_ENDPOINT")
        .env_remove("WORKCELL_CONTROL_TOKEN")
        .output()
        .unwrap()
}

fn json_stdout(output: &Output) -> Value {
    serde_json::from_slice(&output.stdout).unwrap_or_else(|error| {
        panic!(
            "invalid JSON stdout: {error}\nstdout={}\nstderr={}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        )
    })
}

fn free_port() -> u16 {
    TcpListener::bind(("127.0.0.1", 0))
        .unwrap()
        .local_addr()
        .unwrap()
        .port()
}

fn write_declaration(state_root: &Path, services: Value) -> PathBuf {
    fs::create_dir_all(state_root).unwrap();
    let path = state_root.join("services.json");
    fs::write(
        &path,
        serde_json::to_vec_pretty(&json!({
            "schema": "workcell.service-declaration/v1",
            "services": services,
        }))
        .unwrap(),
    )
    .unwrap();
    path
}

/// A declared service whose process this Workcell process starts and owns.
fn provider_process_scoped_declaration(port: u16) -> Value {
    json!([{
        "logical_ref": LOGICAL_INFERENCE_SERVICE,
        "lifetime": "provider-process-scoped",
        "endpoint": format!("http://127.0.0.1:{port}"),
        "program": env::current_exe().unwrap().display().to_string(),
        "args": ["--exact", "service_listener_child", "--nocapture"],
        "env": {
            LISTEN_CHILD_ENV: "1",
            LISTEN_ADDR_ENV: format!("127.0.0.1:{port}"),
        },
        "metadata": {"declared_by": "cli-conformance"},
        "readiness": {"host": "127.0.0.1", "port": port, "timeout_ms": 20000, "interval_ms": 10},
    }])
}

/// A declared service something outside Workcell already runs.
fn target_owned_declaration(port: u16) -> Value {
    json!([{
        "logical_ref": LOGICAL_INFERENCE_SERVICE,
        "lifetime": "target-owned",
        "endpoint": format!("http://127.0.0.1:{port}"),
        "status": {
            "program": env::current_exe().unwrap().display().to_string(),
            "args": ["--exact", "service_status_probe", "--nocapture"],
            "env": {PROBE_ADDR_ENV: format!("127.0.0.1:{port}")},
        },
        "metadata": {"declared_by": "cli-conformance"},
    }])
}

/// Re-invoked by the managed service provider as the material service process.
#[test]
fn service_listener_child() {
    if env::var(LISTEN_CHILD_ENV).as_deref() != Ok("1") {
        return;
    }
    let listener = TcpListener::bind(env::var(LISTEN_ADDR_ENV).unwrap()).unwrap();
    listener.set_nonblocking(true).unwrap();
    let deadline = Instant::now() + Duration::from_secs(60);
    loop {
        match listener.accept() {
            Ok((_stream, _peer)) => {}
            Err(error) if error.kind() == ErrorKind::WouldBlock => {}
            Err(error) => panic!("service listener child accept: {error}"),
        }
        if Instant::now() >= deadline {
            return;
        }
        thread::sleep(Duration::from_millis(5));
    }
}

/// Re-invoked as the target-native status command. It answers for the socket,
/// so an unreachable service reports unavailable instead of being assumed live.
#[test]
fn service_status_probe() {
    let Ok(address) = env::var(PROBE_ADDR_ENV) else {
        return;
    };
    let address = address.parse().unwrap();
    assert!(
        TcpStream::connect_timeout(&address, Duration::from_millis(500)).is_ok(),
        "declared service is not reachable at {address}"
    );
}

fn assert_success(output: &Output, what: &str) {
    assert!(
        output.status.success(),
        "{what} failed\nstdout={}\nstderr={}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn shipped_binary_plans_and_materialises_a_declared_inference_service() {
    let state = temp_path("managed");
    let port = free_port();
    write_declaration(&state, provider_process_scoped_declaration(port));

    let planned = run(&[
        "--state-root",
        path_arg(&state),
        "--json",
        "plan",
        "--demand-ref",
        "demand:cli-inference",
        "--connect",
        LOGICAL_INFERENCE_SERVICE,
    ]);
    assert_success(&planned, "workcell plan");
    let plan = json_stdout(&planned);
    assert_eq!(
        plan["status"], "satisfiable",
        "plan was not satisfiable: {plan}"
    );
    let binding = plan["planned_bindings"]
        .as_array()
        .unwrap()
        .iter()
        .find(|item| item["logical_ref"] == format!("connectivity:{LOGICAL_INFERENCE_SERVICE}"))
        .unwrap_or_else(|| panic!("no service binding was planned: {plan}"));
    assert_eq!(
        binding["provider_ref"], "provider:collapsed-local-managed-services",
        "unexpected provider: {binding}"
    );

    let receipt = state.join("world.json");
    let prepared = run(&[
        "--state-root",
        path_arg(&state),
        "--receipt",
        path_arg(&receipt),
        "--json",
        "prepare",
        "--demand-ref",
        "demand:cli-inference",
        "--connect",
        LOGICAL_INFERENCE_SERVICE,
    ]);
    assert_success(&prepared, "workcell prepare");

    let world: Value = serde_json::from_slice(&fs::read(&receipt).unwrap()).unwrap();
    let bindings = world["binding_graph"]["bindings"].as_array().unwrap();
    let service = bindings
        .iter()
        .find(|item| item["port"] == "service")
        .unwrap_or_else(|| panic!("receipt has no service binding: {world}"));
    assert_eq!(service["health"], "healthy", "service binding: {service}");
    assert_eq!(
        service["properties"]["endpoint"],
        format!("http://127.0.0.1:{port}")
    );
    // The endpoint was reached before this binding was called healthy: the
    // readiness probe in the declaration is a TCP connect against the port the
    // child actually bound.
    assert!(
        service["properties"]["pid"]
            .as_str()
            .unwrap()
            .parse::<u32>()
            .unwrap()
            > 0
    );
    // A provider-owned child does not outlive the command that started it, and
    // the receipt says so rather than implying a service that is still up.
    assert_eq!(
        service["properties"]["lifetime"], "provider-process-scoped",
        "the receipt must disclose the service's lifetime: {service}"
    );

    let _ = fs::remove_dir_all(&state);
}

#[test]
fn a_declared_service_whose_program_is_absent_stays_honestly_unsatisfiable() {
    let state = temp_path("absent");
    let port = free_port();
    let mut declaration = provider_process_scoped_declaration(port);
    declaration[0]["program"] = json!(state.join("no-such-executable").display().to_string());
    write_declaration(&state, declaration);

    let planned = run(&[
        "--state-root",
        path_arg(&state),
        "--json",
        "plan",
        "--demand-ref",
        "demand:cli-inference-absent",
        "--connect",
        LOGICAL_INFERENCE_SERVICE,
    ]);
    let plan = json_stdout(&planned);
    assert_eq!(
        plan["status"], "unsatisfiable",
        "declaring a service must not fake its presence: {plan}"
    );
    let reason = plan["omissions"][0]["reason"].as_str().unwrap();
    assert!(
        reason.contains("unavailable"),
        "the omission must say the offer was unavailable, got `{reason}`"
    );

    let _ = fs::remove_dir_all(&state);
}

#[test]
fn a_target_owned_service_can_be_observed_by_a_later_invocation() {
    let state = temp_path("target-owned");
    let port = free_port();
    let listener = TcpListener::bind(("127.0.0.1", port)).unwrap();
    listener.set_nonblocking(true).unwrap();
    let accepting = thread::spawn(move || {
        let deadline = Instant::now() + Duration::from_secs(60);
        while Instant::now() < deadline {
            match listener.accept() {
                Ok((_stream, _peer)) => {}
                Err(error) if error.kind() == ErrorKind::WouldBlock => {}
                Err(error) => panic!("target-owned listener accept: {error}"),
            }
            thread::sleep(Duration::from_millis(5));
        }
    });

    write_declaration(&state, target_owned_declaration(port));
    let receipt = state.join("world.json");
    let prepared = run(&[
        "--state-root",
        path_arg(&state),
        "--receipt",
        path_arg(&receipt),
        "--json",
        "prepare",
        "--demand-ref",
        "demand:cli-inference-target",
        "--connect",
        LOGICAL_INFERENCE_SERVICE,
    ]);
    assert_success(&prepared, "workcell prepare");

    // A separate process. The provider has no in-memory record of the binding
    // and must re-enter it from the receipt, then let the target's own status
    // command answer for the service.
    let observed = run(&[
        "--state-root",
        path_arg(&state),
        "--receipt",
        path_arg(&receipt),
        "--json",
        "observe",
    ]);
    assert_success(&observed, "workcell observe");
    let observation = json_stdout(&observed);
    let service = observation["observations"]
        .as_array()
        .unwrap()
        .iter()
        .find(|item| item["logical_ref"] == format!("connectivity:{LOGICAL_INFERENCE_SERVICE}"))
        .unwrap_or_else(|| panic!("no service observation: {observation}"));
    assert_eq!(
        service["state"], "healthy",
        "a re-entered target-owned service must be observable: {service}"
    );
    assert_eq!(service["detail"]["lifetime"], "target-owned");
    assert_eq!(service["detail"]["started_by_provider"], "false");

    drop(accepting);
    let _ = fs::remove_dir_all(&state);
}

#[test]
fn the_control_service_offers_declared_services_to_a_remote_shipped_binary() {
    let state = temp_path("control-service");
    let port = free_port();
    let listener = TcpListener::bind(("127.0.0.1", port)).unwrap();
    listener.set_nonblocking(true).unwrap();
    thread::spawn(move || {
        let deadline = Instant::now() + Duration::from_secs(60);
        while Instant::now() < deadline {
            match listener.accept() {
                Ok((_stream, _peer)) => {}
                Err(error) if error.kind() == ErrorKind::WouldBlock => {}
                Err(error) => panic!("control-service listener accept: {error}"),
            }
            thread::sleep(Duration::from_millis(5));
        }
    });
    write_declaration(&state, target_owned_declaration(port));

    // Exactly what `workcell-control-service` composes.
    let config = CollapsedLocalConfig::new(
        WorkcellRef::new("workcell:control-service-test").unwrap(),
        &state,
    );
    let workcell = DurableCollapsedLocalWorkcell::new(config).unwrap();
    let mut server = TcpControlServer::bind("127.0.0.1:0", ControlService::new(workcell)).unwrap();
    let endpoint = server.local_addr().unwrap().to_string();

    let client_state = state.join("client");
    let receipt = client_state.join("world.json");
    // The Workcell control plane is not `Send`, so the server stays on this
    // thread. The client thread always sends both envelopes — even when the
    // first one fails — so `serve_n` cannot deadlock on a failing assertion.
    let client_receipt = receipt.clone();
    let client = thread::spawn(move || {
        let planned = run(&[
            "--endpoint",
            &endpoint,
            "--state-root",
            path_arg(&client_state),
            "--json",
            "plan",
            "--demand-ref",
            "demand:remote-inference",
            "--connect",
            LOGICAL_INFERENCE_SERVICE,
        ]);
        let prepared = run(&[
            "--endpoint",
            &endpoint,
            "--state-root",
            path_arg(&client_state),
            "--receipt",
            path_arg(&client_receipt),
            "--json",
            "prepare",
            "--demand-ref",
            "demand:remote-inference",
            "--connect",
            LOGICAL_INFERENCE_SERVICE,
        ]);
        (planned, prepared)
    });

    server.serve_n(2).unwrap();
    let (planned, prepared) = client.join().unwrap();

    assert_success(&planned, "remote workcell plan");
    let plan = json_stdout(&planned);
    assert_eq!(
        plan["status"], "satisfiable",
        "control service could not satisfy the demand: {plan}"
    );

    assert_success(&prepared, "remote workcell prepare");
    let world: Value = serde_json::from_slice(&fs::read(&receipt).unwrap()).unwrap();
    let service = world["binding_graph"]["bindings"]
        .as_array()
        .unwrap()
        .iter()
        .find(|item| item["port"] == "service")
        .cloned()
        .unwrap_or_else(|| panic!("remote receipt has no service binding: {world}"));
    assert_eq!(service["health"], "healthy", "{service}");
    assert_eq!(service["properties"]["lifetime"], "target-owned");

    let _ = fs::remove_dir_all(&state);
}

#[test]
fn declaring_a_service_does_not_disturb_the_zero_setup_baseline() {
    // `workcell doctor` decides the zero-setup baseline from these same
    // discovery offers, so this asserts on its inputs directly. Doctor itself is
    // not shelled out here because it also runs an external `actuation`
    // detection command, which takes about ten seconds on a developer machine.
    let baseline = |state: &Path| -> Value {
        let output = run(&["--state-root", path_arg(state), "--json", "discover"]);
        assert_success(&output, "workcell discover");
        json_stdout(&output)
    };

    let bare = temp_path("baseline-bare");
    let before = baseline(&bare);
    let ports = |discovery: &Value| -> Vec<String> {
        discovery["offers"]
            .as_array()
            .unwrap()
            .iter()
            .map(|offer| offer["port"].as_str().unwrap().to_owned())
            .collect()
    };
    let before_ports = ports(&before);
    assert!(before_ports.contains(&"workspace".to_owned()));
    assert!(before_ports.contains(&"execution".to_owned()));
    assert!(before_ports.contains(&"artifact-storage".to_owned()));
    assert!(
        !before_ports.contains(&"service".to_owned()),
        "an undeclared service must not be offered: {before}"
    );

    // A declared but absent service appears, says it is unavailable, and leaves
    // every baseline offer exactly as it was.
    let declared = temp_path("baseline-declared");
    let port = free_port();
    let mut declaration = provider_process_scoped_declaration(port);
    declaration[0]["program"] = json!(declared.join("no-such-executable").display().to_string());
    write_declaration(&declared, declaration);

    let after = baseline(&declared);
    let service = after["offers"]
        .as_array()
        .unwrap()
        .iter()
        .find(|offer| offer["port"] == "service")
        .unwrap_or_else(|| panic!("declared service is not offered: {after}"));
    assert_eq!(service["connections"][0], LOGICAL_INFERENCE_SERVICE);
    assert_eq!(
        service["availability"], "unavailable",
        "a declared service that is not on the machine must say so: {service}"
    );
    assert_eq!(service["metadata"]["lifetime"], "provider-process-scoped");
    assert_eq!(
        service["metadata"]["physical_acceptance"], "executable-absent",
        "the offer must not imply the executable was accepted: {service}"
    );

    for baseline_port in ["workspace", "execution", "artifact-storage"] {
        let unchanged: Vec<&Value> = after["offers"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|offer| offer["port"] == baseline_port)
            .collect();
        assert!(
            unchanged
                .iter()
                .all(|offer| offer["availability"] == "available"),
            "baseline port `{baseline_port}` changed: {after}"
        );
    }

    let _ = fs::remove_dir_all(&bare);
    let _ = fs::remove_dir_all(&declared);
}
