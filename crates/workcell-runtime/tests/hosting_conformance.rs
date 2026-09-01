#![cfg(unix)]

use std::collections::BTreeMap;

use epilogos_workcell_core::{
    AffordanceRequirement, DemandRef, ExecutionDemand, ExposureRequirement,
    LogicalConnectionRequirement, PersistenceScope, ProviderRef, RequirementNecessity,
    RetentionExpectation, ServiceMaterialRequest, ServiceProvider, WorkcellError, WorkcellRef,
    WorkspaceAccess, WorkspaceRequirement,
};
use epilogos_workcell_fabric::{
    evaluate_fabric, evaluate_fabric_with_policies, require_fabric_plan, FabricDiagnosticKind,
    FabricPathOffer, FabricPathProvider, FabricPathState, FabricPolicyOffer, FabricPolicyProvider,
    FabricPolicyState, NetworkEndpoint, NetworkRelationship, NetworkSecurity, ReachabilityScope,
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

struct FixturePolicy {
    provider_ref: ProviderRef,
    policies: Vec<FabricPolicyOffer>,
}

impl FabricPolicyProvider for FixturePolicy {
    fn provider_ref(&self) -> &ProviderRef {
        &self.provider_ref
    }

    fn policies(&self) -> epilogos_workcell_core::Result<Vec<FabricPolicyOffer>> {
        Ok(self.policies.clone())
    }
}

fn workcell(value: &str) -> WorkcellRef {
    WorkcellRef::new(value).unwrap()
}

fn endpoint(value: &str) -> NetworkEndpoint {
    NetworkEndpoint::Workcell(workcell(value))
}

fn hosting_demand() -> ExecutionDemand {
    let mut demand = ExecutionDemand::new(DemandRef::new("demand:persistent-hosting").unwrap());
    demand
        .affordances
        .required
        .push(AffordanceRequirement::new("long-lived-execution").unwrap());
    demand
        .affordances
        .required
        .push(AffordanceRequirement::new("supervised-lifecycle").unwrap());
    demand.workspace = Some(WorkspaceRequirement {
        source: None,
        revision: None,
        access: WorkspaceAccess::Writable,
    });
    demand
        .connectivity
        .required
        .push(LogicalConnectionRequirement::new("service:interactive-host").unwrap());
    demand
        .connectivity
        .optional
        .push(LogicalConnectionRequirement::new("ingress:event-webhook").unwrap());
    demand
        .exposure
        .required
        .push(ExposureRequirement::new("interactive-endpoint").unwrap());
    demand
        .exposure
        .optional
        .push(ExposureRequirement::new("event-webhook-ingress").unwrap());
    demand.persistence = Some(PersistenceScope::Project);
    demand.retention = RetentionExpectation::Preserve;
    demand
        .extensions
        .insert("authentication".into(), "required".into());
    demand
        .extensions
        .insert("streaming".into(), "required".into());
    demand
        .extensions
        .insert("readiness".into(), "required".into());
    demand
}

fn fabric_path(
    provider: &str,
    path_ref: &str,
    state: FabricPathState,
    material_endpoint: &str,
    scope: ReachabilityScope,
) -> FabricPathOffer {
    FabricPathOffer {
        provider_ref: ProviderRef::new(provider).unwrap(),
        path_ref: path_ref.into(),
        source: endpoint("workcell:caller"),
        destination: endpoint("workcell:host"),
        transport: Some("opaque-stream".into()),
        scope,
        security: NetworkSecurity::AuthenticatedEncrypted,
        state,
        endpoint: Some(material_endpoint.into()),
        path_class: Some("fixture-overlay".into()),
        provenance: BTreeMap::from([("fixture".into(), "deterministic".into())]),
    }
}

fn hosting_relationship() -> RequiredNetworkRelationship {
    RequiredNetworkRelationship::between_workcells(
        NetworkRelationship::new(
            "relationship:interactive-host",
            "caller:control-surface",
            "service:interactive-host",
        )
        .unwrap()
        .with_transport("opaque-stream")
        .unwrap()
        .with_scope(ReachabilityScope::Private)
        .with_security(NetworkSecurity::AuthenticatedEncrypted),
        RequirementNecessity::Required,
        workcell("workcell:caller"),
        workcell("workcell:host"),
    )
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
fn hosting_demand_is_composed_from_ordinary_workcell_requirements() {
    let demand = hosting_demand();
    demand.validate().unwrap();

    assert!(demand.subjects.is_empty());
    assert_eq!(
        demand.workspace.as_ref().map(|workspace| &workspace.access),
        Some(&WorkspaceAccess::Writable)
    );
    assert_eq!(demand.persistence, Some(PersistenceScope::Project));
    assert_eq!(demand.retention, RetentionExpectation::Preserve);
    assert_eq!(demand.extensions["authentication"], "required");
    assert_eq!(demand.extensions["streaming"], "required");
    assert_eq!(demand.extensions["readiness"], "required");
    assert_eq!(
        demand.connectivity.required[0].as_str(),
        "service:interactive-host"
    );
    assert_eq!(demand.exposure.required[0].as_str(), "interactive-endpoint");
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
    assert_ne!(
        services.observe_service(&first).unwrap().health,
        epilogos_workcell_core::HealthState::Unavailable
    );

    let overlay = FixtureFabric {
        provider_ref: ProviderRef::new("provider:private-overlay-a").unwrap(),
        paths: vec![fabric_path(
            "provider:private-overlay-a",
            "path:overlay-a",
            FabricPathState::Reachable,
            "material://overlay-a/interactive-host",
            ReachabilityScope::Private,
        )],
    };
    let first_fabric =
        require_fabric_plan(evaluate_fabric(&[hosting_relationship()], &[&overlay]).unwrap())
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
            ReachabilityScope::Private,
        )],
    };
    let second_fabric =
        require_fabric_plan(evaluate_fabric(&[hosting_relationship()], &[&replacement]).unwrap())
            .unwrap();
    assert_eq!(
        second_fabric.bindings[0].relationship_ref,
        first_fabric.bindings[0].relationship_ref
    );
    assert_ne!(
        second_fabric.bindings[0].path_provider_ref,
        first_fabric.bindings[0].path_provider_ref
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
fn hosting_reachability_policy_failure_is_not_service_or_path_absence() {
    let relationship = hosting_relationship().with_required_policy();
    let route = FixtureFabric {
        provider_ref: ProviderRef::new("provider:private-overlay").unwrap(),
        paths: vec![fabric_path(
            "provider:private-overlay",
            "path:reachable",
            FabricPathState::Reachable,
            "material://reachable",
            ReachabilityScope::Private,
        )],
    };
    let policy = FixturePolicy {
        provider_ref: ProviderRef::new("provider:policy").unwrap(),
        policies: vec![FabricPolicyOffer {
            provider_ref: ProviderRef::new("provider:policy").unwrap(),
            policy_ref: "policy:deny-interactive-host".into(),
            relationship_ref: Some("relationship:interactive-host".into()),
            source: endpoint("workcell:caller"),
            destination: endpoint("workcell:host"),
            state: FabricPolicyState::Denied,
            provenance: BTreeMap::from([("fixture".into(), "policy".into())]),
        }],
    };
    let plan = evaluate_fabric_with_policies(&[relationship], &[&route], &[&policy]).unwrap();
    assert!(plan
        .diagnostics
        .iter()
        .any(|diagnostic| diagnostic.kind == FabricDiagnosticKind::PolicyDenied));
    assert!(!plan
        .diagnostics
        .iter()
        .any(|diagnostic| diagnostic.kind == FabricDiagnosticKind::DestinationUnavailable));
    assert!(matches!(
        require_fabric_plan(plan),
        Err(WorkcellError::UnsatisfiedDemand(_))
    ));
}

#[test]
fn private_hosting_relation_cannot_be_satisfied_by_public_exposure() {
    let public_only = FixtureFabric {
        provider_ref: ProviderRef::new("provider:public-ingress").unwrap(),
        paths: vec![fabric_path(
            "provider:public-ingress",
            "path:public",
            FabricPathState::Reachable,
            "https://public.example/interactive-host",
            ReachabilityScope::Public,
        )],
    };
    let plan = evaluate_fabric(&[hosting_relationship()], &[&public_only]).unwrap();
    assert!(plan
        .diagnostics
        .iter()
        .any(|diagnostic| diagnostic.kind == FabricDiagnosticKind::ScopeMismatch));
    assert!(matches!(
        require_fabric_plan(plan),
        Err(WorkcellError::UnsatisfiedDemand(_))
    ));
}
