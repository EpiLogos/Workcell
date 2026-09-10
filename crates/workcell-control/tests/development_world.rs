use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    path::PathBuf,
    time::{SystemTime, UNIX_EPOCH},
};

use epilogos_workcell_control::{ControlClient, ControlService, DirectTransport};
use epilogos_workcell_core::{
    AffordanceRequirement, DemandRef, DesiredMaterialState, ExecutionDemand, ExternalRef,
    OutputRequirement, PersistenceScope, RetentionExpectation, WorkcellRef, WorkspaceAccess,
    WorkspaceRequirement,
};
use epilogos_workcell_runtime::{CollapsedLocalConfig, CollapsedLocalWorkcell};
use epilogos_workcell_wire::decode_world;

fn temp_path(label: &str) -> PathBuf {
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    std::env::temp_dir().join(format!(
        "epilogos-development-world-{label}-{}-{nonce}",
        std::process::id()
    ))
}

fn development_demand() -> ExecutionDemand {
    let mut demand = ExecutionDemand::new(DemandRef::new("demand:development-world").unwrap());
    for (role, reference) in [
        ("project", "project:external/specimen"),
        ("run", "run:external/42"),
        ("agent", "agent:external/builder"),
        ("suite", "suite:external/mainline"),
    ] {
        demand = demand.with_subject(role, ExternalRef::new(reference).unwrap());
    }
    demand.workspace = Some(WorkspaceRequirement {
        source: None,
        revision: None,
        access: WorkspaceAccess::Writable,
    });
    demand
        .affordances
        .required
        .push(AffordanceRequirement::new("shell").unwrap());
    demand
        .outputs
        .required
        .push(OutputRequirement::new("logs:run").unwrap());
    demand.persistence = Some(PersistenceScope::Ephemeral);
    demand.retention = RetentionExpectation::Release;
    demand.validate().unwrap();
    demand
}

#[test]
fn development_world_is_inspectable_through_prepare_observe_collect_release_reconcile() {
    let root = temp_path("lifecycle");
    let workcell = CollapsedLocalWorkcell::new(CollapsedLocalConfig::new(
        WorkcellRef::new("workcell:local").unwrap(),
        &root,
    ))
    .unwrap();
    let demand = development_demand();
    let expected_subjects = demand.subjects.clone();

    let mut service = ControlService::new(workcell);
    let mut client = ControlClient::new(DirectTransport::new(&mut service));

    let plan = client.plan(&demand).unwrap();
    assert_eq!(plan["status"], "satisfiable");

    let prepared = client.prepare(&demand).unwrap();
    let world = decode_world(&serde_json::to_string(&prepared).unwrap()).unwrap();
    let world_ref = world.world_ref.clone();
    assert_eq!(world.workcell_ref.as_str(), "workcell:local");
    assert_eq!(world.subjects, expected_subjects);

    let inspected = client.inspect(&world_ref).unwrap();
    let inspected_world = decode_world(&serde_json::to_string(&inspected).unwrap()).unwrap();
    assert_eq!(inspected_world.world_ref, world_ref);
    assert_eq!(inspected_world.subjects, expected_subjects);
    assert!(!inspected_world.binding_graph.bindings.is_empty());

    let binding_refs = inspected_world
        .binding_graph
        .bindings
        .iter()
        .map(|binding| binding.binding_ref.to_string())
        .collect::<BTreeSet<_>>();
    assert_eq!(
        binding_refs.len(),
        inspected_world.binding_graph.bindings.len()
    );
    for binding in &inspected_world.binding_graph.bindings {
        assert!(!binding.provider_ref.as_str().is_empty());
        assert!(!binding.material_ref.is_empty());
        assert_eq!(format!("{:?}", binding.presence), "Present");
    }

    let artifact_binding = inspected_world
        .binding_graph
        .bindings
        .iter()
        .find(|binding| binding.logical_ref == "output:logs:run")
        .expect("required output channel is materially bound");
    let artifact_path = PathBuf::from(
        artifact_binding
            .properties
            .get("path")
            .expect("artifact binding exposes material path"),
    );
    fs::write(
        artifact_path.join("development-proof.txt"),
        "material evidence\n",
    )
    .unwrap();

    let observed = client.observe(&world_ref).unwrap();
    for observation in observed["observations"].as_array().unwrap() {
        let logical_ref = observation["logical_ref"].as_str().unwrap();
        let binding = inspected_world
            .binding_graph
            .bindings
            .iter()
            .find(|binding| binding.logical_ref == logical_ref)
            .unwrap();
        assert_eq!(
            observation["detail"]["provider_ref"].as_str().unwrap(),
            binding.provider_ref.as_str()
        );
        assert_eq!(
            observation["detail"]["material_ref"].as_str().unwrap(),
            binding.material_ref
        );
    }

    let collected = client.collect(&world_ref).unwrap();
    let output = collected["outputs"]
        .as_array()
        .unwrap()
        .iter()
        .find(|output| output["logical_ref"] == "logs:run/development-proof.txt")
        .expect("material evidence is collected through the requested channel");
    assert!(output["material_locator"]
        .as_str()
        .unwrap()
        .ends_with("development-proof.txt"));
    assert_eq!(
        output["provenance"]["implementation"],
        "directory-artifact-storage"
    );

    let before_release = client
        .reconcile(&[DesiredMaterialState {
            logical_ref: "affordance:shell".into(),
            desired: "present".into(),
        }])
        .unwrap();
    assert_eq!(
        before_release["deltas"][0]["action"],
        serde_json::Value::Null
    );

    let release = client.release(&world_ref).unwrap();
    assert_eq!(release["disposition"], "released");

    let released = client.inspect(&world_ref).unwrap();
    let released_world = decode_world(&serde_json::to_string(&released).unwrap()).unwrap();
    assert_eq!(released_world.world_ref, world_ref);
    assert_eq!(released_world.subjects, expected_subjects);
    assert!(released_world
        .binding_graph
        .bindings
        .iter()
        .all(|binding| format!("{:?}", binding.presence) == "Released"));

    let original_material = inspected_world
        .binding_graph
        .bindings
        .iter()
        .map(|binding| {
            (
                binding.logical_ref.clone(),
                (
                    binding.binding_ref.to_string(),
                    binding.provider_ref.to_string(),
                    binding.material_ref.clone(),
                ),
            )
        })
        .collect::<BTreeMap<_, _>>();
    for binding in &released_world.binding_graph.bindings {
        assert_eq!(
            original_material.get(&binding.logical_ref),
            Some(&(
                binding.binding_ref.to_string(),
                binding.provider_ref.to_string(),
                binding.material_ref.clone(),
            ))
        );
    }

    let after_release = client
        .reconcile(&[DesiredMaterialState {
            logical_ref: "affordance:shell".into(),
            desired: "present".into(),
        }])
        .unwrap();
    assert_eq!(after_release["deltas"][0]["action"], "rematerialise");

    let _ = fs::remove_dir_all(root);
}
