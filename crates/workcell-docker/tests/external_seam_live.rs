//! The SDK admission-seam specimen with the real Docker capability.
//!
//! The commission's external-adapter specimen: container execution was an
//! actual missing material capability on the collapsed-local composition.
//! This proof registers the Docker adapter through the *external* seam
//! (`CollapsedLocalConfig::with_external_execution_provider` — the SDK's
//! admission surface), then exercises ordinary public operations against the
//! real Engine: registration, discovery, selection, prepare (with a real
//! caller-authorised ceiling verified on the container), observation and
//! release. No semantic-client type changes anywhere.
//!
//! Opt-in, like the adapter's own live smoke:
//!
//! ```sh
//! WORKCELL_DOCKER_LIVE=1 cargo test -p epilogos-workcell-docker --test external_seam_live
//! ```

use std::process;
use std::sync::Arc;

use epilogos_workcell_core::{
    AffordanceRequirement, DemandRef, ExecutionDemand, ExecutionProvider, ProviderAllocation,
    ProviderPortKind, ProviderRef, RetentionExpectation, WorkcellControlPlane, WorkcellRef,
};
use epilogos_workcell_docker::DockerExecutionProvider;
use epilogos_workcell_runtime::{CollapsedLocalConfig, CollapsedLocalWorkcell};

fn live_enabled() -> bool {
    std::env::var("WORKCELL_DOCKER_LIVE").as_deref() == Ok("1")
}

fn image() -> String {
    std::env::var("WORKCELL_DOCKER_IMAGE").unwrap_or_else(|_| "alpine:3.22".into())
}

fn allocation_of(binding: &epilogos_workcell_core::Binding) -> ProviderAllocation {
    ProviderAllocation {
        provider_ref: binding.provider_ref.clone(),
        port: binding.port,
        material_ref: binding.material_ref.clone(),
        health: binding.health.clone(),
        properties: binding.properties.clone(),
        provenance: binding.provenance.clone(),
    }
}

#[test]
fn docker_container_execution_through_the_external_seam() {
    if !live_enabled() {
        eprintln!("skipping: set WORKCELL_DOCKER_LIVE=1 to run against the real Engine");
        return;
    }
    let nonce = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let state =
        std::env::temp_dir().join(format!("wc-docker-external-seam-{}-{nonce}", process::id()));
    std::fs::create_dir_all(&state).unwrap();

    // Registration happens at composition through the SDK admission seam —
    // the host brings a provider from its own technology. A failing
    // construction would refuse the whole composition (proven separately by
    // the deterministic specimen tests).
    let factory: epilogos_workcell_runtime::ExternalExecutionProviderFactory = {
        let image = image();
        Arc::new(move || {
            let config = epilogos_workcell_docker::DockerExecutionConfig::new(image.clone())?;
            Ok(Box::new(DockerExecutionProvider::new(
                ProviderRef::new("provider:external/docker-live")?,
                config,
            )) as Box<dyn ExecutionProvider>)
        })
    };
    let mut workcell = CollapsedLocalWorkcell::new(
        CollapsedLocalConfig::new(WorkcellRef::new("workcell:docker-live").unwrap(), state)
            .with_external_execution_provider(factory),
    )
    .unwrap();

    // Discovered.
    let discovery = workcell.discover().unwrap();
    let offer = discovery
        .offers
        .iter()
        .find(|offer| offer.provider_ref.as_str() == "provider:external/docker-live")
        .expect("the registered external Docker provider is discoverable");
    assert_eq!(offer.port, ProviderPortKind::Execution.as_str());
    assert_eq!(offer.affordances, vec!["shell".to_owned()]);

    // Selected: the demand plans a binding naming its identity.
    let mut demand = ExecutionDemand::new(DemandRef::new("demand:docker-live").unwrap());
    demand
        .affordances
        .required
        .push(AffordanceRequirement::new("shell").unwrap());
    demand
        .resources
        .push(epilogos_workcell_core::ResourceRequirement {
            key: "memory".into(),
            minimum: Some(32),
            maximum: Some(256),
            unit: Some("MiB".into()),
        });
    demand.retention = RetentionExpectation::Release;
    let plan = workcell.plan(&demand).unwrap();
    assert!(
        plan.planned_bindings
            .iter()
            .any(|binding| binding.provider_ref.as_str() == "provider:external/docker-live"),
        "the plan must select the external Docker provider: {plan:?}"
    );

    // Used: prepare materialises a real container. The caller-authorised
    // ceiling — never the floor — is what the container is capped with.
    let world = workcell.prepare(&demand).expect("real Docker prepare");
    let execution_binding = world
        .binding_graph
        .bindings
        .iter()
        .find(|binding| binding.port == ProviderPortKind::Execution)
        .expect("an execution binding exists");
    let allocation = allocation_of(execution_binding);
    let container_id = allocation
        .provenance
        .get("container_id")
        .cloned()
        .expect("container id in provenance");
    let inspected = process::Command::new("docker")
        .args([
            "container",
            "inspect",
            &container_id,
            "--format",
            "{{.HostConfig.Memory}}",
        ])
        .output()
        .expect("docker inspect runs");
    let memory_limit: u64 = String::from_utf8_lossy(&inspected.stdout)
        .trim()
        .parse()
        .unwrap_or(0);
    assert_eq!(
        memory_limit,
        256 * 1024 * 1024,
        "the ceiling, not the floor, is enforced on the real container"
    );

    // Observed, then released: the container is removed.
    let observation = workcell.observe(&world.world_ref).unwrap();
    assert_eq!(observation.observations.len(), 1);
    workcell.release(&world.world_ref).unwrap();
    let gone = process::Command::new("docker")
        .args(["container", "inspect", &container_id])
        .output()
        .expect("docker inspect runs");
    assert!(
        !gone.status.success(),
        "the container is removed on release"
    );
}
