use std::collections::BTreeMap;

use epilogos_workcell_candidate::CandidateMaterialisationDemand;
use epilogos_workcell_core::{
    compose_world, AffordanceRequirement, Availability, BindingPresence, DemandRef, ExecutionDemand,
    ExecutionMaterialRequest, ExecutionProvider, ExternalRef, HealthState, MaterialisationPlan,
    OfferRef, OperationalOffer, PlanRef, PlanStatus, PlannedAllocation, PlannedBinding,
    PlannedExposure, ProviderAllocation, ProviderObservation, ProviderOperation,
    ProviderOperationResult, ProviderPort, ProviderPortKind, ProviderRef, ProviderReleaseResult,
    ReleaseDisposition, RequirementNecessity, Result, RetentionExpectation, WorkcellError,
    WorkcellRef,
};

fn semantic_view() -> CandidateMaterialisationDemand {
    let mut demand = ExecutionDemand::new(DemandRef::new("demand:candidate-materialise").unwrap());
    demand
        .affordances
        .required
        .push(AffordanceRequirement::new("shell").unwrap());
    CandidateMaterialisationDemand::new(
        ExternalRef::new("factory:candidate:c-42").unwrap(),
        demand,
    )
    .unwrap()
}

fn materialise(
    view: &CandidateMaterialisationDemand,
    workcell: &str,
    provider: &str,
    offer: &str,
    material: &str,
    endpoint: &str,
) -> epilogos_workcell_core::MaterialisedExecutionWorld {
    let provider_ref = ProviderRef::new(provider).unwrap();
    let offer_ref = OfferRef::new(offer).unwrap();
    let plan = MaterialisationPlan {
        plan_ref: PlanRef::new(format!("plan:{workcell}:{material}")).unwrap(),
        demand_ref: view.execution_demand().demand_ref.clone(),
        status: PlanStatus::Satisfiable,
        planned_bindings: vec![PlannedBinding {
            logical_ref: "execution:main".into(),
            requirement: "shell".into(),
            necessity: RequirementNecessity::Required,
            provider_ref: provider_ref.clone(),
            offer_ref: offer_ref.clone(),
        }],
        planned_exposures: vec![PlannedExposure {
            logical_ref: "exposure:application".into(),
            requirement: "browser".into(),
            necessity: RequirementNecessity::Preferred,
            provider_ref: provider_ref.clone(),
            offer_ref: offer_ref.clone(),
        }],
        planned_constraints: vec![],
        degradations: vec![],
        omissions: vec![],
        explanation: vec![],
    };
    compose_world(
        WorkcellRef::new(workcell).unwrap(),
        view.execution_demand(),
        &plan,
        vec![PlannedAllocation {
            logical_ref: "execution:main".into(),
            offer_ref,
            allocation: ProviderAllocation {
                provider_ref,
                port: ProviderPortKind::Execution,
                material_ref: material.into(),
                health: HealthState::Healthy,
                properties: BTreeMap::from([("endpoint".into(), endpoint.into())]),
                provenance: BTreeMap::from([(
                    "material_revision".into(),
                    format!("revision:{material}"),
                )]),
            },
        }],
        vec![],
    )
    .unwrap()
}

#[test]
fn candidate_materialisation_demand_is_only_a_view_over_execution_demand() {
    let view = semantic_view();
    assert_eq!(view.candidate_ref().as_str(), "factory:candidate:c-42");
    assert_eq!(
        view.execution_demand()
            .subjects
            .get("candidate")
            .map(|item| item.as_str()),
        Some("factory:candidate:c-42")
    );
    assert_eq!(
        view.clone().into_execution_demand(),
        view.rematerialisation_demand()
    );

    let conflicting = CandidateMaterialisationDemand::new(
        ExternalRef::new("factory:candidate:different").unwrap(),
        view.rematerialisation_demand(),
    );
    assert!(matches!(conflicting, Err(WorkcellError::InvalidDemand(_))));
}

#[test]
fn same_candidate_ref_survives_provider_workcell_material_and_exposure_rebind() {
    let view = semantic_view();
    let first = materialise(
        &view,
        "workcell:local",
        "provider:container-reference",
        "offer:container-reference",
        "material:container:one",
        "http://127.0.0.1:4101",
    );
    let second = materialise(
        &view,
        "workcell:remote",
        "provider:microvm-reference",
        "offer:microvm-reference",
        "material:vm:two",
        "https://candidate.example.test",
    );

    let first_candidate = first.subjects.get("candidate").unwrap();
    let second_candidate = second.subjects.get("candidate").unwrap();
    assert_eq!(first_candidate, second_candidate);
    assert_eq!(first_candidate.as_str(), "factory:candidate:c-42");
    assert_ne!(first.workcell_ref, second.workcell_ref);
    assert_ne!(
        first.binding_graph.bindings[0].provider_ref,
        second.binding_graph.bindings[0].provider_ref
    );
    assert_ne!(
        first.binding_graph.bindings[0].material_ref,
        second.binding_graph.bindings[0].material_ref
    );
    assert_ne!(
        first.binding_graph.bindings[0].properties.get("endpoint"),
        second.binding_graph.bindings[0].properties.get("endpoint")
    );
    assert_ne!(first.planned_exposures[0].provider_ref, second.planned_exposures[0].provider_ref);
}

