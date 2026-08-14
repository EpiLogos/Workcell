use std::{
    collections::{BTreeMap, VecDeque},
    sync::{Arc, Mutex},
};

use epilogos_workcell_arrakis::{
    ArrakisCommand, ArrakisCommandOutput, ArrakisCommandRunner, ArrakisExecutionConfig,
    ArrakisExecutionProvider, ArrakisHostProbe, ARRAKIS_API_VERSION, ARRAKIS_SOURCE_REVISION,
};
use epilogos_workcell_core::{
    plan, validate_allocation, validate_provider_port, AffordanceRequirement, Availability,
    DemandRef, Discovery, ExecutionDemand, ExecutionMaterialRequest, ExecutionProvider, HealthState,
    IsolationTrustRequirement, OfferRef, OperationalOffer, PlanStatus, ProviderAllocation,
    ProviderObservation, ProviderOperation, ProviderOperationResult, ProviderPort, ProviderPortKind,
    ProviderRef, ProviderReleaseResult, ReleaseDisposition, ResourceRequirement, Result,
    RetentionExpectation, WorkcellError, WorkcellRef,
};

#[derive(Default)]
struct ScriptedRunner {
    responses: Mutex<VecDeque<Result<ArrakisCommandOutput>>>,
    commands: Mutex<Vec<ArrakisCommand>>,
}

impl ScriptedRunner {
    fn new(responses: impl IntoIterator<Item = Result<ArrakisCommandOutput>>) -> Arc<Self> {
        Arc::new(Self {
            responses: Mutex::new(responses.into_iter().collect()),
            commands: Mutex::new(Vec::new()),
        })
    }

    fn commands(&self) -> Vec<ArrakisCommand> {
        self.commands.lock().unwrap().clone()
    }
}

impl ArrakisCommandRunner for ScriptedRunner {
    fn run(&self, command: &ArrakisCommand) -> Result<ArrakisCommandOutput> {
        self.commands.lock().unwrap().push(command.clone());
        self.responses
            .lock()
            .unwrap()
            .pop_front()
            .expect("scripted Arrakis response exhausted")
    }
}

struct StaticHostProbe(bool);

impl ArrakisHostProbe for StaticHostProbe {
    fn local_kvm_available(&self) -> bool {
        self.0
    }
}

fn output(stdout: &str) -> Result<ArrakisCommandOutput> {
    Ok(ArrakisCommandOutput {
        stdout: stdout.into(),
        stderr: String::new(),
    })
}

fn config() -> ArrakisExecutionConfig {
    ArrakisExecutionConfig::new("arrakis-client").unwrap()
}

fn material_request() -> ExecutionMaterialRequest {
    ExecutionMaterialRequest {
        demand_ref: DemandRef::new("demand:arrakis-material").unwrap(),
        affordances: vec!["shell".into(), "snapshot".into()],
        resources: vec![],
        connectivity: vec![],
        isolation_trust: Some(IsolationTrustRequirement::new("strong-isolation").unwrap()),
        retention: RetentionExpectation::Release,
    }
}

#[test]
fn arrakis_provider_conforms_and_uses_first_party_client_surface() {
    let runner = ScriptedRunner::new([
        output("Available VMs:\n"),
        output("Available VMs:\n"),
        output("started VM\n"),
        output("hello\n"),
        output("VM Name: epilogos\nStatus: RUNNING\n"),
        output("snapshot created\n"),
        output("destroyed\n"),
    ]);
    let host = Arc::new(StaticHostProbe(true));
    let mut provider = ArrakisExecutionProvider::with_adapters(
        ProviderRef::new("provider:arrakis").unwrap(),
        config().require_local_kvm(true),
        runner.clone(),
        host,
    );

    validate_provider_port(&provider).unwrap();
    let request = material_request();
    let allocation = provider.prepare_execution(&request).unwrap();
    validate_allocation(&provider, &allocation).unwrap();
    assert_eq!(
        allocation
            .provenance
            .get("arrakis_source_revision")
            .map(String::as_str),
        Some(ARRAKIS_SOURCE_REVISION)
    );
    assert_eq!(
        allocation
            .provenance
            .get("arrakis_api_version")
            .map(String::as_str),
        Some(ARRAKIS_API_VERSION)
    );

    let shell = provider
        .execute_operation(
            &allocation,
            &ProviderOperation {
                key: "shell".into(),
                parameters: BTreeMap::from([("command".into(), "printf hello".into())]),
            },
        )
        .unwrap();
    assert_eq!(shell.output.get("stdout").map(String::as_str), Some("hello\n"));
    assert_eq!(provider.observe_execution(&allocation).unwrap().health, HealthState::Healthy);

    let snapshot = provider
        .snapshot_execution(&allocation, Some("checkpoint-exact"))
        .unwrap();
    assert_eq!(
        snapshot.output.get("snapshot_id").map(String::as_str),
        Some("checkpoint-exact")
    );
    assert_eq!(
        snapshot
            .provenance
            .get("arrakis_snapshot_id")
            .map(String::as_str),
        Some("checkpoint-exact")
    );
    assert_eq!(
        snapshot
            .provenance
            .get("arrakis_source_revision")
            .map(String::as_str),
        Some(ARRAKIS_SOURCE_REVISION)
    );

    let released = provider
        .release_execution(&allocation, &RetentionExpectation::Release)
        .unwrap();
    assert_eq!(released.disposition, ReleaseDisposition::Released);

    let commands = runner.commands();
    assert!(commands.iter().any(|command| command.args.iter().any(|arg| arg == "start")));
    assert!(commands.iter().any(|command| {
        command.args.windows(2).any(|window| {
            window == ["--id".to_string(), "checkpoint-exact".to_string()]
        })
    }));
    assert!(commands.iter().all(|command| {
        !command.args.iter().any(|arg| arg == "cloud-hypervisor")
            && !command.args.iter().any(|arg| arg == "firecracker")
    }));
}

