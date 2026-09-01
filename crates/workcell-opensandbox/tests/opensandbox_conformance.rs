use std::{
    collections::BTreeMap,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc, Mutex,
    },
};

use epilogos_workcell_core::{
    AffordanceRequirement, Availability, CheckpointRequest, DemandRef, DesiredMaterialState,
    ExecutionDemand, ExecutionMaterialRequest, ExecutionProvider, ExternalRef, HealthState,
    MaterialCheckpointProvider, MaterialisationPlan, OfferRef, PersistenceScope, PlanRef,
    PlanStatus, PlannedAllocation, PlannedBinding, ProviderAllocation, ProviderPort,
    ProviderPortKind, ProviderRef, RequirementNecessity, ResourceRequirement, RetentionExpectation,
    WorkcellControlPlane, WorkcellError, WorkcellRef,
};
use epilogos_workcell_opensandbox::{
    compose_opensandbox_material_world, OpenSandboxConfig, OpenSandboxExecutionProvider,
    OpenSandboxHttpRequest, OpenSandboxHttpResponse, OpenSandboxMaterialComposition,
    OpenSandboxStartupSource, OpenSandboxTransport, OPENSANDBOX_SOURCE_REVISION,
};
use serde_json::{json, Value};

#[derive(Clone)]
struct RecordingTransport {
    requests: Arc<Mutex<Vec<OpenSandboxHttpRequest>>>,
    responses: Arc<Mutex<Vec<OpenSandboxHttpResponse>>>,
}

impl RecordingTransport {
    fn new(responses: Vec<OpenSandboxHttpResponse>) -> Self {
        Self {
            requests: Arc::new(Mutex::new(Vec::new())),
            responses: Arc::new(Mutex::new(responses.into_iter().rev().collect())),
        }
    }

    fn requests(&self) -> Vec<OpenSandboxHttpRequest> {
        self.requests.lock().unwrap().clone()
    }
}

impl OpenSandboxTransport for RecordingTransport {
    fn request(
        &self,
        request: OpenSandboxHttpRequest,
    ) -> epilogos_workcell_core::Result<OpenSandboxHttpResponse> {
        self.requests.lock().unwrap().push(request);
        self.responses
            .lock()
            .unwrap()
            .pop()
            .ok_or_else(|| WorkcellError::OperationFailed("fixture response exhausted".into()))
    }
}

#[derive(Clone)]
struct AvailabilityTransport {
    available: Arc<AtomicBool>,
}

impl AvailabilityTransport {
    fn new(initially_available: bool) -> Self {
        Self {
            available: Arc::new(AtomicBool::new(initially_available)),
        }
    }

    fn set_available(&self, available: bool) {
        self.available.store(available, Ordering::SeqCst);
    }
}

impl OpenSandboxTransport for AvailabilityTransport {
    fn request(
        &self,
        request: OpenSandboxHttpRequest,
    ) -> epilogos_workcell_core::Result<OpenSandboxHttpResponse> {
        if !self.available.load(Ordering::SeqCst) {
            return Err(WorkcellError::Unavailable(
                "fixture OpenSandbox lifecycle endpoint unavailable".into(),
            ));
        }
        if request.method == "GET" && request.url.contains("page=1&pageSize=1") {
            return Ok(json_response(200, json!({"items": [], "total": 0})));
        }
        if request.method == "GET" && request.url.contains("/sandboxes/sbx_stable") {
            return Ok(json_response(
                200,
                json!({
                    "id": "sbx_stable",
                    "status": {"state": "Running"},
                    "expiresAt": "2026-09-01T16:00:00Z"
                }),
            ));
        }
        Err(WorkcellError::OperationFailed(format!(
            "unexpected lifecycle fixture request {} {}",
            request.method, request.url
        )))
    }
}

fn json_response(status: u16, value: Value) -> OpenSandboxHttpResponse {
    OpenSandboxHttpResponse {
        status,
        headers: BTreeMap::new(),
        body: if value.is_null() {
            Vec::new()
        } else {
            serde_json::to_vec(&value).unwrap()
        },
    }
}

fn provider_ref() -> ProviderRef {
    ProviderRef::new("provider:opensandbox").unwrap()
}

fn image_config(base_url: &str) -> OpenSandboxConfig {
    let mut config = OpenSandboxConfig::local(
        provider_ref(),
        "opensandbox/code-interpreter:v1.1.0",
        vec!["/opt/code-interpreter/code-interpreter.sh".into()],
    )
    .unwrap();
    config.lifecycle_base_url = base_url.into();
    config.api_key_env = None;
    config.capacity.insert(
        "memory".into(),
        epilogos_workcell_core::Capacity {
            amount: 16,
            unit: Some("GiB".into()),
        },
    );
    config
}

