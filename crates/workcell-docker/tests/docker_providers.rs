use std::{
    collections::{BTreeMap, VecDeque},
    path::PathBuf,
    sync::{Arc, Mutex},
};

use epilogos_workcell_core::{
    validate_allocation, validate_provider_port, DemandRef, ExecutionMaterialRequest,
    ExecutionProvider, ExposureRequirement, LogicalConnectionRequirement, MaterialExposureProvider,
    PersistenceScope, ProjectRuntimeMaterialRequest, ProjectRuntimeProvider,
    ProviderExposureRequest, ProviderOperation, ProviderPort, ProviderRef, RequirementNecessity,
    RetentionExpectation, WorkcellError,
};
use epilogos_workcell_docker::{
    DockerCommand, DockerCommandOutput, DockerCommandRunner, DockerExecutionConfig,
    DockerExecutionProvider, DockerExposureTarget, DockerProjectRuntimeProvider, DockerRuntimeMode,
};

#[derive(Default)]
struct ScriptedRunner {
    responses: Mutex<VecDeque<epilogos_workcell_core::Result<DockerCommandOutput>>>,
    commands: Mutex<Vec<DockerCommand>>,
}

impl ScriptedRunner {
    fn new(
        responses: impl IntoIterator<Item = epilogos_workcell_core::Result<DockerCommandOutput>>,
    ) -> Arc<Self> {
        Arc::new(Self {
            responses: Mutex::new(responses.into_iter().collect()),
            commands: Mutex::new(Vec::new()),
        })
    }

    fn commands(&self) -> Vec<DockerCommand> {
        self.commands.lock().unwrap().clone()
    }
}

impl DockerCommandRunner for ScriptedRunner {
    fn run(&self, command: &DockerCommand) -> epilogos_workcell_core::Result<DockerCommandOutput> {
        self.commands.lock().unwrap().push(command.clone());
        self.responses
            .lock()
            .unwrap()
            .pop_front()
            .expect("scripted Docker response exhausted")
    }
}

fn output(stdout: &str) -> epilogos_workcell_core::Result<DockerCommandOutput> {
    Ok(DockerCommandOutput {
        stdout: stdout.into(),
        stderr: String::new(),
    })
}

fn execution_request() -> ExecutionMaterialRequest {
    ExecutionMaterialRequest {
        demand_ref: DemandRef::new("demand:docker-execution").unwrap(),
        affordances: vec!["shell".into()],
        resources: vec![],
        connectivity: vec![LogicalConnectionRequirement::new("state:graph").unwrap()],
        isolation_trust: None,
        retention: RetentionExpectation::Release,
    }
}

#[test]
fn execution_provider_satisfies_shared_conformance_and_materialises_logical_networks() {
    let runner = ScriptedRunner::new([
        output("29.6.2\n"),
        output("29.6.2\n"),
        output("container-abc\n"),
        output("container-abc\n"),
        output("hello\n"),
        output("running\n"),
        output("container-abc\n"),
        output("container-abc\n"),
    ]);
    let config = DockerExecutionConfig::new("alpine:3.22")
        .unwrap()
        .with_logical_network("state:graph", "physical-graph-net")
        .unwrap();
    let mut provider = DockerExecutionProvider::with_runner(
        ProviderRef::new("provider:docker-execution").unwrap(),
        config,
        runner.clone(),
    );

    validate_provider_port(&provider).unwrap();
    let request = execution_request();
    assert_eq!(request.connectivity[0].as_str(), "state:graph");
    let allocation = provider.prepare_execution(&request).unwrap();
    validate_allocation(&provider, &allocation).unwrap();
    assert_eq!(
        allocation
            .provenance
            .get("container_id")
            .map(String::as_str),
        Some("container-abc")
    );

    let operation = provider
        .execute_operation(
            &allocation,
            &ProviderOperation {
                key: "shell".into(),
                parameters: BTreeMap::from([("command".into(), "printf hello".into())]),
            },
        )
        .unwrap();
    assert_eq!(
        operation.output.get("stdout").map(String::as_str),
        Some("hello\n")
    );

    let observation = provider.observe_execution(&allocation).unwrap();
    assert_eq!(
        observation.detail.get("status").map(String::as_str),
        Some("running")
    );
    provider.restart_execution(&allocation).unwrap();
    provider
        .release_execution(&allocation, &RetentionExpectation::Release)
        .unwrap();

    let commands = runner.commands();
    let create = commands
        .iter()
        .find(|command| {
            command.args.get(0).map(String::as_str) == Some("container")
                && command.args.get(1).map(String::as_str) == Some("create")
        })
        .unwrap();
    assert!(create.args.iter().any(|arg| arg == "physical-graph-net"));
    assert!(!request
        .connectivity
        .iter()
        .any(|connection| connection.as_str() == "physical-graph-net"));
}