#[test]
fn snapshot_destroy_restore_preserves_exact_provider_provenance() {
    let runner = ScriptedRunner::new([
        output("Available VMs:\n"),
        output("started VM\n"),
        output("snapshot created\n"),
        output("destroyed\n"),
        output("restored\n"),
        output("VM Name: restored\nStatus: RUNNING\n"),
    ]);
    let mut provider = ArrakisExecutionProvider::with_adapters(
        ProviderRef::new("provider:arrakis-restore").unwrap(),
        config(),
        runner,
        Arc::new(StaticHostProbe(false)),
    );
    let allocation = provider.prepare_execution(&material_request()).unwrap();
    let snapshot = provider
        .snapshot_execution(&allocation, Some("snapshot:run-184-B"))
        .unwrap();
    provider
        .release_execution(&allocation, &RetentionExpectation::Release)
        .unwrap();
    let restored = provider
        .restore_execution(&allocation, "snapshot:run-184-B")
        .unwrap();
    assert_eq!(
        snapshot
            .provenance
            .get("arrakis_snapshot_id")
            .map(String::as_str),
        Some("snapshot:run-184-B")
    );
    assert_eq!(
        restored
            .provenance
            .get("arrakis_restored_snapshot_id")
            .map(String::as_str),
        Some("snapshot:run-184-B")
    );
    assert_eq!(snapshot.material_ref, restored.material_ref);
    assert_eq!(snapshot.provider_ref, restored.provider_ref);
    assert_eq!(provider.observe_execution(&allocation).unwrap().health, HealthState::Healthy);
}

#[test]
fn local_arrakis_offer_is_absent_when_kvm_is_unavailable() {
    let runner = ScriptedRunner::new([]);
    let mut provider = ArrakisExecutionProvider::with_adapters(
        ProviderRef::new("provider:arrakis-no-kvm").unwrap(),
        config().require_local_kvm(true),
        runner.clone(),
        Arc::new(StaticHostProbe(false)),
    );
    assert!(provider.offers().unwrap().is_empty());
    assert!(matches!(
        provider.prepare_execution(&material_request()),
        Err(WorkcellError::Unavailable(_))
    ));
    assert!(runner.commands().is_empty());
}

#[test]
fn remote_arrakis_does_not_require_kvm_on_the_client_host() {
    let runner = ScriptedRunner::new([output("Available VMs:\n")]);
    let provider = ArrakisExecutionProvider::with_adapters(
        ProviderRef::new("provider:arrakis-remote").unwrap(),
        config(),
        runner,
        Arc::new(StaticHostProbe(false)),
    );
    assert_eq!(provider.offers().unwrap().len(), 1);
}

#[test]
fn arrakis_service_disappearance_is_explicit() {
    let runner = ScriptedRunner::new([
        Err(WorkcellError::Unavailable("restserver unavailable".into())),
        Err(WorkcellError::Unavailable("restserver unavailable".into())),
    ]);
    let mut provider = ArrakisExecutionProvider::with_adapters(
        ProviderRef::new("provider:arrakis-missing").unwrap(),
        config(),
        runner,
        Arc::new(StaticHostProbe(false)),
    );
    assert!(provider.offers().unwrap().is_empty());
    assert!(matches!(
        provider.prepare_execution(&material_request()),
        Err(WorkcellError::Unavailable(_))
    ));
}

#[test]
fn unsupported_per_vm_resource_sizing_is_rejected_not_ignored() {
    let runner = ScriptedRunner::new([output("Available VMs:\n")]);
    let mut provider = ArrakisExecutionProvider::with_adapters(
        ProviderRef::new("provider:arrakis-resources").unwrap(),
        config(),
        runner,
        Arc::new(StaticHostProbe(false)),
    );
    let mut request = material_request();
    request.resources.push(ResourceRequirement {
        key: "memory".into(),
        minimum: Some(8),
        unit: Some("GiB".into()),
    });
    assert!(matches!(
        provider.prepare_execution(&request),
        Err(WorkcellError::UnsatisfiedDemand(_))
    ));
}

