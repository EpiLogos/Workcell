use epilogos_workcell_core::{
    compose_world, plan, validate_allocation, validate_provider_port, DemandRef, Discovery,
    ExecutionDemand, HealthState, LogicalConnectionRequirement, PlanStatus, PlannedAllocation,
    PlannedRelation, ProjectRuntimeMaterialRequest, ProjectRuntimeProvider,
    ProjectRuntimeRequirement, ProviderPort, ProviderRef, RetentionExpectation,
    ServiceMaterialRequest, ServiceProvider, WorkcellRef,
};
use epilogos_workcell_runtime::{
    ReferenceProjectRuntimeProvider, RuntimeMode, StaticService, StaticServiceProvider,
};

fn runtime_provider() -> ReferenceProjectRuntimeProvider {
    ReferenceProjectRuntimeProvider::new(
        ProviderRef::new("provider:runtime-reference").unwrap(),
        [RuntimeMode::new("agent")
            .unwrap()
            .with_connection("project:self")
            .with_exposure("browser")
            .with_endpoint("http://project.internal:3000")],
    )
    .unwrap()
}

fn service_provider(endpoint: &str, search_health: Option<HealthState>) -> StaticServiceProvider {
    let mut services = vec![StaticService::new("state:graph", endpoint).unwrap()];
    if let Some(health) = search_health {
        services.push(
            StaticService::new("search:web", "http://search.internal:8080")
                .unwrap()
                .with_health(health),
        );
    }
    StaticServiceProvider::new(
        ProviderRef::new(format!("provider:services:{endpoint}")).unwrap(),
        services,
    )
    .unwrap()
}

fn demand() -> ExecutionDemand {
    let mut demand = ExecutionDemand::new(DemandRef::new("demand:runtime-services").unwrap());
    demand.project_runtime = Some(ProjectRuntimeRequirement::new("agent").unwrap());
    demand
        .connectivity
        .required
        .push(LogicalConnectionRequirement::new("state:graph").unwrap());
    demand
}

fn runtime_request(demand: &ExecutionDemand) -> ProjectRuntimeMaterialRequest {
    ProjectRuntimeMaterialRequest {
        demand_ref: demand.demand_ref.clone(),
        mode: demand.project_runtime.as_ref().unwrap().as_str().into(),
        connectivity: vec![LogicalConnectionRequirement::new("project:self").unwrap()],
        persistence: demand.persistence.clone(),
        retention: demand.retention.clone(),
    }
}

fn service_request(demand: &ExecutionDemand, logical_ref: &str) -> ServiceMaterialRequest {
    ServiceMaterialRequest {
        demand_ref: demand.demand_ref.clone(),
        connection: LogicalConnectionRequirement::new(logical_ref).unwrap(),
        persistence: demand.persistence.clone(),
    }
}

#[test]
fn runtime_and_service_compose_into_one_native_data_plane_world() {
    let demand = demand();
    let mut runtime = runtime_provider();
    let mut services = service_provider("neo4j://graph.internal:7687", None);
    validate_provider_port(&runtime).unwrap();
    validate_provider_port(&services).unwrap();

    let mut offers = runtime.offers().unwrap();
    offers.extend(services.offers().unwrap());
    let plan = plan(
        &demand,
        &Discovery {
            workcell_ref: WorkcellRef::new("workcell:reference").unwrap(),
            health: HealthState::Healthy,
            offers,
        },
    )
    .unwrap();
    assert_eq!(plan.status, PlanStatus::Satisfiable);

    let runtime_allocation = runtime.prepare_runtime(&runtime_request(&demand)).unwrap();
    let service_allocation = services
        .resolve_service(&service_request(&demand, "state:graph"))
        .unwrap();
    validate_allocation(&runtime, &runtime_allocation).unwrap();
    validate_allocation(&services, &service_allocation).unwrap();

    let runtime_plan = plan
        .planned_bindings
        .iter()
        .find(|binding| binding.logical_ref == "project-runtime:runtime-mode:agent")
        .unwrap();
    let service_plan = plan
        .planned_bindings
        .iter()
        .find(|binding| binding.logical_ref == "connectivity:state:graph")
        .unwrap();
    let world = compose_world(
        WorkcellRef::new("workcell:reference").unwrap(),
        &demand,
        &plan,
        vec![
            PlannedAllocation {
                logical_ref: runtime_plan.logical_ref.clone(),
                offer_ref: runtime_plan.offer_ref.clone(),
                allocation: runtime_allocation,
            },
            PlannedAllocation {
                logical_ref: service_plan.logical_ref.clone(),
                offer_ref: service_plan.offer_ref.clone(),
                allocation: service_allocation,
            },
        ],
        vec![PlannedRelation {
            from_logical_ref: runtime_plan.logical_ref.clone(),
            to_logical_ref: service_plan.logical_ref.clone(),
            relation: "can-reach".into(),
        }],
    )
    .unwrap();

    let runtime_binding = world
        .binding_graph
        .bindings
        .iter()
        .find(|binding| binding.logical_ref.starts_with("project-runtime:"))
        .unwrap();
    let service_binding = world
        .binding_graph
        .bindings
        .iter()
        .find(|binding| binding.logical_ref == "connectivity:state:graph")
        .unwrap();
    assert_eq!(
        runtime_binding.properties.get("endpoint").unwrap(),
        "http://project.internal:3000"
    );
    assert_eq!(
        runtime_binding.properties.get("exposures").unwrap(),
        "browser"
    );
    assert_eq!(
        runtime_binding.properties.get("connections").unwrap(),
        "project:self"
    );
    assert_eq!(
        service_binding.properties.get("endpoint").unwrap(),
        "neo4j://graph.internal:7687"
    );
    assert_eq!(
        service_binding.provenance.get("logical_ref").unwrap(),
        "state:graph"
    );
    assert!(!service_binding.properties.contains_key("proxy"));
    assert_eq!(world.binding_graph.relations.len(), 1);
}