#[test]
fn execution_offer_disappears_and_prepare_fails_when_docker_is_unavailable() {
    let runner = ScriptedRunner::new([
        Err(WorkcellError::Unavailable("docker missing".into())),
        Err(WorkcellError::Unavailable("docker missing".into())),
    ]);
    let mut provider = DockerExecutionProvider::with_runner(
        ProviderRef::new("provider:docker-missing").unwrap(),
        DockerExecutionConfig::new("alpine:3.22").unwrap(),
        runner,
    );
    assert!(provider.offers().unwrap().is_empty());
    assert!(matches!(
        provider.prepare_execution(&ExecutionMaterialRequest {
            demand_ref: DemandRef::new("demand:missing").unwrap(),
            affordances: vec![],
            resources: vec![],
            connectivity: vec![],
            isolation_trust: None,
            retention: RetentionExpectation::Release,
        }),
        Err(WorkcellError::Unavailable(_))
    ));
}

fn runtime_mode() -> DockerRuntimeMode {
    DockerRuntimeMode::new("agent", "/project", "compose.yaml")
        .unwrap()
        .with_connection("project:self")
        .unwrap()
        .with_exposure(
            "browser:application",
            DockerExposureTarget::http("web", 8080).unwrap(),
        )
        .unwrap()
}

fn runtime_request(persistence: PersistenceScope) -> ProjectRuntimeMaterialRequest {
    ProjectRuntimeMaterialRequest {
        demand_ref: DemandRef::new("demand:docker-runtime").unwrap(),
        mode: "agent".into(),
        connectivity: vec![LogicalConnectionRequirement::new("project:self").unwrap()],
        persistence: Some(persistence),
        retention: RetentionExpectation::Release,
    }
}

#[test]
fn compose_runtime_prepares_observes_exposes_restarts_and_releases_candidate_state() {
    let runner = ScriptedRunner::new([
        output("29.6.2\n"),
        output("5.1.4\n"),
        output("29.6.2\n"),
        output("5.1.4\n"),
        output(""),
        output("0.0.0.0:49153\n"),
        output("one\ntwo\n"),
        output("one\ntwo\n"),
        output(""),
        output(""),
    ]);
    let mut provider = DockerProjectRuntimeProvider::with_runner(
        ProviderRef::new("provider:docker-runtime").unwrap(),
        [runtime_mode()],
        runner.clone(),
    )
    .unwrap();

    validate_provider_port(&provider).unwrap();
    let request = runtime_request(PersistenceScope::Candidate);
    let allocation = provider.prepare_runtime(&request).unwrap();
    validate_allocation(&provider, &allocation).unwrap();
    assert_eq!(request.connectivity[0].as_str(), "project:self");

    let surface = provider
        .expose_material(
            &allocation,
            &ProviderExposureRequest {
                demand_ref: request.demand_ref.clone(),
                requirement: ExposureRequirement::new("browser:application").unwrap(),
                necessity: RequirementNecessity::Required,
            },
        )
        .unwrap();
    assert_eq!(
        surface.material.get("endpoint").map(String::as_str),
        Some("http://127.0.0.1:49153/")
    );

    let observation = provider.observe_runtime(&allocation).unwrap();
    assert_eq!(
        observation
            .detail
            .get("services_running")
            .map(String::as_str),
        Some("2")
    );
    provider.restart_runtime(&allocation).unwrap();
    provider
        .release_runtime(&allocation, &RetentionExpectation::Release)
        .unwrap();

    let commands = runner.commands();
    let down = commands
        .iter()
        .find(|command| command.args.iter().any(|arg| arg == "down"))
        .unwrap();
    assert!(down.args.iter().any(|arg| arg == "--volumes"));
    assert!(commands.iter().any(|command| {
        command
            .args
            .windows(3)
            .any(|window| window == ["port".to_string(), "web".to_string(), "8080".to_string()])
    }));
}

