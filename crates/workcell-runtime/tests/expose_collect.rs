use std::{
    fs,
    path::PathBuf,
    time::{SystemTime, UNIX_EPOCH},
};

use epilogos_workcell_artifact::DirectoryArtifactStorageProvider;
use epilogos_workcell_core::{
    compose_world, ArtifactChannelRequest, ArtifactStorageProvider, DemandRef, Discovery,
    ExecutionDemand, ExposureRequirement, ExternalRef, HealthState, MaterialisationPlan,
    OutputRequirement, PlanStatus, PlannedAllocation, PreparedWorldControlPlane,
    ProjectRuntimeMaterialRequest, ProjectRuntimeProvider, ProjectRuntimeRequirement, ProviderPort,
    ProviderRef, ReleaseDisposition, RequirementNecessity, RetentionExpectation,
    WorkcellControlPlane, WorkcellRef, WorldRef,
};
use epilogos_workcell_runtime::{ReferenceProjectRuntimeProvider, RuntimeMode};

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

fn runtime(provider_ref: &str, expose_browser: bool) -> ReferenceProjectRuntimeProvider {
    let mut mode = RuntimeMode::new("agent")
        .unwrap()
        .with_endpoint("http://candidate.internal:3000");
    if expose_browser {
        mode = mode.with_exposure("browser");
    }
    ReferenceProjectRuntimeProvider::new(ProviderRef::new(provider_ref).unwrap(), [mode]).unwrap()
}

fn artifact(provider_ref: &str, root: PathBuf) -> DirectoryArtifactStorageProvider {
    DirectoryArtifactStorageProvider::new(
        ProviderRef::new(provider_ref).unwrap(),
        root,
        ["logs:run".into()],
    )
    .unwrap()
}

fn demand(retention: RetentionExpectation) -> ExecutionDemand {
    let mut demand = ExecutionDemand::new(DemandRef::new("demand:expose-collect").unwrap())
        .with_subject(
            "candidate",
            ExternalRef::new("client:candidate:stable").unwrap(),
        );
    demand.project_runtime = Some(ProjectRuntimeRequirement::new("agent").unwrap());
    demand
        .exposure
        .required
        .push(ExposureRequirement::new("browser").unwrap());
    demand
        .outputs
        .required
        .push(OutputRequirement::new("logs:run").unwrap());
    demand.retention = retention;
    demand
}

fn runtime_request(demand: &ExecutionDemand) -> ProjectRuntimeMaterialRequest {
    ProjectRuntimeMaterialRequest {
        demand_ref: demand.demand_ref.clone(),
        mode: demand.project_runtime.as_ref().unwrap().as_str().into(),
        connectivity: vec![],
        persistence: demand.persistence.clone(),
        retention: demand.retention.clone(),
    }
}

fn artifact_request(demand: &ExecutionDemand) -> ArtifactChannelRequest {
    ArtifactChannelRequest {
        demand_ref: demand.demand_ref.clone(),
        logical_channel: "logs:run".into(),
        persistence: demand.persistence.clone(),
        retention: demand.retention.clone(),
    }
}

fn plan_for(
    demand: &ExecutionDemand,
    runtime: &ReferenceProjectRuntimeProvider,
    artifact: &DirectoryArtifactStorageProvider,
) -> MaterialisationPlan {
    let mut offers = runtime.offers().unwrap();
    offers.extend(artifact.offers().unwrap());
    epilogos_workcell_core::plan(
        demand,
        &Discovery {
            workcell_ref: WorkcellRef::new("workcell:surface-test").unwrap(),
            health: HealthState::Healthy,
            offers,
        },
    )
    .unwrap()
}