#[test]
fn runtime_is_ensured_observed_exposable_and_stopped_through_its_port() {
    let demand = demand();
    let mut runtime = runtime_provider();
    let offer = runtime.offers().unwrap().remove(0);
    assert!(offer.exposures.iter().any(|value| value == "browser"));
    assert!(offer.connections.iter().any(|value| value == "project:self"));

    let allocation = runtime.prepare_runtime(&runtime_request(&demand)).unwrap();
    assert_eq!(
        allocation.properties.get("endpoint").unwrap(),
        "http://project.internal:3000"
    );
    let observation = runtime.observe_runtime(&allocation).unwrap();
    assert_eq!(observation.detail.get("running").unwrap(), "true");
    assert_eq!(observation.detail.get("mode").unwrap(), "agent");

    let preserved = runtime
        .release_runtime(&allocation, &RetentionExpectation::Preserve)
        .unwrap();
    assert!(!preserved.changed);
    runtime.observe_runtime(&allocation).unwrap();

    let released = runtime
        .release_runtime(&allocation, &RetentionExpectation::Release)
        .unwrap();
    assert!(released.changed);
    assert!(runtime.observe_runtime(&allocation).is_err());
}

#[test]
fn required_and_preferred_service_failures_remain_explicit() {
    let required = demand();
    let runtime = runtime_provider();
    let empty_services =
        StaticServiceProvider::new(ProviderRef::new("provider:services:empty").unwrap(), [])
            .unwrap();
    let mut offers = runtime.offers().unwrap();
    offers.extend(empty_services.offers().unwrap());
    let required_plan = plan(
        &required,
        &Discovery {
            workcell_ref: WorkcellRef::new("workcell:required-missing").unwrap(),
            health: HealthState::Healthy,
            offers,
        },
    )
    .unwrap();
    assert_eq!(required_plan.status, PlanStatus::Unsatisfiable);
    assert!(required_plan
        .omissions
        .iter()
        .any(|item| item.requirement == "connectivity:state:graph"));

    let mut preferred = demand();
    preferred
        .connectivity
        .preferred
        .push(LogicalConnectionRequirement::new("search:web").unwrap());
    let runtime = runtime_provider();
    let degraded_services =
        service_provider("neo4j://graph.internal:7687", Some(HealthState::Degraded));
    let mut offers = runtime.offers().unwrap();
    offers.extend(degraded_services.offers().unwrap());
    let preferred_plan = plan(
        &preferred,
        &Discovery {
            workcell_ref: WorkcellRef::new("workcell:preferred-degraded").unwrap(),
            health: HealthState::Healthy,
            offers,
        },
    )
    .unwrap();
    assert_eq!(preferred_plan.status, PlanStatus::Degraded);
    assert!(preferred_plan
        .degradations
        .iter()
        .any(|item| item.requirement == "connectivity:search:web"));
}

#[test]
fn logical_service_can_move_between_providers_without_changing_demand() {
    let demand = demand();
    let before = demand.clone();
    let request = service_request(&demand, "state:graph");
    let mut local = service_provider("neo4j://local.internal:7687", None);
    let mut remote = service_provider("neo4j+s://managed.example:7687", None);

    let local_allocation = local.resolve_service(&request).unwrap();
    let remote_allocation = remote.resolve_service(&request).unwrap();
    assert_eq!(demand, before);
    assert_eq!(
        local_allocation.properties.get("endpoint").unwrap(),
        "neo4j://local.internal:7687"
    );
    assert_eq!(
        remote_allocation.properties.get("endpoint").unwrap(),
        "neo4j+s://managed.example:7687"
    );
    assert_ne!(
        local_allocation.provider_ref,
        remote_allocation.provider_ref
    );
}