struct ReferenceExecutionProvider {
    provider_ref: ProviderRef,
    offer_ref: OfferRef,
}

impl ReferenceExecutionProvider {
    fn new() -> Self {
        Self {
            provider_ref: ProviderRef::new("provider:reference-isolation").unwrap(),
            offer_ref: OfferRef::new("offer:reference-isolation").unwrap(),
        }
    }
}

impl ProviderPort for ReferenceExecutionProvider {
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
            affordances: vec!["shell".into(), "snapshot".into()],
            connections: vec![],
            exposures: vec![],
            isolation_trust: vec!["strong-isolation".into()],
            availability: Availability::Available,
            health: HealthState::Healthy,
            capacity: BTreeMap::new(),
            metadata: BTreeMap::from([("implementation".into(), "reference".into())]),
        }])
    }
}

impl ExecutionProvider for ReferenceExecutionProvider {
    fn prepare_execution(&mut self, _: &ExecutionMaterialRequest) -> Result<ProviderAllocation> {
        Err(WorkcellError::Unsupported("planning-only reference provider".into()))
    }

    fn execute_operation(
        &mut self,
        _: &ProviderAllocation,
        _: &ProviderOperation,
    ) -> Result<ProviderOperationResult> {
        Err(WorkcellError::Unsupported("planning-only reference provider".into()))
    }

    fn observe_execution(&self, _: &ProviderAllocation) -> Result<ProviderObservation> {
        Err(WorkcellError::Unsupported("planning-only reference provider".into()))
    }

    fn release_execution(
        &mut self,
        _: &ProviderAllocation,
        _: &RetentionExpectation,
    ) -> Result<ProviderReleaseResult> {
        Err(WorkcellError::Unsupported("planning-only reference provider".into()))
    }
}

fn semantic_isolated_demand() -> ExecutionDemand {
    let mut demand = ExecutionDemand::new(DemandRef::new("demand:portable-isolation").unwrap());
    demand
        .affordances
        .required
        .push(AffordanceRequirement::new("shell").unwrap());
    demand
        .affordances
        .preferred
        .push(AffordanceRequirement::new("snapshot").unwrap());
    demand.isolation_trust = Some(IsolationTrustRequirement::new("strong-isolation").unwrap());
    demand
}

#[test]
fn same_semantic_demand_can_plan_reference_or_arrakis_without_brand_selection() {
    let demand = semantic_isolated_demand();
    let reference = ReferenceExecutionProvider::new();
    let reference_discovery = Discovery {
        workcell_ref: WorkcellRef::new("workcell:reference-plan").unwrap(),
        health: HealthState::Healthy,
        capacity: BTreeMap::new(),
        offers: reference.offers().unwrap(),
    };
    let reference_plan = plan(&demand, &reference_discovery).unwrap();

    let runner = ScriptedRunner::new([output("Available VMs:\n")]);
    let arrakis = ArrakisExecutionProvider::with_adapters(
        ProviderRef::new("provider:arrakis-plan").unwrap(),
        config(),
        runner,
        Arc::new(StaticHostProbe(false)),
    );
    let arrakis_discovery = Discovery {
        workcell_ref: WorkcellRef::new("workcell:arrakis-plan").unwrap(),
        health: HealthState::Healthy,
        capacity: BTreeMap::new(),
        offers: arrakis.offers().unwrap(),
    };
    let arrakis_plan = plan(&demand, &arrakis_discovery).unwrap();

    assert_eq!(reference_plan.demand_ref, demand.demand_ref);
    assert_eq!(arrakis_plan.demand_ref, demand.demand_ref);
    assert_eq!(reference_plan.status, PlanStatus::Satisfiable);
    assert_eq!(arrakis_plan.status, PlanStatus::Satisfiable);
    assert_eq!(demand.affordances.required[0].as_str(), "shell");
    assert_eq!(
        demand.isolation_trust.as_ref().map(|item| item.as_str()),
        Some("strong-isolation")
    );
    assert_ne!(
        reference_plan.planned_bindings[0].provider_ref,
        arrakis_plan.planned_bindings[0].provider_ref
    );
}

#[test]
fn core_semantic_demand_does_not_acquire_arrakis_or_microvm_brands() {
    let source = include_str!("../../workcell-core/src/demand.rs").to_ascii_lowercase();
    assert!(!source.contains("arrakis"));
    assert!(!source.contains("microvm"));
    assert!(!source.contains("cloud-hypervisor"));
    assert!(!source.contains("/dev/kvm"));
}