fn execution_request() -> ExecutionMaterialRequest {
    ExecutionMaterialRequest {
        demand_ref: DemandRef::new("demand:portable-world").unwrap(),
        affordances: vec!["shell".into(), "filesystem".into()],
        resources: vec![ResourceRequirement {
            key: "memory".into(),
            minimum: Some(4),
            unit: Some("GiB".into()),
        }],
        connectivity: Vec::new(),
        isolation_trust: None,
        retention: RetentionExpectation::Preserve,
    }
}

fn stable_execution_allocation() -> ProviderAllocation {
    ProviderAllocation {
        provider_ref: provider_ref(),
        port: ProviderPortKind::Execution,
        material_ref: "sbx_stable".into(),
        health: HealthState::Healthy,
        properties: BTreeMap::new(),
        provenance: BTreeMap::from([(
            "upstream.revision".into(),
            OPENSANDBOX_SOURCE_REVISION.into(),
        )]),
    }
}

fn stable_world() -> epilogos_workcell_core::MaterialisedExecutionWorld {
    let demand_ref = DemandRef::new("demand:stable-world").unwrap();
    let mut demand = ExecutionDemand::new(demand_ref.clone())
        .with_subject("project", ExternalRef::new("project:stable").unwrap());
    demand
        .affordances
        .required
        .push(AffordanceRequirement::new("shell").unwrap());
    demand.persistence = Some(PersistenceScope::Project);
    demand.retention = RetentionExpectation::Preserve;
    demand.validate().unwrap();

    let offer_ref = OfferRef::new("offer:provider:opensandbox:execution").unwrap();
    let plan = MaterialisationPlan {
        plan_ref: PlanRef::new("plan:stable-world").unwrap(),
        demand_ref,
        status: PlanStatus::Satisfiable,
        planned_bindings: vec![PlannedBinding {
            logical_ref: "execution:agent".into(),
            requirement: "shell".into(),
            necessity: RequirementNecessity::Required,
            provider_ref: provider_ref(),
            offer_ref: offer_ref.clone(),
        }],
        planned_exposures: Vec::new(),
        planned_constraints: Vec::new(),
        degradations: Vec::new(),
        omissions: Vec::new(),
        explanation: Vec::new(),
    };

    compose_opensandbox_material_world(
        WorkcellRef::new("workcell:fixture").unwrap(),
        &demand,
        &plan,
        OpenSandboxMaterialComposition {
            execution_logical_ref: "execution:agent".into(),
            allocations: vec![PlannedAllocation {
                logical_ref: "execution:agent".into(),
                offer_ref,
                allocation: stable_execution_allocation(),
            }],
            relations: Vec::new(),
        },
    )
    .unwrap()
}

#[test]
fn one_generic_demand_crosses_local_docker_and_remote_cluster_lifecycle_shapes_unchanged() {
    let local_transport = RecordingTransport::new(vec![json_response(
        202,
        json!({"id":"sbx_local","status":{"state":"Running"}}),
    )]);
    let cluster_transport = RecordingTransport::new(vec![json_response(
        202,
        json!({"id":"sbx_cluster","status":{"state":"Running"}}),
    )]);
    let local_inspect = local_transport.clone();
    let cluster_inspect = cluster_transport.clone();

    let mut local = OpenSandboxExecutionProvider::new(
        image_config("http://docker-opensandbox.fixture:8080/v1"),
        local_transport,
    )
    .unwrap();
    let mut cluster = OpenSandboxExecutionProvider::new(
        image_config("http://kubernetes-opensandbox.fixture:8080/v1"),
        cluster_transport,
    )
    .unwrap();
    let demand = execution_request();

    let local_allocation = local.prepare_execution(&demand).unwrap();
    let cluster_allocation = cluster.prepare_execution(&demand).unwrap();

    assert_eq!(
        local_allocation.provider_ref,
        cluster_allocation.provider_ref
    );
    assert_ne!(
        local_allocation.material_ref,
        cluster_allocation.material_ref
    );
    let local_request = local_inspect.requests().pop().unwrap();
    let cluster_request = cluster_inspect.requests().pop().unwrap();
    assert_eq!(local_request.method, "POST");
    assert_eq!(cluster_request.method, "POST");
    assert_ne!(local_request.url, cluster_request.url);
    assert_eq!(local_request.body, cluster_request.body);
    assert!(String::from_utf8(local_request.body)
        .unwrap()
        .contains("demand:portable-world"));
}