#[test]
fn project_scoped_compose_state_survives_runtime_release() {
    let runner = ScriptedRunner::new([
        output("29.6.2\n"),
        output("5.1.4\n"),
        output(""),
        output(""),
    ]);
    let mut provider = DockerProjectRuntimeProvider::with_runner(
        ProviderRef::new("provider:docker-project-state").unwrap(),
        [runtime_mode()],
        runner.clone(),
    )
    .unwrap();
    let allocation = provider
        .prepare_runtime(&runtime_request(PersistenceScope::Project))
        .unwrap();
    provider
        .release_runtime(&allocation, &RetentionExpectation::Release)
        .unwrap();
    let down = runner
        .commands()
        .into_iter()
        .find(|command| command.args.iter().any(|arg| arg == "down"))
        .unwrap();
    assert!(!down.args.iter().any(|arg| arg == "--volumes"));
}

#[test]
fn compose_offer_disappears_when_compose_plugin_is_unavailable() {
    let runner = ScriptedRunner::new([
        output("29.6.2\n"),
        Err(WorkcellError::Unavailable("compose missing".into())),
    ]);
    let provider = DockerProjectRuntimeProvider::with_runner(
        ProviderRef::new("provider:compose-missing").unwrap(),
        [runtime_mode()],
        runner,
    )
    .unwrap();
    assert!(provider.offers().unwrap().is_empty());
}

#[test]
fn provider_replacement_changes_material_details_not_the_execution_request() {
    let request = execution_request();
    let runner_a = ScriptedRunner::new([
        output("29.6.2\n"),
        output("container-a\n"),
        output("container-a\n"),
    ]);
    let runner_b = ScriptedRunner::new([
        output("29.6.2\n"),
        output("container-b\n"),
        output("container-b\n"),
    ]);
    let config_a = DockerExecutionConfig::new("alpine:3.22")
        .unwrap()
        .with_logical_network("state:graph", "physical-a")
        .unwrap();
    let config_b = DockerExecutionConfig::new("alpine:3.22")
        .unwrap()
        .with_logical_network("state:graph", "physical-b")
        .unwrap();
    let mut provider_a = DockerExecutionProvider::with_runner(
        ProviderRef::new("provider:docker-a").unwrap(),
        config_a,
        runner_a,
    );
    let mut provider_b = DockerExecutionProvider::with_runner(
        ProviderRef::new("provider:docker-b").unwrap(),
        config_b,
        runner_b,
    );

    let a = provider_a.prepare_execution(&request).unwrap();
    let b = provider_b.prepare_execution(&request).unwrap();
    assert_eq!(request.connectivity[0].as_str(), "state:graph");
    assert_ne!(a.material_ref, b.material_ref);
    assert_ne!(a.provider_ref, b.provider_ref);
}

#[test]
fn core_execution_demand_does_not_acquire_docker_vocabulary() {
    let source = include_str!("../../workcell-core/src/demand.rs").to_ascii_lowercase();
    assert!(!source.contains("docker"));
    assert!(!source.contains("compose_project"));
    assert!(!source.contains("container_name"));
    let _: Option<PathBuf> = None;
}
