use std::{collections::BTreeMap, env};

use epilogos_workcell_core::{
    plan, Availability, Capacity, DemandRef, Discovery, ExecutionDemand, ExecutionMaterialRequest,
    ExecutionProvider, ExternalRef, HealthState, LogicalConnectionRequirement, OfferRef,
    OperationalOffer, PlanStatus, ProviderOperation, ProviderPort, ProviderPortKind, ProviderRef,
    ResourceRequirement, RetentionExpectation, ServiceMaterialRequest, ServiceProvider, WorkcellRef,
};
use epilogos_workcell_runtime::{
    HostProcessExecutionProvider, ManagedHostService, ManagedHostServiceProvider, TcpEndpointProbe,
};

const OLLAMA_REVISION: &str = "d67ad83426633195089509347ffd4fe795120198";
const LLAMA_CPP_REVISION: &str = "4df29be4f4c3673f428170fda944a5b19f743bb8";
const VLLM_REVISION: &str = "6b0b850a8b1764a66d7ffbb023c0b0e0bbdb900b";
const LOGICAL_INFERENCE_SERVICE: &str = "inference:caller-owned-service";

fn service_request(demand_ref: &str) -> ServiceMaterialRequest {
    ServiceMaterialRequest {
        demand_ref: DemandRef::new(demand_ref).unwrap(),
        connection: LogicalConnectionRequirement::new(LOGICAL_INFERENCE_SERVICE).unwrap(),
        persistence: None,
    }
}

fn ollama_service(program: &str, port: u16) -> ManagedHostService {
    ManagedHostService::new(
        LOGICAL_INFERENCE_SERVICE,
        format!("http://127.0.0.1:{port}"),
        program,
    )
    .unwrap()
    .with_arg("serve")
    .with_env("OLLAMA_HOST", format!("127.0.0.1:{port}"))
    .with_metadata("engine", "ollama")
    .with_metadata("upstream_revision", OLLAMA_REVISION)
    .with_tcp_readiness(
        TcpEndpointProbe::new("127.0.0.1", port)
            .unwrap()
            .with_timeout_ms(20_000),
    )
}

fn llama_server_service(program: &str, model_path: &str, port: u16) -> ManagedHostService {
    ManagedHostService::new(
        LOGICAL_INFERENCE_SERVICE,
        format!("http://127.0.0.1:{port}"),
        program,
    )
    .unwrap()
    .with_arg("-m")
    .with_arg(model_path)
    .with_arg("--host")
    .with_arg("127.0.0.1")
    .with_arg("--port")
    .with_arg(port.to_string())
    .with_metadata("engine", "llama.cpp")
    .with_metadata("upstream_revision", LLAMA_CPP_REVISION)
    .with_metadata("provider_model_path", model_path)
    .with_tcp_readiness(
        TcpEndpointProbe::new("127.0.0.1", port)
            .unwrap()
            .with_timeout_ms(60_000),
    )
}

fn vllm_service(program: &str, provider_model_id: &str, port: u16) -> ManagedHostService {
    ManagedHostService::new(
        LOGICAL_INFERENCE_SERVICE,
        format!("http://127.0.0.1:{port}"),
        program,
    )
    .unwrap()
    .with_arg("serve")
    .with_arg(provider_model_id)
    .with_arg("--host")
    .with_arg("127.0.0.1")
    .with_arg("--port")
    .with_arg(port.to_string())
    .with_metadata("engine", "vllm")
    .with_metadata("upstream_revision", VLLM_REVISION)
    .with_metadata("provider_model_id", provider_model_id)
    .with_tcp_readiness(
        TcpEndpointProbe::new("127.0.0.1", port)
            .unwrap()
            .with_timeout_ms(180_000)
            .with_interval_ms(100),
    )
}

fn material_demand(placement: &str) -> ExecutionDemand {
    let mut demand = ExecutionDemand::new(DemandRef::new("demand:model-serving-runtime").unwrap())
        .with_subject(
            "model",
            ExternalRef::new("model:caller-owned/opaque").unwrap(),
        )
        .with_subject(
            "variant",
            ExternalRef::new("variant:caller-owned/opaque").unwrap(),
        );
    demand
        .connectivity
        .required
        .push(LogicalConnectionRequirement::new(LOGICAL_INFERENCE_SERVICE).unwrap());
    demand
        .extensions
        .insert("placement".into(), placement.into());
    demand
}

