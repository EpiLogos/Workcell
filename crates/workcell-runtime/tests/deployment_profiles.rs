use std::collections::BTreeMap;

use epilogos_workcell_core::{
    AffordanceRequirement, Availability, Capacity, DemandRef, ExecutionDemand,
    ExecutionMaterialRequest, ExecutionProvider, HealthState, OfferRef, OperationalOffer,
    PlanStatus, ProviderAllocation, ProviderObservation, ProviderOperation, ProviderOperationResult,
    ProviderPort, ProviderPortKind, ProviderRef, ProviderReleaseResult, Result,
    RetentionExpectation, WorkcellControlPlane, WorkcellError, WorkcellRef,
};
use epilogos_workcell_runtime::{
    deployment_parity_report, DeploymentProfile, PlacementRef,
};

struct FixtureExecutionProvider {
    provider_ref: ProviderRef,
    offer_ref: OfferRef,
    affordances: Vec<String>,
    capacity: BTreeMap<String, Capacity>,
    placement: String,
}

impl FixtureExecutionProvider {
    fn new(provider_ref: &str, offer_ref: &str, placement: &str) -> Self {
        Self {
            provider_ref: ProviderRef::new(provider_ref).unwrap(),
            offer_ref: OfferRef::new(offer_ref).unwrap(),
            affordances: vec!["shell".into()],
            capacity: BTreeMap::new(),
            placement: placement.into(),
        }
    }

    fn with_affordance(mut self, affordance: &str) -> Self {
        self.affordances.push(affordance.into());
        self
    }

    fn with_capacity(mut self, key: &str, amount: u64, unit: &str) -> Self {
        self.capacity.insert(
            key.into(),
            Capacity {
                amount,
                unit: Some(unit.into()),
            },
        );
        self
    }
}

impl ProviderPort for FixtureExecutionProvider {
    fn provider_ref(&self) -> &ProviderRef {
        &self.provider_ref
    }

    fn port_kind(&self) -> ProviderPortKind {
        ProviderPortKind::Execution
    }

    fn offers(&self) -> Result<Vec<OperationalOffer>> {
        Ok(vec![OperationalOffer {
            offer_ref: self.offer_ref.clone(),
            provider_ref: self.provider_ref.clone(),
            port: ProviderPortKind::Execution.as_str().into(),
            affordances: self.affordances.clone(),
            connections: vec![],
            exposures: vec![],
            isolation_trust: vec![],
            availability: Availability::Available,
            health: HealthState::Healthy,
            capacity: self.capacity.clone(),
            metadata: BTreeMap::from([("placement".into(), self.placement.clone())]),
        }])
    }
}

impl ExecutionProvider for FixtureExecutionProvider {
    fn prepare_execution(&mut self, _: &ExecutionMaterialRequest) -> Result<ProviderAllocation> {
        Err(WorkcellError::Unsupported(
            "deployment profile fixture does not materialise".into(),
        ))
    }

    fn execute_operation(
        &mut self,
        _: &ProviderAllocation,
        _: &ProviderOperation,
    ) -> Result<ProviderOperationResult> {
        Err(WorkcellError::Unsupported(
            "deployment profile fixture does not execute operations".into(),
        ))
    }

    fn observe_execution(&self, _: &ProviderAllocation) -> Result<ProviderObservation> {
        Err(WorkcellError::Unsupported(
            "deployment profile fixture has no material allocation".into(),
        ))
    }

    fn release_execution(
        &mut self,
        _: &ProviderAllocation,
        _: &RetentionExpectation,
    ) -> Result<ProviderReleaseResult> {
        Err(WorkcellError::Unsupported(
            "deployment profile fixture has no material allocation".into(),
        ))
    }
}

fn semantic_demand() -> ExecutionDemand {
    let mut demand = ExecutionDemand::new(DemandRef::new("demand:profile-parity").unwrap());
    demand
        .affordances
        .required
        .push(AffordanceRequirement::new("shell").unwrap());
    demand
        .affordances
        .optional
        .push(AffordanceRequirement::new("gpu").unwrap());
    demand
}

