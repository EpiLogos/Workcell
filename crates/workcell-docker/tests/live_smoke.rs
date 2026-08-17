use std::{
    collections::BTreeMap,
    fs, process,
    time::{SystemTime, UNIX_EPOCH},
};

use epilogos_workcell_core::{
    DemandRef, ExecutionMaterialRequest, ExecutionProvider, PersistenceScope,
    ProjectRuntimeMaterialRequest, ProjectRuntimeProvider, ProviderOperation, ProviderRef,
    RetentionExpectation,
};
use epilogos_workcell_docker::{
    DockerExecutionConfig, DockerExecutionProvider, DockerProjectRuntimeProvider, DockerRuntimeMode,
};

fn live_enabled() -> bool {
    std::env::var("WORKCELL_DOCKER_LIVE").as_deref() == Ok("1")
}

fn image() -> String {
    std::env::var("WORKCELL_DOCKER_IMAGE").unwrap_or_else(|_| "alpine:3.22".into())
}

#[test]
fn live_docker_execution_prepare_observe_shell_restart_release() {
    if !live_enabled() {
        return;
    }

    let mut provider = DockerExecutionProvider::new(
        ProviderRef::new("provider:docker-live-execution").unwrap(),
        DockerExecutionConfig::new(image()).unwrap(),
    );
    assert!(!epilogos_workcell_core::ProviderPort::offers(&provider)
        .unwrap()
        .is_empty());

    let request = ExecutionMaterialRequest {
        demand_ref: DemandRef::new("demand:docker-live-execution").unwrap(),
        affordances: vec!["shell".into()],
        resources: vec![],
        connectivity: vec![],
        isolation_trust: None,
        retention: RetentionExpectation::Release,
    };
    let allocation = provider.prepare_execution(&request).unwrap();
    let result = provider
        .execute_operation(
            &allocation,
            &ProviderOperation {
                key: "shell".into(),
                parameters: BTreeMap::from([(
                    "command".into(),
                    "printf workcell-docker-live".into(),
                )]),
            },
        )
        .unwrap();
    assert_eq!(
        result.output.get("stdout").map(String::as_str),
        Some("workcell-docker-live")
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
}

#[test]
fn live_docker_compose_runtime_prepare_observe_restart_release() {
    if !live_enabled() {
        return;
    }

    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let root = std::env::temp_dir().join(format!(
        "epilogos-workcell-docker-live-{}-{nonce}",
        process::id()
    ));
    fs::create_dir_all(&root).unwrap();
    let compose_file = root.join("compose.yaml");
    fs::write(
        &compose_file,
        format!(
            "services:\n  worker:\n    image: {}\n    command: [\"sh\", \"-lc\", \"while :; do sleep 3600; done\"]\n",
            image()
        ),
    )
    .unwrap();

    let mode = DockerRuntimeMode::new("agent", &root, &compose_file).unwrap();
    let mut provider = DockerProjectRuntimeProvider::new(
        ProviderRef::new("provider:docker-live-runtime").unwrap(),
        [mode],
    )
    .unwrap();
    let request = ProjectRuntimeMaterialRequest {
        demand_ref: DemandRef::new(format!("demand:docker-live-runtime-{nonce}")).unwrap(),
        mode: "agent".into(),
        connectivity: vec![],
        persistence: Some(PersistenceScope::TaskOrRun),
        retention: RetentionExpectation::Release,
    };
    let allocation = provider.prepare_runtime(&request).unwrap();
    let observation = provider.observe_runtime(&allocation).unwrap();
    assert_eq!(
        observation
            .detail
            .get("services_running")
            .map(String::as_str),
        Some("1")
    );
    provider.restart_runtime(&allocation).unwrap();
    provider
        .release_runtime(&allocation, &RetentionExpectation::Release)
        .unwrap();
    fs::remove_dir_all(root).unwrap();
}
