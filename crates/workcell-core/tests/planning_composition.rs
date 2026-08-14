use epilogos_workcell_core::{
    plan, plan_with_policy, AffordanceRequirement, Availability, DemandRef, Discovery,
    ExecutionDemand, ExternalRef, HealthState, LogicalConnectionRequirement, OfferRef,
    OperationalOffer, PlanStatus, PlanningPolicy, PolicyAssessment, ProviderRef, WorkcellRef,
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
fn one_plan_can_compose_multiple_offers() {
    let mut demand = ExecutionDemand::new(DemandRef::new("demand:composed").unwrap());
    demand
        .affordances
        .required
        .push(AffordanceRequirement::new("shell").unwrap());
    demand
        .connectivity
        .required
        .push(LogicalConnectionRequirement::new("state:graph").unwrap());
    let mut execution = offer("execution");
    execution.affordances.push("shell".into());
    let mut service = offer("service");
    service.connections.push("state:graph".into());
    let result = plan(&demand, &discovery(vec![execution, service])).unwrap();
    assert_eq!(result.status, PlanStatus::Satisfiable);
    assert_eq!(result.planned_bindings.len(), 2);
    assert_ne!(
        result.planned_bindings[0].provider_ref,
        result.planned_bindings[1].provider_ref
    );
}

struct Reject {
    provider: String,
}
impl PlanningPolicy for Reject {
    fn assess(&self, _: &ExecutionDemand, offer: &OperationalOffer) -> PolicyAssessment {
        if offer.provider_ref.as_str() == self.provider {
            PolicyAssessment::reject("placement rule")
        } else {
            PolicyAssessment::allow(0)
        }
    }
}

#[test]
fn policy_rejection_is_explicit() {
    let mut demand = ExecutionDemand::new(DemandRef::new("demand:policy").unwrap());
    demand
        .affordances
        .required
        .push(AffordanceRequirement::new("shell").unwrap());
    let mut only = offer("only");
    only.affordances.push("shell".into());
    let policy = Reject {
        provider: only.provider_ref.to_string(),
    };
    let result = plan_with_policy(&demand, &discovery(vec![only]), &policy).unwrap();
    assert_eq!(result.status, PlanStatus::Unsatisfiable);
    assert!(result.omissions[0].reason.contains("policy"));
}

#[test]
fn equal_offers_are_deterministic_and_subject_refs_survive() {
    let subject = ExternalRef::new("client:candidate:stable").unwrap();
    let mut demand = ExecutionDemand::new(DemandRef::new("demand:tie").unwrap())
        .with_subject("candidate", subject.clone());
    demand
        .affordances
        .required
        .push(AffordanceRequirement::new("shell").unwrap());
    let before = demand.clone();
    let mut b = offer("b");
    b.affordances.push("shell".into());
    let mut a = offer("a");
    a.affordances.push("shell".into());
    let result = plan(&demand, &discovery(vec![b, a])).unwrap();
    assert_eq!(result.planned_bindings[0].offer_ref.as_str(), "offer:a");
    assert_eq!(demand, before);
    assert_eq!(demand.subjects.get("candidate"), Some(&subject));
}
