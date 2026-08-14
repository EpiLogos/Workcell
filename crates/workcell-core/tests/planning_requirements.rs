use epilogos_workcell_core::{
    plan, AffordanceRequirement, Availability, Capacity, DemandRef, Discovery, ExecutionDemand,
    HealthState, OfferRef, OperationalOffer, PlanStatus, ProviderRef, RequirementNecessity,
    ResourceRequirement, WorkcellRef,
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
fn required_failure_is_explicit() {
    let mut demand = ExecutionDemand::new(DemandRef::new("demand:required").unwrap());
    demand
        .affordances
        .required
        .push(AffordanceRequirement::new("shell").unwrap());
    let result = plan(&demand, &discovery(vec![offer("empty")])).unwrap();
    assert_eq!(result.status, PlanStatus::Unsatisfiable);
    assert_eq!(
        result.omissions[0].necessity,
        RequirementNecessity::Required
    );
}

#[test]
fn preferred_failure_is_degradation() {
    let mut demand = ExecutionDemand::new(DemandRef::new("demand:preferred").unwrap());
    demand
        .affordances
        .preferred
        .push(AffordanceRequirement::new("snapshot").unwrap());
    let result = plan(&demand, &discovery(vec![offer("minimal")])).unwrap();
    assert_eq!(result.status, PlanStatus::Degraded);
    assert_eq!(
        result.degradations[0].necessity,
        RequirementNecessity::Preferred
    );
}

#[test]
fn capacity_exhaustion_is_distinct() {
    let mut demand = ExecutionDemand::new(DemandRef::new("demand:capacity").unwrap());
    demand.resources.push(ResourceRequirement {
        key: "memory".into(),
        minimum: Some(16),
        unit: Some("GiB".into()),
    });
    let mut small = offer("small");
    small.capacity.insert(
        "memory".into(),
        Capacity {
            amount: 8,
            unit: Some("GiB".into()),
        },
    );
    let result = plan(&demand, &discovery(vec![small])).unwrap();
    assert_eq!(result.status, PlanStatus::Unsatisfiable);
    assert!(result.omissions[0].reason.contains("capacity"));
}