fn local_profile() -> DeploymentProfile {
    DeploymentProfile::new(
        "collapsed-local",
        WorkcellRef::new("workcell:profile-local").unwrap(),
    )
    .unwrap()
    .with_capacity("cpu", 8, Some("cores"))
    .unwrap()
    .with_capacity("memory", 16, Some("GiB"))
    .unwrap()
    .with_placement("execution", PlacementRef::new("same-host").unwrap())
    .unwrap()
    .with_metadata("specimen", "collapsed-local")
    .unwrap()
}

fn ubuntu_profile() -> DeploymentProfile {
    DeploymentProfile::new(
        "reference-ubuntu-worker",
        WorkcellRef::new("workcell:profile-ubuntu-worker").unwrap(),
    )
    .unwrap()
    .with_capacity("cpu", 16, Some("cores"))
    .unwrap()
    .with_capacity("memory", 64, Some("GiB"))
    .unwrap()
    .with_placement("execution", PlacementRef::new("worker-host").unwrap())
    .unwrap()
    .with_placement("state", PlacementRef::new("worker-host").unwrap())
    .unwrap()
    .with_metadata("operating-system", "Ubuntu")
    .unwrap()
}

fn distributed_profile() -> DeploymentProfile {
    DeploymentProfile::new(
        "distributed-fake-provider",
        WorkcellRef::new("workcell:profile-distributed").unwrap(),
    )
    .unwrap()
    .with_health(HealthState::Degraded)
    .with_capacity("cpu", 32, Some("cores"))
    .unwrap()
    .with_capacity("memory", 128, Some("GiB"))
    .unwrap()
    .with_placement("execution", PlacementRef::new("compute-domain-a").unwrap())
    .unwrap()
    .with_placement("state", PlacementRef::new("state-domain-b").unwrap())
    .unwrap()
    .with_metadata("specimen", "distributed-fake-provider")
    .unwrap()
}

fn plan_with(
    profile: &DeploymentProfile,
    provider: FixtureExecutionProvider,
    demand: &ExecutionDemand,
) -> (epilogos_workcell_core::Discovery, epilogos_workcell_core::MaterialisationPlan) {
    let mut plane = profile.control_plane().unwrap();
    plane.register_execution_provider(provider).unwrap();
    let discovery = plane.discover().unwrap();
    let plan = plane.plan(demand).unwrap();
    (discovery, plan)
}

#[test]
fn same_semantic_demand_plans_across_local_ubuntu_and_distributed_profiles() {
    let demand = semantic_demand();
    let local = local_profile();
    let ubuntu = ubuntu_profile();
    let distributed = distributed_profile();

    let (local_discovery, local_plan) = plan_with(
        &local,
        FixtureExecutionProvider::new(
            "provider:profile-local",
            "offer:profile-local",
            "same-host",
        ),
        &demand,
    );
    let (ubuntu_discovery, ubuntu_plan) = plan_with(
        &ubuntu,
        FixtureExecutionProvider::new(
            "provider:profile-ubuntu",
            "offer:profile-ubuntu",
            "worker-host",
        )
        .with_capacity("cpu", 16, "cores"),
        &demand,
    );
    let (distributed_discovery, distributed_plan) = plan_with(
        &distributed,
        FixtureExecutionProvider::new(
            "provider:profile-distributed",
            "offer:profile-distributed",
            "compute-domain-a",
        )
        .with_capacity("cpu", 32, "cores"),
        &demand,
    );

    for plan in [&local_plan, &ubuntu_plan, &distributed_plan] {
        assert_eq!(plan.demand_ref, demand.demand_ref);
        assert_eq!(plan.status, PlanStatus::Satisfiable);
        assert!(plan
            .planned_bindings
            .iter()
            .any(|binding| binding.requirement == "shell"));
        assert!(plan
            .omissions
            .iter()
            .any(|omission| omission.requirement == "affordance:gpu"));
    }

    assert_ne!(
        local_plan.planned_bindings[0].provider_ref,
        ubuntu_plan.planned_bindings[0].provider_ref
    );
    assert_ne!(
        ubuntu_plan.planned_bindings[0].provider_ref,
        distributed_plan.planned_bindings[0].provider_ref
    );
    assert_ne!(local_discovery.workcell_ref, ubuntu_discovery.workcell_ref);
    assert_ne!(ubuntu_discovery.workcell_ref, distributed_discovery.workcell_ref);
    assert_ne!(local_discovery.capacity, ubuntu_discovery.capacity);
    assert_ne!(ubuntu_discovery.capacity, distributed_discovery.capacity);
    assert_eq!(local_discovery.health, HealthState::Healthy);
    assert_eq!(ubuntu_discovery.health, HealthState::Healthy);
    assert_eq!(distributed_discovery.health, HealthState::Degraded);
}

