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
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    std::env::temp_dir().join(format!(
        "epilogos-workcell-external-service-{label}-{}-{nonce}",
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
    ServiceMaterialRequest {
        demand_ref: DemandRef::new("demand:external-service").unwrap(),
        connection: LogicalConnectionRequirement::new("service:existing-gateway").unwrap(),
        persistence: Some(epilogos_workcell_core::PersistenceScope::Project),
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
