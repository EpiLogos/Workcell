#![cfg(unix)]

use std::{
    fs,
    os::unix::fs::PermissionsExt,
    path::{Path, PathBuf},
    time::{SystemTime, UNIX_EPOCH},
};

use epilogos_workcell_core::{
    DemandRef, LogicalConnectionRequirement, ProviderRef, RetentionExpectation,
    ServiceMaterialRequest, ServiceProvider,
};
use epilogos_workcell_runtime::{
    ExternalManagedService, ExternalManagedServiceProvider, ExternalServiceAcquisition,
    ExternalServiceCommand,
};

fn temp_root(label: &str) -> PathBuf {
    use std::sync::atomic::{AtomicU64, Ordering};
    // Tests in one binary share a pid; a loaded run can quantise two
    // same-instant calls onto one clock tick.
    static SEQ: AtomicU64 = AtomicU64::new(0);
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let seq = SEQ.fetch_add(1, Ordering::Relaxed);
    std::env::temp_dir().join(format!(
        "epilogos-workcell-external-service-{label}-{}-{nonce}-{seq}",
        std::process::id()
    ))
}

fn fixture(root: &Path) -> (PathBuf, PathBuf) {
    fs::create_dir_all(root).unwrap();
    let script = root.join("target-service.sh");
    let state = root.join("running");
    fs::write(
        &script,
        r#"#!/bin/sh
set -eu
case "$1" in
  status|ready) test -f "$TARGET_STATE" ;;
  start) : > "$TARGET_STATE" ;;
  stop) rm -f "$TARGET_STATE" ;;
  restart) rm -f "$TARGET_STATE"; : > "$TARGET_STATE" ;;
  *) exit 64 ;;
esac
"#,
    )
    .unwrap();
    let mut permissions = fs::metadata(&script).unwrap().permissions();
    permissions.set_mode(0o755);
    fs::set_permissions(&script, permissions).unwrap();
    (script, state)
}

fn command(script: &Path, state: &Path, operation: &str) -> ExternalServiceCommand {
    ExternalServiceCommand::new(script.to_string_lossy())
        .unwrap()
        .with_arg(operation)
        .with_env("TARGET_STATE", state.to_string_lossy())
        .unwrap()
}

fn service(
    script: &Path,
    state: &Path,
    acquisition: ExternalServiceAcquisition,
) -> ExternalManagedService {
    ExternalManagedService::new(
        "service:existing-gateway",
        "target-native://gateway",
        command(script, state, "status"),
    )
    .unwrap()
    .with_readiness(command(script, state, "ready"))
    .with_start(command(script, state, "start"))
    .with_stop(command(script, state, "stop"))
    .with_restart(command(script, state, "restart"))
    .with_acquisition(acquisition)
    .with_metadata("configuration_owner", "target")
    .unwrap()
    .with_metadata("application_protocol", "opaque-to-workcell")
    .unwrap()
}

fn request() -> ServiceMaterialRequest {
    request_with("service:existing-gateway")
}

fn request_with(connection: &str) -> ServiceMaterialRequest {
    ServiceMaterialRequest {
        demand_ref: DemandRef::new("demand:external-service").unwrap(),
        connection: LogicalConnectionRequirement::new(connection).unwrap(),
        persistence: Some(epilogos_workcell_core::PersistenceScope::Project),
        retention: RetentionExpectation::Release,
    }
}

#[test]
fn ensure_running_uses_target_lifecycle_and_stops_only_what_workcell_started() {
    let root = temp_root("ensure");
    let (script, state) = fixture(&root);
    let mut provider = ExternalManagedServiceProvider::new(
        ProviderRef::new("provider:external-service-fixture").unwrap(),
        [service(
            &script,
            &state,
            ExternalServiceAcquisition::EnsureRunning,
        )],
    )
    .unwrap();

    assert!(!state.exists());
    let allocation = provider.resolve_service(&request()).unwrap();
    assert!(state.exists());
    assert_eq!(
        allocation
            .properties
            .get("configuration_owner")
            .map(String::as_str),
        Some("target")
    );
    assert_eq!(
        allocation
            .properties
            .get("started_by_provider")
            .map(String::as_str),
        Some("true")
    );
    assert_eq!(
        provider.observe_service(&allocation).unwrap().health,
        epilogos_workcell_core::HealthState::Healthy
    );
    assert_eq!(
        provider.restart_service(&allocation).unwrap().health,
        epilogos_workcell_core::HealthState::Healthy
    );
    let released = provider
        .release_service(&allocation, &RetentionExpectation::Release)
        .unwrap();
    assert!(released.changed);
    assert!(!state.exists());

    let _ = fs::remove_dir_all(root);
}