#[test]
fn removing_optional_provider_changes_offers_not_semantic_demand() {
    let demand = semantic_demand();
    let profile = local_profile();

    let (without_discovery, without_plan) = plan_with(
        &profile,
        FixtureExecutionProvider::new(
            "provider:profile-shell-only",
            "offer:profile-shell-only",
            "same-host",
        ),
        &demand,
    );
    let (with_discovery, with_plan) = plan_with(
        &profile,
        FixtureExecutionProvider::new(
            "provider:profile-shell-gpu",
            "offer:profile-shell-gpu",
            "same-host",
        )
        .with_affordance("gpu"),
        &demand,
    );

    assert_eq!(without_plan.demand_ref, demand.demand_ref);
    assert_eq!(with_plan.demand_ref, demand.demand_ref);
    assert_eq!(without_plan.status, PlanStatus::Satisfiable);
    assert_eq!(with_plan.status, PlanStatus::Satisfiable);
    assert_eq!(without_discovery.offers.len(), with_discovery.offers.len());
    assert!(without_plan
        .omissions
        .iter()
        .any(|omission| omission.requirement == "affordance:gpu"));
    assert!(!with_plan
        .omissions
        .iter()
        .any(|omission| omission.requirement == "affordance:gpu"));
    assert_eq!(demand.affordances.required[0].as_str(), "shell");
    assert_eq!(demand.affordances.optional[0].as_str(), "gpu");
}

#[test]
fn provider_removal_changes_discovery_offer_set_only() {
    let profile = local_profile();
    let mut with_optional = profile.control_plane().unwrap();
    with_optional
        .register_execution_provider(FixtureExecutionProvider::new(
            "provider:profile-required",
            "offer:profile-required",
            "same-host",
        ))
        .unwrap();
    with_optional
        .register_execution_provider(
            FixtureExecutionProvider::new(
                "provider:profile-optional",
                "offer:profile-optional",
                "same-host",
            )
            .with_affordance("gpu"),
        )
        .unwrap();

    let mut without_optional = profile.control_plane().unwrap();
    without_optional
        .register_execution_provider(FixtureExecutionProvider::new(
            "provider:profile-required",
            "offer:profile-required",
            "same-host",
        ))
        .unwrap();

    let with = with_optional.discover().unwrap();
    let without = without_optional.discover().unwrap();
    assert_eq!(with.workcell_ref, without.workcell_ref);
    assert_eq!(with.health, without.health);
    assert_eq!(with.capacity, without.capacity);
    assert_eq!(with.offers.len(), without.offers.len() + 1);
}

#[test]
fn parity_report_describes_physical_difference_without_new_semantic_types() {
    let report = deployment_parity_report([
        local_profile(),
        ubuntu_profile(),
        distributed_profile(),
    ])
    .unwrap();
    assert_eq!(report.len(), 3);
    assert_eq!(report[0].profile_id, "collapsed-local");
    assert_eq!(report[1].profile_id, "distributed-fake-provider");
    assert_eq!(report[2].profile_id, "reference-ubuntu-worker");
    assert_eq!(
        report[0]
            .placements
            .get("execution")
            .map(PlacementRef::as_str),
        Some("same-host")
    );
    assert_eq!(
        report[1]
            .placements
            .get("state")
            .map(PlacementRef::as_str),
        Some("state-domain-b")
    );
}

#[test]
fn semantic_demand_and_profile_types_do_not_encode_reference_deployment_ontology() {
    let demand_source = include_str!("../../workcell-core/src/demand.rs").to_ascii_lowercase();
    assert!(!demand_source.contains("ubuntu"));
    assert!(!demand_source.contains("docker"));
    assert!(!demand_source.contains("kubernetes"));
    assert!(!demand_source.contains("cluster"));
    assert!(!demand_source.contains("192.168."));

    let profile_source = include_str!("../src/profile.rs").to_ascii_lowercase();
    assert!(!profile_source.contains("kubernetes"));
    assert!(!profile_source.contains("cluster"));
    assert!(!profile_source.contains("ubuntu"));
    assert!(!profile_source.contains("distributed"));
}
