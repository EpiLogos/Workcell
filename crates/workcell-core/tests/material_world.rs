use std::collections::BTreeMap;

use epilogos_workcell_core::{
    compose_world, BindingPresence, DemandRef, ExecutionDemand, ExternalRef, HealthState,
    MaterialisationPlan, OfferRef, PlanRef, PlanStatus, PlannedAllocation, PlannedBinding,
    PlannedRelation, ProviderAllocation, ProviderPortKind, ProviderRef, RequirementNecessity,
    WorkcellError, WorkcellRef,
};

fn demand() -> ExecutionDemand {
    ExecutionDemand::new(DemandRef::new("demand:material-world").unwrap()).with_subject(
        "candidate",
        ExternalRef::new("client:candidate:stable").unwrap(),
    )
}

fn binding(logical_ref: &str, provider: &str, offer: &str, requirement: &str) -> PlannedBinding {
    PlannedBinding {
        logical_ref: logical_ref.into(),
        requirement: requirement.into(),
        necessity: RequirementNecessity::Required,
        provider_ref: ProviderRef::new(provider).unwrap(),
        offer_ref: OfferRef::new(offer).unwrap(),
    }
}

fn plan(bindings: Vec<PlannedBinding>, suffix: &str) -> MaterialisationPlan {
    MaterialisationPlan {
        plan_ref: PlanRef::new(format!("plan:{suffix}")).unwrap(),
        demand_ref: DemandRef::new("demand:material-world").unwrap(),
        status: PlanStatus::Satisfiable,
        planned_bindings: bindings,
        degradations: vec![],
        omissions: vec![],
        explanation: vec![],
    }
}

fn allocation(
    logical_ref: &str,
    provider: &str,
    offer: &str,
    port: ProviderPortKind,
    material_ref: &str,
    properties: &[(&str, &str)],
    provenance: &[(&str, &str)],
) -> PlannedAllocation {
    PlannedAllocation {
        logical_ref: logical_ref.into(),
        offer_ref: OfferRef::new(offer).unwrap(),
        allocation: ProviderAllocation {
            provider_ref: ProviderRef::new(provider).unwrap(),
            port,
            material_ref: material_ref.into(),
            health: HealthState::Healthy,
            properties: properties
                .iter()
                .map(|(key, value)| ((*key).into(), (*value).into()))
                .collect::<BTreeMap<_, _>>(),
            provenance: provenance
                .iter()
                .map(|(key, value)| ((*key).into(), (*value).into()))
                .collect::<BTreeMap<_, _>>(),
        },
    }
}

#[test]
fn world_composes_multiple_providers_relations_and_material_provenance() {
    let demand = demand();
    let plan = plan(
        vec![
            binding(
                "workspace:source",
                "provider:workspace",
                "offer:workspace",
                "writable-workspace",
            ),
            binding(
                "connection:state:graph",
                "provider:service",
                "offer:service",
                "state:graph",
            ),
        ],
        "composed",
    );
    let world = compose_world(
        WorkcellRef::new("workcell:test").unwrap(),
        &demand,
        &plan,
        vec![
            allocation(
                "workspace:source",
                "provider:workspace",
                "offer:workspace",
                ProviderPortKind::Workspace,
                "workspace-material:42",
                &[("path", "/tmp/material-42")],
                &[("source_commit", "abc123")],
            ),
            allocation(
                "connection:state:graph",
                "provider:service",
                "offer:service",
                ProviderPortKind::Service,
                "service-material:graph",
                &[("endpoint", "neo4j://graph:7687")],
                &[("resolver", "static-fixture")],
            ),
        ],
        vec![PlannedRelation {
            from_logical_ref: "workspace:source".into(),
            to_logical_ref: "connection:state:graph".into(),
            relation: "can-reach".into(),
        }],
    )
    .unwrap();

    assert_eq!(world.workcell_ref.as_str(), "workcell:test");
    assert_eq!(world.demand_ref, demand.demand_ref);
    assert_eq!(world.subjects, demand.subjects);
    assert_eq!(world.binding_graph.bindings.len(), 2);
    assert_eq!(world.binding_graph.relations.len(), 1);

    let workspace = world
        .binding_graph
        .bindings
        .iter()
        .find(|binding| binding.logical_ref == "workspace:source")
        .unwrap();
    let service = world
        .binding_graph
        .bindings
        .iter()
        .find(|binding| binding.logical_ref == "connection:state:graph")
        .unwrap();
    assert_ne!(workspace.provider_ref, service.provider_ref);
    assert_eq!(workspace.offer_ref.as_str(), "offer:workspace");
    assert_eq!(
        workspace.properties.get("path").unwrap(),
        "/tmp/material-42"
    );
    assert_eq!(workspace.provenance.get("source_commit").unwrap(), "abc123");
    assert_eq!(
        service.properties.get("endpoint").unwrap(),
        "neo4j://graph:7687"
    );

    let relation = &world.binding_graph.relations[0];
    assert_eq!(relation.from, workspace.binding_ref);
    assert_eq!(relation.to, service.binding_ref);
    assert_eq!(relation.relation, "can-reach");
    assert_eq!(world.provenance.get("plan_ref").unwrap(), "plan:composed");
}