fn prepared(
    retention: RetentionExpectation,
) -> (PreparedWorldControlPlane, WorldRef, PathBuf, ExternalRef) {
    let demand = demand(retention);
    let candidate = demand.subjects.get("candidate").unwrap().clone();
    let mut runtime = runtime("provider:runtime-surface", true);
    let artifact_root = temp_path("artifact-surface");
    let mut artifact = artifact("provider:artifact-surface", artifact_root.clone());
    let plan = plan_for(&demand, &runtime, &artifact);
    assert_ne!(plan.status, PlanStatus::Unsatisfiable);

    assert!(plan
        .planned_bindings
        .iter()
        .any(|binding| binding.logical_ref == "project-runtime:runtime-mode:agent"));
    assert!(plan
        .planned_bindings
        .iter()
        .any(|binding| binding.logical_ref == "output:logs:run"));
    assert!(plan
        .planned_exposures
        .iter()
        .any(|exposure| exposure.logical_ref == "exposure:browser"));
    assert!(!plan
        .planned_bindings
        .iter()
        .any(|binding| binding.logical_ref == "exposure:browser"));
    if demand.retention == RetentionExpectation::Preserve {
        assert!(plan
            .planned_constraints
            .iter()
            .any(|constraint| constraint.logical_ref == "retention:retention:preserve"));
    }

    let runtime_allocation = runtime.prepare_runtime(&runtime_request(&demand)).unwrap();
    let artifact_allocation = artifact
        .prepare_artifact_channel(&artifact_request(&demand))
        .unwrap();
    let artifact_path = PathBuf::from(artifact_allocation.properties.get("path").unwrap());
    fs::write(artifact_path.join("agent.log"), "candidate output\n").unwrap();

    let runtime_plan = plan
        .planned_bindings
        .iter()
        .find(|binding| binding.logical_ref == "project-runtime:runtime-mode:agent")
        .unwrap();
    let artifact_plan = plan
        .planned_bindings
        .iter()
        .find(|binding| binding.logical_ref == "output:logs:run")
        .unwrap();
    let world = compose_world(
        WorkcellRef::new("workcell:surface-test").unwrap(),
        &demand,
        &plan,
        vec![
            PlannedAllocation {
                logical_ref: runtime_plan.logical_ref.clone(),
                offer_ref: runtime_plan.offer_ref.clone(),
                allocation: runtime_allocation,
            },
            PlannedAllocation {
                logical_ref: artifact_plan.logical_ref.clone(),
                offer_ref: artifact_plan.offer_ref.clone(),
                allocation: artifact_allocation,
            },
        ],
        vec![],
    )
    .unwrap();
    assert_eq!(world.subjects.get("candidate"), Some(&candidate));
    let world_ref = world.world_ref.clone();

    let mut control =
        PreparedWorldControlPlane::new(WorkcellRef::new("workcell:surface-test").unwrap());
    control.register_world(world).unwrap();
    control.register_runtime_provider(runtime).unwrap();
    control.register_artifact_provider(artifact).unwrap();
    (control, world_ref, artifact_root, candidate)
}

fn public_expose(control: &dyn WorkcellControlPlane, world_ref: &WorldRef) {
    let bundle = control.expose(world_ref).unwrap();
    assert_eq!(bundle.surfaces.len(), 1);
    let surface = &bundle.surfaces[0];
    assert_eq!(surface.logical_ref, "exposure:browser");
    assert_eq!(surface.interaction, "browser");
    assert_eq!(
        surface.material.get("endpoint").unwrap(),
        "http://candidate.internal:3000"
    );
    assert_eq!(
        surface.provenance.get("provider_ref").unwrap(),
        "provider:runtime-surface"
    );
}

fn public_collect(control: &dyn WorkcellControlPlane, world_ref: &WorldRef) {
    let bundle = control.collect(world_ref).unwrap();
    assert_eq!(bundle.outputs.len(), 1);
    assert_eq!(bundle.outputs[0].logical_ref, "logs:run/agent.log");
    assert!(bundle.outputs[0].material_locator.ends_with("agent.log"));
    assert_eq!(
        bundle.outputs[0].provenance.get("relative_path").unwrap(),
        "agent.log"
    );
}

#[test]
fn public_control_plane_exposes_and_collects_material_without_semantic_identity_collapse() {
    let (control, world_ref, root, candidate) = prepared(RetentionExpectation::Release);
    public_expose(&control, &world_ref);
    public_collect(&control, &world_ref);

    let world = control.world(&world_ref).unwrap();
    assert_eq!(world.subjects.get("candidate"), Some(&candidate));
    assert_ne!(world.world_ref.as_str(), candidate.as_str());
    for binding in &world.binding_graph.bindings {
        assert_ne!(binding.binding_ref.as_str(), candidate.as_str());
        assert_ne!(binding.material_ref, candidate.as_str());
    }

    let _ = fs::remove_dir_all(root);
}

