use std::{
    collections::VecDeque,
    sync::{Arc, Mutex},
};

use epilogos_workcell_core::{
    DemandRef, ExecutionMaterialRequest, ExecutionProvider, HealthState, PersistenceScope,
    ProjectRuntimeMaterialRequest, ProjectRuntimeProvider, ProviderRef, RetentionExpectation,
};
use epilogos_workcell_docker::{
    DockerCommand, DockerCommandOutput, DockerCommandRunner, DockerExecutionConfig,
    DockerExecutionProvider, DockerProjectRuntimeProvider, DockerRuntimeMode,
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

#[test]
fn execution_allocation_survives_provider_process_restart() {
    let prepare_runner = ScriptedRunner::new([
        output("29.6.2\n"),
        output("container-recover\n"),
        output("container-recover\n"),
    ]);
    let config = DockerExecutionConfig::new("alpine:3.22").unwrap();
    let provider_ref = ProviderRef::new("provider:docker-recovery").unwrap();
    let mut first =
        DockerExecutionProvider::with_runner(provider_ref.clone(), config.clone(), prepare_runner);
    let allocation = first
        .prepare_execution(&ExecutionMaterialRequest {
            demand_ref: DemandRef::new("demand:docker-recovery").unwrap(),
            affordances: vec!["shell".into()],
            resources: vec![],
            connectivity: vec![],
            isolation_trust: None,
            retention: RetentionExpectation::Release,
        })
        .unwrap();
    drop(first);

    let recovery_runner = ScriptedRunner::new([output("running\n"), output("container-recover\n")]);
    let mut restarted =
        DockerExecutionProvider::with_runner(provider_ref, config, recovery_runner.clone());
    let observed = restarted.observe_execution(&allocation).unwrap();
    assert_eq!(observed.health, HealthState::Healthy);
    restarted
        .release_execution(&allocation, &RetentionExpectation::Release)
        .unwrap();

    let commands = recovery_runner.commands();
    assert!(commands.iter().any(|command| {
        command.args.iter().any(|arg| arg == "inspect")
            && command.args.iter().any(|arg| arg == "container-recover")
    }));
    assert!(commands.iter().any(|command| {
        command.args.iter().any(|arg| arg == "rm")
            && command.args.iter().any(|arg| arg == "container-recover")
    }));
}

#[test]
fn compose_allocation_survives_provider_process_restart() {
    let prepare_runner = ScriptedRunner::new([output("29.6.2\n"), output("5.1.4\n"), output("")]);
    let mode = DockerRuntimeMode::new("agent", "/project", "compose.yaml").unwrap();
    let provider_ref = ProviderRef::new("provider:docker-runtime-recovery").unwrap();
    let mut first = DockerProjectRuntimeProvider::with_runner(
        provider_ref.clone(),
        [mode.clone()],
        prepare_runner,
    )
    .unwrap();
    let allocation = first
        .prepare_runtime(&ProjectRuntimeMaterialRequest {
            demand_ref: DemandRef::new("demand:docker-runtime-recovery").unwrap(),
            mode: "agent".into(),
            connectivity: vec![],
            persistence: Some(PersistenceScope::Project),
            retention: RetentionExpectation::Release,
        })
        .unwrap();
    drop(first);

    let recovery_runner = ScriptedRunner::new([output("one\n"), output("one\n"), output("")]);
    let mut restarted =
        DockerProjectRuntimeProvider::with_runner(provider_ref, [mode], recovery_runner.clone())
            .unwrap();
    let observed = restarted.observe_runtime(&allocation).unwrap();
    assert_eq!(observed.health, HealthState::Healthy);
    restarted
        .release_runtime(&allocation, &RetentionExpectation::Release)
        .unwrap();

    let down = recovery_runner
        .commands()
        .into_iter()
        .find(|command| command.args.iter().any(|arg| arg == "down"))
        .unwrap();
    assert!(!down.args.iter().any(|arg| arg == "--volumes"));
    assert_eq!(
        allocation
            .properties
            .get("persistence_scope")
            .map(String::as_str),
        Some("project")
    );
    assert!(allocation.provenance.contains_key("compose_project"));
    assert!(allocation.properties.contains_key("compose_project"));
    assert!(!allocation.material_ref.is_empty());
}
