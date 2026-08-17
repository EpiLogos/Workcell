use std::{collections::BTreeMap, path::PathBuf};

use epilogos_workcell_arrakis::{ArrakisExecutionConfig, ArrakisExecutionProvider};
use epilogos_workcell_core::{
    DemandRef, ExecutionMaterialRequest, ExecutionProvider, IsolationTrustRequirement,
    ProviderOperation, ProviderPort, ProviderRef, RetentionExpectation,
};

fn live_enabled() -> bool {
    std::env::var("WORKCELL_ARRAKIS_LIVE").as_deref() == Ok("1")
}

fn config() -> ArrakisExecutionConfig {
    let client = std::env::var("WORKCELL_ARRAKIS_CLIENT")
        .map(PathBuf::from)
        .unwrap_or_else(|_| PathBuf::from("arrakis-client"));
    let mut config = ArrakisExecutionConfig::new(client).unwrap();
    if let Ok(path) = std::env::var("WORKCELL_ARRAKIS_CONFIG") {
        config = config.with_client_config(path).unwrap();
    }
    if let Ok(path) = std::env::var("WORKCELL_ARRAKIS_KERNEL") {
        config = config.with_kernel(path).unwrap();
    }
    if let Ok(path) = std::env::var("WORKCELL_ARRAKIS_ROOTFS") {
        config = config.with_rootfs(path).unwrap();
    }
    let require_local_kvm = std::env::var("WORKCELL_ARRAKIS_REQUIRE_LOCAL_KVM")
        .map(|value| value != "0")
        .unwrap_or(true);
    config.require_local_kvm(require_local_kvm)
}

#[test]
fn live_arrakis_prepare_shell_snapshot_restore_observe_release() {
    if !live_enabled() {
        return;
    }

    let mut provider =
        ArrakisExecutionProvider::new(ProviderRef::new("provider:arrakis-live").unwrap(), config());
    assert!(!provider.offers().unwrap().is_empty());

    let request = ExecutionMaterialRequest {
        demand_ref: DemandRef::new(format!("demand:arrakis-live-{}", std::process::id())).unwrap(),
        affordances: vec!["shell".into(), "snapshot".into(), "restore".into()],
        resources: vec![],
        connectivity: vec![],
        isolation_trust: Some(IsolationTrustRequirement::new("strong-isolation").unwrap()),
        retention: RetentionExpectation::Release,
    };
    let allocation = provider.prepare_execution(&request).unwrap();

    let shell = provider
        .execute_operation(
            &allocation,
            &ProviderOperation {
                key: "shell".into(),
                parameters: BTreeMap::from([(
                    "command".into(),
                    "printf workcell-arrakis-live".into(),
                )]),
            },
        )
        .unwrap();
    assert!(shell
        .output
        .get("stdout")
        .is_some_and(|value| value.contains("workcell-arrakis-live")));
    assert_eq!(
        provider.observe_execution(&allocation).unwrap().health,
        epilogos_workcell_core::HealthState::Healthy
    );

    let snapshot_id = format!("workcell-live-{}", std::process::id());
    let snapshot = provider
        .snapshot_execution(&allocation, Some(&snapshot_id))
        .unwrap();
    assert_eq!(
        snapshot
            .provenance
            .get("arrakis_snapshot_id")
            .map(String::as_str),
        Some(snapshot_id.as_str())
    );
    provider
        .release_execution(&allocation, &RetentionExpectation::Release)
        .unwrap();

    let restored = provider
        .restore_execution(&allocation, &snapshot_id)
        .unwrap();
    assert_eq!(
        restored
            .provenance
            .get("arrakis_restored_snapshot_id")
            .map(String::as_str),
        Some(snapshot_id.as_str())
    );
    assert_eq!(
        provider.observe_execution(&allocation).unwrap().health,
        epilogos_workcell_core::HealthState::Healthy
    );
    provider
        .release_execution(&allocation, &RetentionExpectation::Release)
        .unwrap();
}
