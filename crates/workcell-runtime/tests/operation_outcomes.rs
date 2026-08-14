use epilogos_workcell_core::{
    compose_world, plan, DemandRef, Discovery, ExecutionDemand, ExposureRequirement, HealthState,
    OutputRequirement, PlannedAllocation, PreparedWorldControlPlane, ProjectRuntimeMaterialRequest,
    ProjectRuntimeProvider, ProjectRuntimeRequirement, ProviderPort, ProviderRef,
    RequirementNecessity, RetentionExpectation, WorkcellControlPlane, WorkcellRef,
};
use epilogos_workcell_runtime::{ReferenceProjectRuntimeProvider, RuntimeMode};

fn runtime(provider_ref: &str) -> ReferenceProjectRuntimeProvider {
    ReferenceProjectRuntimeProvider::new(
        ProviderRef::new(provider_ref).unwrap(),
        [RuntimeMode::new("agent")
            .unwrap()
            .with_endpoint("http://candidate.internal:3000")],
    )
    .unwrap()
}

fn runtime_request(demand: &ExecutionDemand) -> ProjectRuntimeMaterialRequest {
    ProjectRuntimeMaterialRequest {
        demand_ref: demand.demand_ref.clone(),
        mode: demand.project_runtime.as_ref().unwrap().as_str().into(),
        connectivity: vec![],
        persistence: demand.persistence.clone(),
        retention: RetentionExpectation::Release,
    }
}

fn prepared_runtime_world(
    demand: &ExecutionDemand,
    mut runtime: ReferenceProjectRuntimeProvider,
) -> (PreparedWorldControlPlane, epilogos_workcell_core::WorldRef) {
    let discovery = Discovery {
        workcell_ref: WorkcellRef::new("workcell:operation-outcomes").unwrap(),
        health: HealthState::Healthy,
        offers: runtime.offers().unwrap(),
    };
    let plan = plan(demand, &discovery).unwrap();
    let runtime_binding = plan
        .planned_bindings
        .iter()
        .find(|binding| binding.logical_ref == "project-runtime:runtime-mode:agent")
        .unwrap();
    let allocation = runtime.prepare_runtime(&runtime_request(demand)).unwrap();
    let world = compose_world(
        discovery.workcell_ref.clone(),
        demand,
        &plan,
        vec![PlannedAllocation {
            logical_ref: runtime_binding.logical_ref.clone(),
            offer_ref: runtime_binding.offer_ref.clone(),
            allocation,
        }],
        vec![],
    )
    .unwrap();
    let world_ref = world.world_ref.clone();
    let mut control = PreparedWorldControlPlane::new(discovery.workcell_ref);
    control.register_world(world).unwrap();
    control.register_runtime_provider(runtime).unwrap();
    (control, world_ref)
}

#[test]
fn expose_replays_plan_time_preferred_and_optional_absence() {
    let mut preferred =
        ExecutionDemand::new(DemandRef::new("demand:preferred-exposure-outcome").unwrap());
    preferred.project_runtime = Some(ProjectRuntimeRequirement::new("agent").unwrap());
    preferred
        .exposure
        .preferred
        .push(ExposureRequirement::new("browser").unwrap());
    let (control, world_ref) = prepared_runtime_world(
        &preferred,
        runtime("provider:preferred-exposure-outcome"),
    );
    let result = control.expose(&world_ref).unwrap();
    assert_eq!(result.surfaces.len(), 0);
    assert_eq!(result.degradations.len(), 1);
    assert_eq!(
        result.degradations[0].necessity,
        RequirementNecessity::Preferred
    );
    assert_eq!(result.degradations[0].requirement, "exposure:browser");

    let mut optional =
        ExecutionDemand::new(DemandRef::new("demand:optional-exposure-outcome").unwrap());
    optional.project_runtime = Some(ProjectRuntimeRequirement::new("agent").unwrap());
    optional
        .exposure
        .optional
        .push(ExposureRequirement::new("browser").unwrap());
    let (control, world_ref) = prepared_runtime_world(
        &optional,
        runtime("provider:optional-exposure-outcome"),
    );
    let result = control.expose(&world_ref).unwrap();
    assert_eq!(result.surfaces.len(), 0);
    assert_eq!(result.omissions.len(), 1);
    assert_eq!(result.omissions[0].necessity, RequirementNecessity::Optional);
    assert_eq!(result.omissions[0].requirement, "exposure:browser");
}

#[test]
fn collect_replays_plan_time_preferred_and_optional_absence() {
    let mut preferred =
        ExecutionDemand::new(DemandRef::new("demand:preferred-output-outcome").unwrap());
    preferred.project_runtime = Some(ProjectRuntimeRequirement::new("agent").unwrap());
    preferred
        .outputs
        .preferred
        .push(OutputRequirement::new("logs:run").unwrap());
    let (control, world_ref) = prepared_runtime_world(
        &preferred,
        runtime("provider:preferred-output-outcome"),
    );
    let result = control.collect(&world_ref).unwrap();
    assert_eq!(result.outputs.len(), 0);
    assert_eq!(result.degradations.len(), 1);
    assert_eq!(
        result.degradations[0].necessity,
        RequirementNecessity::Preferred
    );
    assert_eq!(result.degradations[0].requirement, "output:logs:run");

    let mut optional =
        ExecutionDemand::new(DemandRef::new("demand:optional-output-outcome").unwrap());
    optional.project_runtime = Some(ProjectRuntimeRequirement::new("agent").unwrap());
    optional
        .outputs
        .optional
        .push(OutputRequirement::new("logs:run").unwrap());
    let (control, world_ref) = prepared_runtime_world(
        &optional,
        runtime("provider:optional-output-outcome"),
    );
    let result = control.collect(&world_ref).unwrap();
    assert_eq!(result.outputs.len(), 0);
    assert_eq!(result.omissions.len(), 1);
    assert_eq!(result.omissions[0].necessity, RequirementNecessity::Optional);
    assert_eq!(result.omissions[0].requirement, "output:logs:run");
}