#[test]
fn required_preferred_and_optional_surface_absence_remain_distinct() {
    let runtime = runtime("provider:runtime-no-browser", false);
    let root = temp_path("artifact-plan-tiers");
    let artifact = artifact("provider:artifact-plan-tiers", root.clone());

    let mut required = demand(RetentionExpectation::Release);
    required.outputs.required.clear();
    let required_plan = plan_for(&required, &runtime, &artifact);
    assert_eq!(required_plan.status, PlanStatus::Unsatisfiable);

    let mut preferred = required.clone();
    preferred.exposure.required.clear();
    preferred
        .exposure
        .preferred
        .push(ExposureRequirement::new("browser").unwrap());
    let preferred_plan = plan_for(&preferred, &runtime, &artifact);
    assert_eq!(preferred_plan.status, PlanStatus::Degraded);
    assert!(preferred_plan
        .degradations
        .iter()
        .any(|item| item.requirement == "exposure:browser"));

    let mut optional = required;
    optional.exposure.required.clear();
    optional
        .exposure
        .optional
        .push(ExposureRequirement::new("browser").unwrap());
    let optional_plan = plan_for(&optional, &runtime, &artifact);
    assert_eq!(optional_plan.status, PlanStatus::Satisfiable);
    assert!(optional_plan
        .omissions
        .iter()
        .any(|item| item.requirement == "exposure:browser"));

    let _ = fs::remove_dir_all(root);
}

#[test]
fn required_preferred_and_optional_output_absence_remain_distinct() {
    let runtime = runtime("provider:runtime-output-tiers", true);
    let empty_root = temp_path("artifact-output-tiers");
    let empty_artifact = DirectoryArtifactStorageProvider::new(
        ProviderRef::new("provider:artifact-empty").unwrap(),
        empty_root.clone(),
        [],
    )
    .unwrap();

    let mut required = demand(RetentionExpectation::Release);
    required.exposure.required.clear();
    let required_plan = plan_for(&required, &runtime, &empty_artifact);
    assert_eq!(required_plan.status, PlanStatus::Unsatisfiable);

    let mut preferred = required.clone();
    preferred.outputs.required.clear();
    preferred
        .outputs
        .preferred
        .push(OutputRequirement::new("logs:run").unwrap());
    let preferred_plan = plan_for(&preferred, &runtime, &empty_artifact);
    assert_eq!(preferred_plan.status, PlanStatus::Degraded);
    assert!(preferred_plan
        .degradations
        .iter()
        .any(|item| item.requirement == "output:logs:run"));

    let mut optional = required;
    optional.outputs.required.clear();
    optional
        .outputs
        .optional
        .push(OutputRequirement::new("logs:run").unwrap());
    let optional_plan = plan_for(&optional, &runtime, &empty_artifact);
    assert_eq!(optional_plan.status, PlanStatus::Satisfiable);
    assert!(optional_plan
        .omissions
        .iter()
        .any(|item| item.requirement == "output:logs:run"));

    let _ = fs::remove_dir_all(empty_root);
}

#[test]
fn collected_logical_output_survives_provider_and_locator_replacement() {
    let demand = demand(RetentionExpectation::Release);
    let root_a = temp_path("artifact-relocation-a");
    let root_b = temp_path("artifact-relocation-b");
    let mut first = artifact("provider:artifact-a", root_a.clone());
    let mut second = artifact("provider:artifact-b", root_b.clone());
    let request = artifact_request(&demand);

    let first_allocation = first.prepare_artifact_channel(&request).unwrap();
    let second_allocation = second.prepare_artifact_channel(&request).unwrap();
    let first_path = PathBuf::from(first_allocation.properties.get("path").unwrap());
    let second_path = PathBuf::from(second_allocation.properties.get("path").unwrap());
    fs::write(first_path.join("agent.log"), "one\n").unwrap();
    fs::write(second_path.join("agent.log"), "two\n").unwrap();

    let first_output = first.collect_material(&first_allocation).unwrap().remove(0);
    let second_output = second
        .collect_material(&second_allocation)
        .unwrap()
        .remove(0);
    assert_eq!(first_output.logical_output, "logs:run/agent.log");
    assert_eq!(first_output.logical_output, second_output.logical_output);
    assert_ne!(first_output.provider_ref, second_output.provider_ref);
    assert_ne!(first_output.locator, second_output.locator);

    first
        .release_artifact_channel(&first_allocation, &RetentionExpectation::Release)
        .unwrap();
    second
        .release_artifact_channel(&second_allocation, &RetentionExpectation::Release)
        .unwrap();
    let _ = fs::remove_dir_all(root_a);
    let _ = fs::remove_dir_all(root_b);
}