#[test]
fn checkpoint_ref_is_directly_reusable_as_provider_local_restore_source() {
    let transport = RecordingTransport::new(vec![json_response(
        202,
        json!({"id":"snap_reusable","status":{"state":"Ready"}}),
    )]);
    let mut provider = OpenSandboxExecutionProvider::new(
        image_config("http://opensandbox.fixture:8080/v1"),
        transport,
    )
    .unwrap();
    let checkpoint = provider
        .checkpoint(
            &stable_execution_allocation(),
            &CheckpointRequest {
                name: Some("stable-project".into()),
            },
        )
        .unwrap();

    assert!(checkpoint.reusable);
    let restore = OpenSandboxConfig::from_snapshot(
        provider_ref(),
        "http://opensandbox.fixture:8080/v1",
        checkpoint.checkpoint_ref.clone(),
    )
    .unwrap();
    assert_eq!(
        restore.startup,
        OpenSandboxStartupSource::Snapshot {
            snapshot_id: "snap_reusable".into()
        }
    );
}

#[test]
fn provider_loss_and_reappearance_reconcile_the_same_world_and_material_ref() {
    let transport = AvailabilityTransport::new(true);
    let controller = transport.clone();
    let provider = OpenSandboxExecutionProvider::new(
        image_config("http://opensandbox.fixture:8080/v1"),
        transport,
    )
    .unwrap();
    let world = stable_world();
    let world_ref = world.world_ref.clone();
    let demand_ref = world.demand_ref.clone();
    let material_ref = world.binding_graph.bindings[0].material_ref.clone();

    let mut control = epilogos_workcell_core::PreparedWorldControlPlane::new(
        WorkcellRef::new("workcell:fixture").unwrap(),
    );
    control.register_world(world).unwrap();
    control.register_execution_provider(provider).unwrap();

    let desired = [DesiredMaterialState {
        logical_ref: "execution:agent".into(),
        desired: "present".into(),
    }];
    let present = control.reconcile(&desired).unwrap();
    assert_eq!(present.deltas[0].action, None);

    controller.set_available(false);
    let lost = control.reconcile(&desired).unwrap();
    assert_eq!(lost.deltas[0].action.as_deref(), Some("recover"));
    let stale_world = control.world(&world_ref).unwrap();
    assert_eq!(stale_world.demand_ref, demand_ref);
    assert_eq!(
        stale_world.binding_graph.bindings[0].material_ref,
        material_ref
    );

    controller.set_available(true);
    let recovered = control.reconcile(&desired).unwrap();
    assert_eq!(recovered.deltas[0].action, None);
    let recovered_world = control.world(&world_ref).unwrap();
    assert_eq!(recovered_world.world_ref, world_ref);
    assert_eq!(recovered_world.demand_ref, demand_ref);
    assert_eq!(
        recovered_world.binding_graph.bindings[0].material_ref,
        material_ref
    );
    assert_eq!(
        recovered_world.binding_graph.bindings[0].health,
        HealthState::Healthy
    );
}

#[test]
fn arbitrary_native_service_ports_are_exposed_without_promoting_app_surface_ontology() {
    let transport = RecordingTransport::new(vec![
        json_response(200, json!({"endpoint":"browser.fixture:9222","headers":{}})),
        json_response(200, json!({"endpoint":"desktop.fixture:6080","headers":{}})),
        json_response(200, json!({"endpoint":"code.fixture:3000","headers":{}})),
    ]);
    let inspect = transport.clone();
    let provider = OpenSandboxExecutionProvider::new(
        image_config("http://opensandbox.fixture:8080/v1"),
        transport,
    )
    .unwrap();
    let allocation = stable_execution_allocation();

    assert_eq!(
        provider
            .endpoint_reading(&allocation, 9222)
            .unwrap()
            .endpoint,
        "browser.fixture:9222"
    );
    assert_eq!(
        provider
            .endpoint_reading(&allocation, 6080)
            .unwrap()
            .endpoint,
        "desktop.fixture:6080"
    );
    assert_eq!(
        provider
            .endpoint_reading(&allocation, 3000)
            .unwrap()
            .endpoint,
        "code.fixture:3000"
    );

    let requests = inspect.requests();
    assert!(requests[0].url.contains("/endpoints/9222"));
    assert!(requests[1].url.contains("/endpoints/6080"));
    assert!(requests[2].url.contains("/endpoints/3000"));
    let offer = provider.offers().unwrap();
    assert_eq!(offer[0].availability, Availability::Unavailable);
    assert_eq!(offer[0].provider_ref, provider_ref());
}