fn vllm_material_demand(placement: &str) -> ExecutionDemand {
    let mut demand = material_demand(placement);
    demand.resources.push(ResourceRequirement {
        key: "accelerator".into(),
        minimum: Some(1),
        unit: Some("device".into()),
    });
    demand
}

fn gpu_offer(provider_ref: &ProviderRef, amount: u64) -> OperationalOffer {
    OperationalOffer {
        offer_ref: OfferRef::new(format!("offer:{provider_ref}:gpu")).unwrap(),
        provider_ref: provider_ref.clone(),
        port: ProviderPortKind::Execution.as_str().into(),
        affordances: vec!["execution:remote".into()],
        connections: vec![],
        exposures: vec![],
        isolation_trust: vec![],
        availability: Availability::Available,
        health: HealthState::Healthy,
        capacity: BTreeMap::from([(
            "accelerator".into(),
            Capacity {
                amount,
                unit: Some("device".into()),
            },
        )]),
        metadata: BTreeMap::from([("placement".into(), "remote".into())]),
    }
}

#[test]
fn provider_specific_recipes_remain_material_facts_on_one_service_contract() {
    let fixtures = [
        ollama_service("ollama", 11434),
        llama_server_service("llama-server", "/models/opaque.gguf", 18080),
        vllm_service("vllm", "provider-native-model-id", 18000),
    ];

    for fixture in fixtures {
        assert_eq!(fixture.logical_ref, LOGICAL_INFERENCE_SERVICE);
        assert!(fixture.endpoint.starts_with("http://127.0.0.1:"));
        assert!(fixture.metadata.contains_key("engine"));
        assert!(fixture.metadata.contains_key("upstream_revision"));
    }

    assert_eq!(OLLAMA_REVISION.len(), 40);
    assert_eq!(LLAMA_CPP_REVISION.len(), 40);
    assert_eq!(VLLM_REVISION.len(), 40);
}

#[test]
fn vllm_accelerator_requirement_fails_without_capacity_and_plans_with_remote_capacity() {
    let demand = vllm_material_demand("remote");
    let service_provider = ManagedHostServiceProvider::new(
        ProviderRef::new("provider:vllm-service-shape").unwrap(),
        [vllm_service("vllm", "provider-native-model-id", 18000)],
    )
    .unwrap();
    let mut offers = service_provider.offers().unwrap();
    let no_gpu = plan(
        &demand,
        &Discovery {
            workcell_ref: WorkcellRef::new("workcell:no-gpu").unwrap(),
            health: HealthState::Healthy,
            capacity: BTreeMap::new(),
            offers: offers.clone(),
        },
    )
    .unwrap();
    assert_eq!(no_gpu.status, PlanStatus::Unsatisfiable);
    assert!(no_gpu
        .omissions
        .iter()
        .any(|item| item.requirement == "resource:accelerator"));

    let remote_gpu = ProviderRef::new("provider:remote-gpu-execution").unwrap();
    offers.push(gpu_offer(&remote_gpu, 1));
    let with_gpu = plan(
        &demand,
        &Discovery {
            workcell_ref: WorkcellRef::new("workcell:remote-gpu").unwrap(),
            health: HealthState::Healthy,
            capacity: BTreeMap::new(),
            offers,
        },
    )
    .unwrap();
    assert_eq!(with_gpu.status, PlanStatus::Satisfiable);
    assert!(with_gpu
        .planned_bindings
        .iter()
        .any(|binding| binding.provider_ref == remote_gpu));
    assert_eq!(demand.subjects["model"].as_str(), "model:caller-owned/opaque");
}

#[test]
fn llama_cpp_direct_execution_is_an_ordinary_process_operation_not_a_service_ontology() {
    let operation = ProviderOperation {
        key: "process".into(),
        parameters: BTreeMap::from([
            ("program".into(), "llama-cli".into()),
            ("arg.0".into(), "-m".into()),
            ("arg.1".into(), "/models/opaque.gguf".into()),
            ("arg.2".into(), "-p".into()),
            ("arg.3".into(), "workcell conformance".into()),
            ("arg.4".into(), "-n".into()),
            ("arg.5".into(), "1".into()),
        ]),
    };
    assert_eq!(operation.key, "process");
    assert_eq!(operation.parameters["program"], "llama-cli");
}