#[test]
fn preserve_keeps_surfaces_and_outputs_live_while_release_keeps_only_provenance() {
    let (mut preserved, preserved_ref, preserved_root, candidate) =
        prepared(RetentionExpectation::Preserve);
    let result = preserved.release(&preserved_ref).unwrap();
    assert_eq!(result.disposition, ReleaseDisposition::Preserved);
    assert!(!result.changed);
    public_expose(&preserved, &preserved_ref);
    public_collect(&preserved, &preserved_ref);
    assert_eq!(
        preserved
            .world(&preserved_ref)
            .unwrap()
            .subjects
            .get("candidate"),
        Some(&candidate)
    );

    let (mut released, released_ref, released_root, released_candidate) =
        prepared(RetentionExpectation::Release);
    let before = released.world(&released_ref).unwrap().clone();
    let result = released.release(&released_ref).unwrap();
    assert_eq!(result.disposition, ReleaseDisposition::Released);
    assert!(result.changed);
    assert!(released.expose(&released_ref).is_err());
    assert!(released.collect(&released_ref).is_err());

    let after = released.world(&released_ref).unwrap();
    assert_eq!(after.world_ref, before.world_ref);
    assert_eq!(after.provenance, before.provenance);
    assert_eq!(after.subjects.get("candidate"), Some(&released_candidate));
    assert!(after
        .binding_graph
        .bindings
        .iter()
        .all(|binding| binding.presence == epilogos_workcell_core::BindingPresence::Released));
    assert!(after
        .binding_graph
        .bindings
        .iter()
        .all(|binding| !binding.provenance.is_empty()));

    let _ = fs::remove_dir_all(preserved_root);
    let _ = fs::remove_dir_all(released_root);
}

#[test]
fn post_prepare_provider_loss_uses_requirement_necessity() {
    let (control, world_ref, root, _) = prepared(RetentionExpectation::Release);
    let world = control.world(&world_ref).unwrap().clone();

    let mut required =
        PreparedWorldControlPlane::new(WorkcellRef::new("workcell:surface-test").unwrap());
    required.register_world(world.clone()).unwrap();
    assert!(required.expose(&world_ref).is_err());
    assert!(required.collect(&world_ref).is_err());

    let mut preferred_world = world.clone();
    preferred_world.planned_exposures[0].necessity = RequirementNecessity::Preferred;
    preferred_world
        .binding_graph
        .bindings
        .iter_mut()
        .find(|binding| binding.port == epilogos_workcell_core::ProviderPortKind::ArtifactStorage)
        .unwrap()
        .necessity = RequirementNecessity::Preferred;
    let mut preferred =
        PreparedWorldControlPlane::new(WorkcellRef::new("workcell:surface-test").unwrap());
    preferred.register_world(preferred_world).unwrap();
    assert_eq!(preferred.expose(&world_ref).unwrap().degradations.len(), 1);
    assert_eq!(preferred.collect(&world_ref).unwrap().degradations.len(), 1);

    let mut optional_world = world;
    optional_world.planned_exposures[0].necessity = RequirementNecessity::Optional;
    optional_world
        .binding_graph
        .bindings
        .iter_mut()
        .find(|binding| binding.port == epilogos_workcell_core::ProviderPortKind::ArtifactStorage)
        .unwrap()
        .necessity = RequirementNecessity::Optional;
    let mut optional =
        PreparedWorldControlPlane::new(WorkcellRef::new("workcell:surface-test").unwrap());
    optional.register_world(optional_world).unwrap();
    assert_eq!(optional.expose(&world_ref).unwrap().omissions.len(), 1);
    assert_eq!(optional.collect(&world_ref).unwrap().omissions.len(), 1);

    let _ = fs::remove_dir_all(root);
}
