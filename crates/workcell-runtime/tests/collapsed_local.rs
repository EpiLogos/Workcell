use std::{
    fs,
    path::PathBuf,
    time::{SystemTime, UNIX_EPOCH},
};

use epilogos_workcell_core::{
    AffordanceRequirement, DemandRef, ExecutionDemand, PlanStatus, RetentionExpectation,
    WorkcellControlPlane, WorkcellError, WorkcellRef, WorkspaceAccess, WorkspaceRequirement,
};
use epilogos_workcell_runtime::{CollapsedLocalConfig, CollapsedLocalWorkcell};

fn temp_path(label: &str) -> PathBuf {
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    std::env::temp_dir().join(format!(
        "epilogos-workcell-{label}-{}-{nonce}",
        std::process::id()
    ))
}

fn shell_workspace_demand() -> ExecutionDemand {
    let mut demand = ExecutionDemand::new(DemandRef::new("demand:collapsed-local").unwrap());
    demand
        .affordances
        .required
        .push(AffordanceRequirement::new("shell").unwrap());
    demand
        .affordances
        .preferred
        .push(AffordanceRequirement::new("microvm-snapshot").unwrap());
    demand.workspace = Some(WorkspaceRequirement {
        source: None,
        revision: None,
        access: WorkspaceAccess::Writable,
    });
    demand.retention = RetentionExpectation::Release;
    demand
}

#[test]
fn collapsed_local_materialises_shell_and_writable_workspace_without_daemon() {
    let source = temp_path("collapsed-source");
    let state = temp_path("collapsed-state");
    fs::create_dir_all(&source).unwrap();
    fs::write(source.join("source.txt"), "portable material source\n").unwrap();

    let mut workcell = CollapsedLocalWorkcell::new(
        CollapsedLocalConfig::new(
            WorkcellRef::new("workcell:collapsed-local-test").unwrap(),
            &state,
        )
        .with_workspace_source(&source),
    )
    .unwrap();
    let demand = shell_workspace_demand();

    let discovery = workcell.discover().unwrap();
    assert!(discovery.offers.iter().any(|offer| offer
        .affordances
        .iter()
        .any(|value| value == "shell")));
    assert!(discovery.offers.iter().any(|offer| offer
        .affordances
        .iter()
        .any(|value| value == "workspace:writable")));

    let plan = workcell.plan(&demand).unwrap();
    assert_eq!(plan.status, PlanStatus::Degraded);
    assert!(plan
        .degradations
        .iter()
        .any(|item| item.requirement == "affordance:microvm-snapshot"));

    let world = workcell.prepare(&demand).unwrap();
    assert_eq!(world.demand_ref, demand.demand_ref);
    assert!(world
        .binding_graph
        .bindings
        .iter()
        .any(|binding| binding.logical_ref == "affordance:shell"));
    let workspace = world
        .binding_graph
        .bindings
        .iter()
        .find(|binding| binding.logical_ref == "workspace:workspace:writable")
        .unwrap();
    let material_path = PathBuf::from(workspace.properties.get("path").unwrap());
    assert_eq!(
        fs::read_to_string(material_path.join("source.txt")).unwrap(),
        "portable material source\n"
    );

    let observed = workcell.observe(&world.world_ref).unwrap();
    assert!(observed
        .observations
        .iter()
        .all(|observation| observation.state != epilogos_workcell_core::HealthState::Unavailable));
    let collected = workcell.collect(&world.world_ref).unwrap();
    assert!(collected.outputs.is_empty());

    let released = workcell.release(&world.world_ref).unwrap();
    assert!(released.changed);
    assert!(!material_path.exists());

    let _ = fs::remove_dir_all(source);
    let _ = fs::remove_dir_all(state);
}

#[test]
fn collapsed_local_required_unavailable_affordance_fails_explicitly() {
    let state = temp_path("collapsed-required-state");
    let mut workcell = CollapsedLocalWorkcell::new(CollapsedLocalConfig::new(
        WorkcellRef::new("workcell:collapsed-local-required").unwrap(),
        &state,
    ))
    .unwrap();
    let mut demand = ExecutionDemand::new(DemandRef::new("demand:required-isolation").unwrap());
    demand
        .affordances
        .required
        .push(AffordanceRequirement::new("microvm-snapshot").unwrap());

    let plan = workcell.plan(&demand).unwrap();
    assert_eq!(plan.status, PlanStatus::Unsatisfiable);
    assert!(matches!(
        workcell.prepare(&demand),
        Err(WorkcellError::UnsatisfiedDemand(_))
    ));

    let _ = fs::remove_dir_all(state);
}
