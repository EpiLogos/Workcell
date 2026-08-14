use epilogos_workcell_core::{
    plan, AffordanceRequirement, Availability, DemandRef, Discovery, ExecutionDemand,
    ExposureRequirement, HealthState, OfferRef, OperationalOffer, PlanStatus, ProviderRef,
    RequirementNecessity, WorkcellRef,
};
use std::collections::BTreeMap;

fn offer(id: &str) -> OperationalOffer {
    OperationalOffer {
        offer_ref: OfferRef::new(format!("offer:{id}")).unwrap(),
        provider_ref: ProviderRef::new(format!("provider:{id}")).unwrap(),
        port: "fixture".into(),
        affordances: vec![],
        connections: vec![],
        exposures: vec![],
        isolation_trust: vec![],
        availability: Availability::Available,
        health: HealthState::Healthy,
        capacity: BTreeMap::new(),
        metadata: BTreeMap::new(),
    }
}

fn discovery(offers: Vec<OperationalOffer>) -> Discovery {
    Discovery {
        workcell_ref: WorkcellRef::new("workcell:test").unwrap(),
        health: HealthState::Healthy,
        offers,
    }
}

#[test]
fn optional_absence_is_explicit_without_degrading_the_plan() {
    let mut demand = ExecutionDemand::new(DemandRef::new("demand:optional").unwrap());
    demand
        .exposure
        .optional
        .push(ExposureRequirement::new("browser").unwrap());
    let result = plan(&demand, &discovery(vec![offer("minimal")])).unwrap();
    assert_eq!(result.status, PlanStatus::Satisfiable);
    assert_eq!(result.omissions.len(), 1);
    assert_eq!(
        result.omissions[0].necessity,
        RequirementNecessity::Optional
    );
}

#[test]
fn unavailable_matching_offer_is_not_selected() {
    let mut demand = ExecutionDemand::new(DemandRef::new("demand:unavailable").unwrap());
    demand
        .affordances
        .required
        .push(AffordanceRequirement::new("shell").unwrap());
    let mut unavailable = offer("down");
    unavailable.affordances.push("shell".into());
    unavailable.availability = Availability::Unavailable;
    let result = plan(&demand, &discovery(vec![unavailable])).unwrap();
    assert_eq!(result.status, PlanStatus::Unsatisfiable);
    assert!(result.omissions[0].reason.contains("unavailable"));
}
