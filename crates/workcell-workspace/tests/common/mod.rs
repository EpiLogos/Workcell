use epilogos_workcell_core::{
    validate_allocation, validate_provider_port, HealthState, ProviderAllocation,
    WorkspaceMaterialRequest, WorkspaceProvider,
};

pub fn assert_workspace_provider_basics<P: WorkspaceProvider>(
    provider: &mut P,
    request: &WorkspaceMaterialRequest,
) -> ProviderAllocation {
    validate_provider_port(provider).unwrap();
    let allocation = provider.prepare_workspace(request).unwrap();
    validate_allocation(provider, &allocation).unwrap();
    let observation = provider.observe_workspace(&allocation).unwrap();
    assert_eq!(observation.provider_ref, allocation.provider_ref);
    assert_eq!(observation.material_ref, allocation.material_ref);
    assert_eq!(observation.health, HealthState::Healthy);
    allocation
}
