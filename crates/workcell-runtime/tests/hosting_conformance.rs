#![cfg(unix)]

use std::collections::BTreeMap;

use epilogos_workcell_core::{
    DemandRef, LogicalConnectionRequirement, PersistenceScope, ProviderRef, RequirementNecessity,
    RetentionExpectation, ServiceMaterialRequest, ServiceProvider, WorkcellError, WorkcellRef,
};
use epilogos_workcell_fabric::{
    evaluate_fabric, require_fabric_plan, FabricDiagnosticKind, FabricPathOffer,
    FabricPathProvider, FabricPathState, NetworkRelationship, NetworkSecurity, ReachabilityScope,
    RequiredNetworkRelationship,
};
use epilogos_workcell_runtime::{ManagedHostService, ManagedHostServiceProvider};

struct FixtureFabric {
    provider_ref: ProviderRef,
    paths: Vec<FabricPathOffer>,
}

impl FabricPathProvider for FixtureFabric {
    fn provider_ref(&self) -> &ProviderRef {
        &self.provider_ref
    }

    fn paths(&self) -> epilogos_workcell_core::Result<Vec<FabricPathOffer>> {
        Ok(self.paths.clone())
    }
}

fn workcell(value: &str) -> WorkcellRef {
    WorkcellRef::new(value).unwrap()
}

fn fabric_path(
    provider: &str,
    path_ref: &str,
    state: FabricPathState,
    endpoint: &str,
) -> FabricPathOffer {
    FabricPathOffer {
        provider_ref: ProviderRef::new(provider).unwrap(),
        path_ref: path_ref.into(),
        source_workcell: workcell("workcell:caller"),
        destination_workcell: workcell("workcell:host"),
        transport: Some("opaque-stream".into()),
        scope: ReachabilityScope::Private,
        security: NetworkSecurity::AuthenticatedEncrypted,
        state,
        endpoint: Some(endpoint.into()),
        path_class: Some("fixture-private-overlay".into()),
        provenance: BTreeMap::from([("fixture".into(), "deterministic".into())]),
    }
}

fn hosting_relationship() -> RequiredNetworkRelationship {
    RequiredNetworkRelationship {
        relationship: NetworkRelationship::new(
            "relationship:interactive-host",
            "caller:control-surface",
            "service:interactive-host",
        )
        .unwrap()
        .with_transport("opaque-stream")
        .unwrap()
        .with_scope(ReachabilityScope::Private)
        .with_security(NetworkSecurity::AuthenticatedEncrypted),
        necessity: RequirementNecessity::Required,
        source_workcell: workcell("workcell:caller"),
        destination_workcell: workcell("workcell:host"),
    }
}

fn managed_provider() -> ManagedHostServiceProvider {
    let service = ManagedHostService::new(
        "service:interactive-host",
        "opaque://interactive-host",
        "sh",
    )
    .unwrap()
    .with_arg("-c")
    .with_arg("while :; do sleep 1; done")
    .with_metadata("lifecycle", "long-lived")
    .with_metadata("authentication", "required")
    .with_metadata("streaming", "supported-by-application")
    .with_metadata("event-ingress", "optional")
    .with_metadata("application-protocol", "opaque-to-workcell");
    ManagedHostServiceProvider::new(
        ProviderRef::new("provider:managed-hosting-fixture").unwrap(),
        [service],
    )
    .unwrap()
}

#[test]
fn persistent_hosting_is_service_lifecycle_plus_fabric_not_agent_gateway_ontology() {
    let request = ServiceMaterialRequest {
        demand_ref: DemandRef::new("demand:persistent-hosting").unwrap(),
        connection: LogicalConnectionRequirement::new("service:interactive-host").unwrap(),
        persistence: Some(PersistenceScope::Project),
    };

    let mut services = managed_provider();
    let first = services.resolve_service(&request).unwrap();
    let first_material = first.material_ref.clone();
    assert_eq!(
        first.properties.get("logical_ref").map(String::as_str),
        Some("service:interactive-host")
    );
    assert!(services.observe_service(&first).unwrap().health != epilogos_workcell_core::HealthState::Unavailable);

    let overlay = FixtureFabric {
        provider_ref: ProviderRef::new("provider:private-overlay-a").unwrap(),
        paths: vec![fabric_path(
            "provider:private-overlay-a",
            "path:overlay-a",
            FabricPathState::Reachable,
            "material://overlay-a/interactive-host",
        )],
    };
    let first_fabric = require_fabric_plan(
        evaluate_fabric(&[hosting_relationship()], &[&overlay]).unwrap(),
    )
    .unwrap();
    assert_eq!(
        first_fabric.bindings[0].relationship_ref,
        "relationship:interactive-host"
    );
    assert_eq!(
        first_fabric.bindings[0].endpoint.as_deref(),
        Some("material://overlay-a/interactive-host")
    );

    services
        .release_service(&first, &RetentionExpectation::Release)
        .unwrap();
    let rebound = services.resolve_service(&request).unwrap();
    assert_ne!(rebound.material_ref, first_material);
    assert_eq!(
        rebound.properties.get("logical_ref").map(String::as_str),
        Some("service:interactive-host")
    );

    let replacement = FixtureFabric {
        provider_ref: ProviderRef::new("provider:private-overlay-b").unwrap(),
        paths: vec![fabric_path(
            "provider:private-overlay-b",
            "path:overlay-b",
            FabricPathState::Reachable,
            "material://overlay-b/interactive-host",
        )],
    };
    let second_fabric = require_fabric_plan(
        evaluate_fabric(&[hosting_relationship()], &[&replacement]).unwrap(),
    )
    .unwrap();
    assert_eq!(
        second_fabric.bindings[0].relationship_ref,
        first_fabric.bindings[0].relationship_ref
    );
    assert_ne!(
        second_fabric.bindings[0].provider_ref,
        first_fabric.bindings[0].provider_ref
    );
    assert_ne!(
        second_fabric.bindings[0].endpoint,
        first_fabric.bindings[0].endpoint
    );

    services
        .release_service(&rebound, &RetentionExpectation::Release)
        .unwrap();
}

#[test]
fn hosting_reachability_policy_failure_is_not_service_absence() {
    let denied = FixtureFabric {
        provider_ref: ProviderRef::new("provider:private-overlay").unwrap(),
        paths: vec![fabric_path(
            "provider:private-overlay",
            "path:denied",
            FabricPathState::Denied,
            "material://denied",
        )],
    };
    let plan = evaluate_fabric(&[hosting_relationship()], &[&denied]).unwrap();
    assert!(plan
        .diagnostics
        .iter()
        .any(|diagnostic| diagnostic.kind == FabricDiagnosticKind::PolicyDenied));
    assert!(matches!(
        require_fabric_plan(plan),
        Err(WorkcellError::UnsatisfiedDemand(_))
    ));
}