struct BoundExecutionProvider {
    provider_ref: ProviderRef,
    offer_ref: OfferRef,
    material_ref: String,
    available: bool,
}

impl ProviderPort for BoundExecutionProvider {
    fn provider_ref(&self) -> &ProviderRef {
        &self.provider_ref
    }

    fn port_kind(&self) -> ProviderPortKind {
        ProviderPortKind::Execution
    }

    fn offers(&self) -> Result<Vec<OperationalOffer>> {
        if !self.available {
            return Ok(vec![]);
        }
        Ok(vec![OperationalOffer {
            offer_ref: self.offer_ref.clone(),
            provider_ref: self.provider_ref.clone(),
            port: ProviderPortKind::Execution.as_str().into(),
            affordances: vec!["shell".into()],
            connections: vec![],
            exposures: vec![],
            isolation_trust: vec![],
            availability: Availability::Available,
            health: HealthState::Healthy,
            capacity: BTreeMap::new(),
            metadata: BTreeMap::new(),
        }])
    }
}

impl ExecutionProvider for BoundExecutionProvider {
    fn prepare_execution(&mut self, _: &ExecutionMaterialRequest) -> Result<ProviderAllocation> {
        Err(WorkcellError::Unsupported("fixture is already materialised".into()))
    }

    fn execute_operation(
        &mut self,
        _: &ProviderAllocation,
        _: &ProviderOperation,
    ) -> Result<ProviderOperationResult> {
        Err(WorkcellError::Unsupported("fixture does not execute".into()))
    }

    fn observe_execution(&self, allocation: &ProviderAllocation) -> Result<ProviderObservation> {
        if allocation.material_ref != self.material_ref {
            return Err(WorkcellError::NotFound(allocation.material_ref.clone()));
        }
        if !self.available {
            return Err(WorkcellError::Unavailable("fixture provider disappeared".into()));
        }
        Ok(ProviderObservation {
            provider_ref: self.provider_ref.clone(),
            material_ref: allocation.material_ref.clone(),
            health: HealthState::Healthy,
            detail: BTreeMap::from([("provider_observation".into(), "live".into())]),
        })
    }

    fn release_execution(
        &mut self,
        allocation: &ProviderAllocation,
        retention: &RetentionExpectation,
    ) -> Result<ProviderReleaseResult> {
        if allocation.material_ref != self.material_ref {
            return Err(WorkcellError::NotFound(allocation.material_ref.clone()));
        }
        match retention {
            RetentionExpectation::Release => Ok(ProviderReleaseResult {
                provider_ref: self.provider_ref.clone(),
                material_ref: allocation.material_ref.clone(),
                disposition: ReleaseDisposition::Released,
                changed: true,
            }),
            _ => Err(WorkcellError::Unsupported(
                "fixture only proves release/rematerialise".into(),
            )),
        }
    }
}

#[test]
fn release_failure_and_stale_material_change_availability_not_candidate_identity() {
    let view = semantic_view();
    let mut world = materialise(
        &view,
        "workcell:lifecycle",
        "provider:lifecycle-candidate",
        "offer:lifecycle-candidate",
        "material:lifecycle-candidate",
        "http://127.0.0.1:4200",
    );
    let candidate = world.subjects.get("candidate").unwrap().clone();

    world.binding_graph.bindings[0].presence = BindingPresence::Stale;
    world.binding_graph.bindings[0].health = HealthState::Unavailable;
    world.state = HealthState::Unavailable;
    assert_eq!(world.subjects.get("candidate"), Some(&candidate));
    assert_eq!(candidate.as_str(), "factory:candidate:c-42");

    let rematerialised = materialise(
        &view,
        "workcell:lifecycle-replacement",
        "provider:lifecycle-replacement",
        "offer:lifecycle-replacement",
        "material:lifecycle-replacement",
        "http://127.0.0.1:4300",
    );
    assert_eq!(rematerialised.subjects.get("candidate"), Some(&candidate));
    assert_eq!(rematerialised.state, HealthState::Healthy);
}

#[test]
fn candidate_integration_crate_contains_no_revision_or_new_candidate_authority() {
    let source = include_str!("../src/lib.rs").to_ascii_lowercase();
    assert!(!source.contains("candidate_revision"));
    assert!(!source.contains("new_candidate"));
    assert!(!source.contains("candidateequivalence"));
    assert!(!source.contains("providerref"));
}
