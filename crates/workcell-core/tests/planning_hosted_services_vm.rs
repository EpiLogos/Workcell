use epilogos_workcell_core::{
    plan, AffordanceRequirement, Availability, DemandRef, Discovery, ExecutionDemand, HealthState,
    OfferRef, OperationalOffer, PersistenceScope, PlanStatus, ProviderRef, RetentionExpectation,
    WorkcellRef,
};
use std::collections::BTreeMap;

const HOSTED_SERVICES_VM_ADVISORY: &str = "execution:hosting-advisory";
const PROFILE_SECTION: &str = "Profile: hosted services VM";

fn host_process_offer() -> OperationalOffer {
    OperationalOffer {
        offer_ref: OfferRef::new("offer:collapsed-local-host-process:host-process").unwrap(),
        provider_ref: ProviderRef::new("provider:collapsed-local-host-process").unwrap(),
        port: "execution".into(),
        affordances: vec![
            "shell".into(),
            "process-execution".into(),
            "execution:host-process".into(),
            "persistence:ephemeral".into(),
            "retention:preserve".into(),
        ],
        connections: vec![],
        exposures: vec![],
        isolation_trust: vec!["host-process".into()],
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
        capacity: BTreeMap::new(),
        offers,
    }
}

fn demand_with_shell(demand_ref: &str) -> ExecutionDemand {
    let mut demand = ExecutionDemand::new(DemandRef::new(demand_ref).unwrap());
    demand
        .affordances
        .required
        .push(AffordanceRequirement::new("shell").unwrap());
    demand
}

#[test]
fn preserve_on_host_process_carries_the_hosted_services_vm_advisory() {
    let mut demand = demand_with_shell("demand:preserve-host-process");
    demand.retention = RetentionExpectation::Preserve;
    let result = plan(&demand, &discovery(vec![host_process_offer()])).unwrap();
    let advisory = result
        .degradations
        .iter()
        .find(|degradation| degradation.requirement == HOSTED_SERVICES_VM_ADVISORY)
        .expect("hosted-services-VM advisory expected");
    assert!(advisory.reason.contains("services VM"));
    assert!(advisory.reason.contains("docs/DEPLOYMENT-PROFILES.md"));
    assert!(advisory.reason.contains(PROFILE_SECTION));
    assert_eq!(result.status, PlanStatus::Degraded);
}

#[test]
fn durable_persistence_scope_triggers_the_advisory() {
    let mut demand = demand_with_shell("demand:project-host-process");
    demand.persistence = Some(PersistenceScope::Project);
    let result = plan(&demand, &discovery(vec![host_process_offer()])).unwrap();
    assert!(
        result
            .degradations
            .iter()
            .any(|degradation| degradation.requirement == HOSTED_SERVICES_VM_ADVISORY),
        "durable persistence scope should trigger the advisory"
    );
}

#[test]
fn short_lived_placement_on_host_process_is_not_advised() {
    let mut demand = demand_with_shell("demand:ephemeral-host-process");
    demand.persistence = Some(PersistenceScope::Ephemeral);
    let result = plan(&demand, &discovery(vec![host_process_offer()])).unwrap();
    assert_eq!(result.status, PlanStatus::Satisfiable);
    assert!(result.degradations.is_empty());
}

#[test]
fn non_durable_retention_on_host_process_is_not_advised() {
    let mut demand = demand_with_shell("demand:snapshot-host-process");
    demand.retention = RetentionExpectation::SnapshotIfSupported;
    let result = plan(&demand, &discovery(vec![host_process_offer()])).unwrap();
    assert!(result
        .degradations
        .iter()
        .all(|degradation| degradation.requirement != HOSTED_SERVICES_VM_ADVISORY));
}

#[test]
fn preserve_on_another_provider_is_not_advised() {
    let mut demand = demand_with_shell("demand:preserve-elsewhere");
    demand.retention = RetentionExpectation::Preserve;
    let mut other = host_process_offer();
    other.offer_ref = OfferRef::new("offer:elsewhere").unwrap();
    other.provider_ref = ProviderRef::new("provider:elsewhere").unwrap();
    let result = plan(&demand, &discovery(vec![other])).unwrap();
    assert_eq!(result.status, PlanStatus::Satisfiable);
    assert!(result.degradations.is_empty());
}