#[test]
fn observe_existing_never_takes_ownership_of_target_configuration_or_lifecycle() {
    let root = temp_root("observe");
    let (script, state) = fixture(&root);
    fs::write(&state, "already-running\n").unwrap();
    let mut provider = ExternalManagedServiceProvider::new(
        ProviderRef::new("provider:external-service-fixture").unwrap(),
        [service(
            &script,
            &state,
            ExternalServiceAcquisition::ObserveExisting,
        )],
    )
    .unwrap();

    let allocation = provider.resolve_service(&request()).unwrap();
    assert_eq!(
        allocation
            .properties
            .get("started_by_provider")
            .map(String::as_str),
        Some("false")
    );
    let released = provider
        .release_service(&allocation, &RetentionExpectation::Release)
        .unwrap();
    assert!(!released.changed);
    assert!(state.exists(), "target-owned service must remain running");

    let _ = fs::remove_dir_all(root);
}

fn executable(path: &Path) {
    let mut permissions = fs::metadata(path).unwrap().permissions();
    permissions.set_mode(0o755);
    fs::set_permissions(path, permissions).unwrap();
}

#[test]
fn ensure_running_polls_a_slow_start_instead_of_refusing_one_sample() {
    let root = temp_root("slow-start");
    fs::create_dir_all(&root).unwrap();
    // The start command returns immediately; the material it launches only
    // becomes healthy ~600ms later. One probe taken right after start would
    // mistake that launch for a failure.
    let script = root.join("slow-service.sh");
    let state = root.join("running");
    fs::write(
        &script,
        r#"#!/bin/sh
set -eu
case "$1" in
  status|ready) test -f "$TARGET_STATE" ;;
  start) ( sleep 0.6; : > "$TARGET_STATE" ) & ;;
  stop) rm -f "$TARGET_STATE" ;;
  *) exit 64 ;;
esac
"#,
    )
    .unwrap();
    executable(&script);

    let mut provider = ExternalManagedServiceProvider::new(
        ProviderRef::new("provider:external-service-fixture").unwrap(),
        [ExternalManagedService::new(
            "service:slow-gateway",
            "target-native://slow-gateway",
            command(&script, &state, "status"),
        )
        .unwrap()
        .with_readiness(command(&script, &state, "ready"))
        .with_start(command(&script, &state, "start"))
        .with_stop(command(&script, &state, "stop"))
        .with_acquisition(ExternalServiceAcquisition::EnsureRunning)
        .with_readiness_timing(5_000, 50)],
    )
    .unwrap();

    let allocation = provider
        .resolve_service(&request_with("service:slow-gateway"))
        .unwrap();
    assert_eq!(
        allocation
            .properties
            .get("started_by_provider")
            .map(String::as_str),
        Some("true"),
        "a start that becomes healthy inside the window must be owned"
    );
    let released = provider
        .release_service(&allocation, &RetentionExpectation::Release)
        .unwrap();
    assert!(released.changed);
    assert!(
        !state.exists(),
        "release must stop what ensure-running started"
    );

    let _ = fs::remove_dir_all(root);
}

#[test]
fn a_start_that_never_becomes_healthy_is_stopped_not_leaked() {
    let root = temp_root("never-ready");
    fs::create_dir_all(&root).unwrap();
    let script = root.join("never-ready-service.sh");
    let state = root.join("running");
    let rolled_back = root.join("rolled-back");
    fs::write(
        &script,
        r#"#!/bin/sh
set -eu
case "$1" in
  status|ready) exit 1 ;;
  start) : > "$DECOY_STATE" ;;
  stop) : > "$ROLLED_BACK_STATE" ;;
  *) exit 64 ;;
esac
"#,
    )
    .unwrap();
    executable(&script);

    let target_state = state.to_string_lossy().to_string();
    let decoy = root.join("decoy").to_string_lossy().to_string();
    let rollback_marker = rolled_back.to_string_lossy().to_string();
    let op = |operation: &str| {
        ExternalServiceCommand::new(script.to_string_lossy().to_string())
            .unwrap()
            .with_arg(operation)
            .with_env("TARGET_STATE", target_state.clone())
            .unwrap()
            .with_env("DECOY_STATE", decoy.clone())
            .unwrap()
            .with_env("ROLLED_BACK_STATE", rollback_marker.clone())
            .unwrap()
    };
    let mut provider = ExternalManagedServiceProvider::new(
        ProviderRef::new("provider:external-service-fixture").unwrap(),
        [ExternalManagedService::new(
            "service:never-ready",
            "target-native://never-ready",
            op("status"),
        )
        .unwrap()
        .with_start(op("start"))
        .with_stop(op("stop"))
        .with_acquisition(ExternalServiceAcquisition::EnsureRunning)
        .with_readiness_timing(1_000, 50)],
    )
    .unwrap();

    let request = ServiceMaterialRequest {
        demand_ref: DemandRef::new("demand:external-service").unwrap(),
        connection: LogicalConnectionRequirement::new("service:never-ready").unwrap(),
        persistence: None,
        retention: RetentionExpectation::Release,
    };
    let error = provider.resolve_service(&request).unwrap_err();
    assert!(
        format!("{error}").contains("readiness window"),
        "the refusal must name the exhausted window, got: {error}"
    );
    assert!(
        rolled_back.exists(),
        "the declared stop must run when the window is exhausted: a refused allocation must not leak the process it started"
    );

    let _ = fs::remove_dir_all(root);
}