#[test]
fn world_rejects_unsatisfied_or_mismatched_materialisation() {
    let demand = demand();
    let planned = binding(
        "workspace:source",
        "provider:workspace",
        "offer:workspace",
        "writable-workspace",
    );
    let valid_plan = plan(vec![planned.clone()], "valid");
    let wrong_offer = allocation(
        "workspace:source",
        "provider:workspace",
        "offer:other",
        ProviderPortKind::Workspace,
        "workspace-material:42",
        &[],
        &[],
    );
    assert!(compose_world(
        WorkcellRef::new("workcell:test").unwrap(),
        &demand,
        &valid_plan,
        vec![wrong_offer],
        vec![],
    )
    .is_err());

    let mut unsatisfied = plan(vec![planned], "unsatisfied");
    unsatisfied.status = PlanStatus::Unsatisfiable;
    let result = compose_world(
        WorkcellRef::new("workcell:test").unwrap(),
        &demand,
        &unsatisfied,
        vec![],
        vec![],
    );
    assert!(matches!(result, Err(WorkcellError::UnsatisfiedDemand(_))));
}

#[test]
fn rematerialisation_changes_material_identity_without_changing_semantic_identity() {
    let demand = demand();
    let semantic_candidate = demand.subjects.get("candidate").unwrap().clone();

    let first = compose_world(
        WorkcellRef::new("workcell:first").unwrap(),
        &demand,
        &plan(
            vec![binding(
                "workspace:source",
                "provider:first",
                "offer:first",
                "writable-workspace",
            )],
            "first",
        ),
        vec![allocation(
            "workspace:source",
            "provider:first",
            "offer:first",
            ProviderPortKind::Workspace,
            "workspace-material:first",
            &[("path", "/tmp/first")],
            &[("generation", "one")],
        )],
        vec![],
    )
    .unwrap();

    let second = compose_world(
        WorkcellRef::new("workcell:second").unwrap(),
        &demand,
        &plan(
            vec![binding(
                "workspace:source",
                "provider:second",
                "offer:second",
                "writable-workspace",
            )],
            "second",
        ),
        vec![allocation(
            "workspace:source",
            "provider:second",
            "offer:second",
            ProviderPortKind::Workspace,
            "client:candidate:stable",
            &[("path", "/tmp/second")],
            &[("generation", "two")],
        )],
        vec![],
    )
    .unwrap();

    assert_eq!(first.subjects.get("candidate"), Some(&semantic_candidate));
    assert_eq!(second.subjects.get("candidate"), Some(&semantic_candidate));
    assert_eq!(first.demand_ref, second.demand_ref);
    assert_ne!(first.world_ref, second.world_ref);
    assert_ne!(
        first.binding_graph.bindings[0].binding_ref,
        second.binding_graph.bindings[0].binding_ref
    );
    assert_eq!(
        second.binding_graph.bindings[0].material_ref,
        semantic_candidate.as_str()
    );
    assert_ne!(
        second.binding_graph.bindings[0].binding_ref.as_str(),
        semantic_candidate.as_str()
    );
    assert_ne!(second.world_ref.as_str(), semantic_candidate.as_str());

    let mut released = second.clone();
    released.binding_graph.bindings[0].presence = BindingPresence::Released;
    released.binding_graph.bindings[0].health = HealthState::Unavailable;
    released.state = HealthState::Unavailable;
    assert_eq!(
        released.binding_graph.bindings[0]
            .provenance
            .get("generation")
            .unwrap(),
        "two"
    );
    assert_eq!(
        released.subjects.get("candidate"),
        Some(&semantic_candidate)
    );

    let mut stale = second;
    stale.binding_graph.bindings[0].presence = BindingPresence::Stale;
    stale.binding_graph.bindings[0].health = HealthState::Unknown;
    assert_eq!(
        stale.binding_graph.bindings[0].presence,
        BindingPresence::Stale
    );
    assert_eq!(stale.subjects.get("candidate"), Some(&semantic_candidate));
}
