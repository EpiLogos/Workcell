use std::{
    env,
    io::ErrorKind,
    net::TcpListener,
    path::PathBuf,
    thread,
    time::{Duration, Instant},
};

use epilogos_workcell_core::{
    Availability, DemandRef, HealthState, LogicalConnectionRequirement, ProviderPort, ProviderRef,
    RetentionExpectation, ServiceMaterialRequest, ServiceProvider,
};
use epilogos_workcell_runtime::{
    ManagedHostService, ManagedHostServiceProvider, TcpEndpointProbe,
};

const CHILD_ENV: &str = "WORKCELL_MANAGED_SERVICE_CHILD";
const CHILD_ADDR_ENV: &str = "WORKCELL_MANAGED_SERVICE_CHILD_ADDR";
const CHILD_EXIT_MS_ENV: &str = "WORKCELL_MANAGED_SERVICE_CHILD_EXIT_MS";

fn free_port() -> u16 {
    TcpListener::bind(("127.0.0.1", 0))
        .unwrap()
        .local_addr()
        .unwrap()
        .port()
}

fn request(logical_ref: &str) -> ServiceMaterialRequest {
    ServiceMaterialRequest {
        demand_ref: DemandRef::new(format!("demand:managed-service:{logical_ref}")).unwrap(),
        connection: LogicalConnectionRequirement::new(logical_ref).unwrap(),
        persistence: None,
    }
}

fn child_service(logical_ref: &str, port: u16, exit_ms: Option<u64>) -> ManagedHostService {
    let executable = env::current_exe().unwrap();
    let mut service = ManagedHostService::new(
        logical_ref,
        format!("http://127.0.0.1:{port}"),
        executable.display().to_string(),
    )
    .unwrap()
    .with_arg("--exact")
    .with_arg("managed_service_child")
    .with_arg("--nocapture")
    .with_env(CHILD_ENV, "1")
    .with_env(CHILD_ADDR_ENV, format!("127.0.0.1:{port}"))
    .with_tcp_readiness(
        TcpEndpointProbe::new("127.0.0.1", port)
            .unwrap()
            .with_timeout_ms(3_000)
            .with_interval_ms(10),
    );
    if let Some(exit_ms) = exit_ms {
        service = service.with_env(CHILD_EXIT_MS_ENV, exit_ms.to_string());
    }
    service
}

#[test]
fn managed_service_child() {
    if env::var(CHILD_ENV).as_deref() != Ok("1") {
        return;
    }

    let address = env::var(CHILD_ADDR_ENV).unwrap();
    let listener = TcpListener::bind(&address).unwrap();
    listener.set_nonblocking(true).unwrap();
    let exit_after = env::var(CHILD_EXIT_MS_ENV)
        .ok()
        .and_then(|value| value.parse::<u64>().ok())
        .map(Duration::from_millis);
    let started = Instant::now();

    loop {
        match listener.accept() {
            Ok((_stream, _peer)) => {}
            Err(error) if error.kind() == ErrorKind::WouldBlock => {}
            Err(error) => panic!("managed service child accept: {error}"),
        }
        if exit_after.is_some_and(|duration| started.elapsed() >= duration) {
            return;
        }
        thread::sleep(Duration::from_millis(5));
    }
}

#[test]
fn managed_host_service_starts_becomes_reachable_and_releases() {
    let logical_ref = "inference:caller-owned-service";
    let port = free_port();
    let mut provider = ManagedHostServiceProvider::new(
        ProviderRef::new("provider:test-managed-service").unwrap(),
        [child_service(logical_ref, port, None).with_metadata("engine", "test-child")],
    )
    .unwrap();

    let offer = provider.offers().unwrap().remove(0);
    assert_eq!(offer.availability, Availability::Available);
    assert_eq!(offer.connections, vec![logical_ref]);

    let allocation = provider.resolve_service(&request(logical_ref)).unwrap();
    assert_eq!(
        allocation.properties.get("logical_ref").map(String::as_str),
        Some(logical_ref)
    );
    assert_eq!(
        allocation.properties.get("endpoint").map(String::as_str),
        Some(format!("http://127.0.0.1:{port}").as_str())
    );
    assert!(allocation.properties.contains_key("pid"));
    assert_eq!(
        allocation
            .provenance
            .get("metadata.engine")
            .map(String::as_str),
        Some("test-child")
    );

    let observation = provider.observe_service(&allocation).unwrap();
    assert_eq!(observation.health, HealthState::Healthy);
    assert_eq!(observation.detail.get("running").map(String::as_str), Some("true"));
    assert_eq!(
        observation.detail.get("reachable").map(String::as_str),
        Some("true")
    );

    let released = provider
        .release_service(&allocation, &RetentionExpectation::Release)
        .unwrap();
    assert!(released.changed);
    assert!(provider.observe_service(&allocation).is_err());
}

#[test]
fn process_disappearance_degrades_observation_and_rematerialises_without_identity_collapse() {
    let logical_ref = "inference:stable-logical-service";
    let port = free_port();
    let mut provider = ManagedHostServiceProvider::new(
        ProviderRef::new("provider:test-rematerialisation").unwrap(),
        [child_service(logical_ref, port, Some(80))],
    )
    .unwrap();

    let first = provider.resolve_service(&request(logical_ref)).unwrap();
    thread::sleep(Duration::from_millis(160));
    let missing = provider.observe_service(&first).unwrap();
    assert_eq!(missing.health, HealthState::Unavailable);
    assert_eq!(missing.detail.get("running").map(String::as_str), Some("false"));

    provider
        .release_service(&first, &RetentionExpectation::Release)
        .unwrap();

    let second_port = free_port();
    let mut replacement = ManagedHostServiceProvider::new(
        ProviderRef::new("provider:test-replacement").unwrap(),
        [child_service(logical_ref, second_port, None)],
    )
    .unwrap();
    let second = replacement.resolve_service(&request(logical_ref)).unwrap();

    assert_ne!(first.material_ref, second.material_ref);
    assert_ne!(first.provider_ref, second.provider_ref);
    assert_eq!(
        first.properties.get("logical_ref"),
        second.properties.get("logical_ref")
    );
    assert_eq!(
        replacement.observe_service(&second).unwrap().health,
        HealthState::Healthy
    );
    replacement
        .release_service(&second, &RetentionExpectation::Release)
        .unwrap();
}

#[test]
fn missing_program_is_reported_as_unavailable_instead_of_fake_health() {
    let missing = PathBuf::from("/workcell/conformance/definitely-missing/model-service");
    let logical_ref = "inference:missing-engine";
    let service = ManagedHostService::new(
        logical_ref,
        "http://127.0.0.1:65534",
        missing.display().to_string(),
    )
    .unwrap();
    let mut provider = ManagedHostServiceProvider::new(
        ProviderRef::new("provider:test-missing-engine").unwrap(),
        [service],
    )
    .unwrap();

    let offer = provider.offers().unwrap().remove(0);
    assert_eq!(offer.availability, Availability::Unavailable);
    assert_eq!(offer.health, HealthState::Unavailable);
    assert!(provider.resolve_service(&request(logical_ref)).is_err());
}
