use std::{fs, path::PathBuf, thread, time::{Duration, SystemTime, UNIX_EPOCH}};

use epilogos_workcell_control::{
    ControlClient, ControlClientError, ControlService, TcpControlServer, TcpControlTransport,
};
use epilogos_workcell_core::{
    AffordanceRequirement, DemandRef, ExecutionDemand, WorkcellRef,
};
use epilogos_workcell_runtime::{CollapsedLocalConfig, CollapsedLocalWorkcell};

fn temp_path(label: &str) -> PathBuf {
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    std::env::temp_dir().join(format!(
        "epilogos-workcell-control-tcp-{label}-{}-{nonce}",
        std::process::id()
    ))
}

fn local(label: &str) -> (CollapsedLocalWorkcell, PathBuf) {
    let root = temp_path(label);
    let workcell = CollapsedLocalWorkcell::new(CollapsedLocalConfig::new(
        WorkcellRef::new("workcell:tcp-control-test").unwrap(),
        &root,
    ))
    .unwrap();
    (workcell, root)
}

fn shell_demand(label: &str) -> ExecutionDemand {
    let mut demand = ExecutionDemand::new(DemandRef::new(format!("demand:{label}")).unwrap());
    demand
        .affordances
        .required
        .push(AffordanceRequirement::new("shell").unwrap());
    demand
}

#[test]
fn tcp_path_carries_versioned_control_calls_without_identity_translation() {
    let (workcell, root) = local("parity");
    let service = ControlService::new(workcell);
    let mut server = TcpControlServer::bind("127.0.0.1:0", service).unwrap();
    let address = server.local_addr().unwrap();
    let server_thread = thread::spawn(move || server.serve_n(3).unwrap());

    let mut client = ControlClient::new(
        TcpControlTransport::new(address.to_string()).with_timeout(Some(Duration::from_secs(2))),
    );
    let discovery = client.discover().unwrap();
    assert_eq!(discovery["workcell_ref"], "workcell:tcp-control-test");
    assert_eq!(client.status().unwrap()["health"], "healthy");
    assert_eq!(client.plan(&shell_demand("tcp-parity")).unwrap()["status"], "satisfiable");

    server_thread.join().unwrap();
    let _ = fs::remove_dir_all(root);
}

#[test]
fn tcp_authentication_failure_remains_distinct_from_transport_failure() {
    let (workcell, root) = local("auth");
    let service = ControlService::new(workcell).with_authorization("secret");
    let mut server = TcpControlServer::bind("127.0.0.1:0", service).unwrap();
    let address = server.local_addr().unwrap();
    let server_thread = thread::spawn(move || server.serve_n(2).unwrap());

    let mut unauthenticated = ControlClient::new(TcpControlTransport::new(address.to_string()));
    assert!(matches!(
        unauthenticated.discover(),
        Err(ControlClientError::AuthenticationFailed(_))
    ));

    let mut authenticated = ControlClient::new(TcpControlTransport::new(address.to_string()))
        .with_authorization("secret");
    assert!(authenticated.discover().is_ok());
    server_thread.join().unwrap();

    let dead_listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let dead_address = dead_listener.local_addr().unwrap();
    drop(dead_listener);
    let mut unavailable = ControlClient::new(TcpControlTransport::new(dead_address.to_string()));
    assert!(matches!(
        unavailable.discover(),
        Err(ControlClientError::TransportUnavailable(_))
    ));

    let _ = fs::remove_dir_all(root);
}