#[test]
fn live_ollama_service_lifecycle_when_explicitly_enabled() {
    if env::var("WORKCELL_OLLAMA_LIVE").as_deref() != Ok("1") {
        return;
    }
    let program = env::var("WORKCELL_OLLAMA_BIN").unwrap_or_else(|_| "ollama".into());
    let port = env::var("WORKCELL_OLLAMA_PORT")
        .ok()
        .and_then(|value| value.parse().ok())
        .unwrap_or(21434);
    let mut provider = ManagedHostServiceProvider::new(
        ProviderRef::new("provider:ollama-live").unwrap(),
        [ollama_service(&program, port)],
    )
    .unwrap();
    let allocation = provider
        .resolve_service(&service_request("demand:ollama-live"))
        .unwrap();
    assert_eq!(provider.observe_service(&allocation).unwrap().health, HealthState::Healthy);
    provider
        .release_service(&allocation, &RetentionExpectation::Release)
        .unwrap();
}

#[test]
fn live_llama_cpp_direct_and_server_forms_when_explicitly_enabled() {
    if env::var("WORKCELL_LLAMA_CPP_LIVE").as_deref() != Ok("1") {
        return;
    }
    let cli = env::var("WORKCELL_LLAMA_CPP_CLI").unwrap_or_else(|_| "llama-cli".into());
    let server = env::var("WORKCELL_LLAMA_CPP_SERVER").unwrap_or_else(|_| "llama-server".into());
    let model = env::var("WORKCELL_LLAMA_CPP_MODEL").expect("WORKCELL_LLAMA_CPP_MODEL is required");
    let port = env::var("WORKCELL_LLAMA_CPP_PORT")
        .ok()
        .and_then(|value| value.parse().ok())
        .unwrap_or(28080);

    let mut execution = HostProcessExecutionProvider::new(
        ProviderRef::new("provider:llama-cpp-direct-live").unwrap(),
    );
    let allocation = execution
        .prepare_execution(&ExecutionMaterialRequest {
            demand_ref: DemandRef::new("demand:llama-cpp-direct-live").unwrap(),
            affordances: vec!["process-execution".into()],
            resources: vec![],
            connectivity: vec![],
            isolation_trust: None,
            retention: RetentionExpectation::Release,
        })
        .unwrap();
    let direct = execution
        .execute_operation(
            &allocation,
            &ProviderOperation {
                key: "process".into(),
                parameters: BTreeMap::from([
                    ("program".into(), cli),
                    ("arg.0".into(), "-m".into()),
                    ("arg.1".into(), model.clone()),
                    ("arg.2".into(), "-p".into()),
                    ("arg.3".into(), "Workcell conformance".into()),
                    ("arg.4".into(), "-n".into()),
                    ("arg.5".into(), "1".into()),
                ]),
            },
        )
        .unwrap();
    assert_eq!(direct.output.get("success").map(String::as_str), Some("true"));

    let mut services = ManagedHostServiceProvider::new(
        ProviderRef::new("provider:llama-cpp-server-live").unwrap(),
        [llama_server_service(&server, &model, port)],
    )
    .unwrap();
    let service = services
        .resolve_service(&service_request("demand:llama-cpp-server-live"))
        .unwrap();
    assert_eq!(services.observe_service(&service).unwrap().health, HealthState::Healthy);
    services
        .release_service(&service, &RetentionExpectation::Release)
        .unwrap();
}

#[test]
fn live_vllm_service_when_gpu_environment_is_explicitly_enabled() {
    if env::var("WORKCELL_VLLM_LIVE").as_deref() != Ok("1") {
        return;
    }
    let program = env::var("WORKCELL_VLLM_BIN").unwrap_or_else(|_| "vllm".into());
    let model = env::var("WORKCELL_VLLM_MODEL").expect("WORKCELL_VLLM_MODEL is required");
    let port = env::var("WORKCELL_VLLM_PORT")
        .ok()
        .and_then(|value| value.parse().ok())
        .unwrap_or(28000);
    let mut provider = ManagedHostServiceProvider::new(
        ProviderRef::new("provider:vllm-live").unwrap(),
        [vllm_service(&program, &model, port)],
    )
    .unwrap();
    let allocation = provider
        .resolve_service(&service_request("demand:vllm-live"))
        .unwrap();
    assert_eq!(provider.observe_service(&allocation).unwrap().health, HealthState::Healthy);
    provider
        .release_service(&allocation, &RetentionExpectation::Release)
        .unwrap();
}
