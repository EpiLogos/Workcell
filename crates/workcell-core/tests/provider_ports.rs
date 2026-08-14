use std::collections::BTreeMap;

use epilogos_workcell_core::{
    validate_allocation, validate_provider_port, ArtifactStorageProvider, Availability,
    ExecutionProvider, HealthState, OperationalOffer, ProjectRuntimeProvider, ProviderAllocation,
    ProviderObservation, ProviderPort, ProviderPortKind, ProviderRef, ProviderReleaseResult,
    ReleaseDisposition, RetentionExpectation, ServiceProvider, WorkspaceMaterialRequest,
    WorkspaceProvider,
};

struct WorkspaceOnly {
    provider_ref: ProviderRef,
}

impl WorkspaceOnly {
    fn new() -> Self {
        Self {
            provider_ref: ProviderRef::new("provider:workspace-fixture").unwrap(),
        }
    }
}

impl ProviderPort for WorkspaceOnly {
    fn provider_ref(&self) -> &ProviderRef {
        &self.provider_ref
    }

    fn port_kind(&self) -> ProviderPortKind {
        ProviderPortKind::Workspace
    }

    fn offers(&self) -> epilogos_workcell_core::Result<Vec<OperationalOffer>> {
        Ok(vec![OperationalOffer {
            offer_ref: epilogos_workcell_core::OfferRef::new("offer:workspace-fixture").unwrap(),
            provider_ref: self.provider_ref.clone(),
            port: "workspace".into(),
            affordances: vec!["workspace:writable".into()],
            connections: vec![],
            exposures: vec![],
            isolation_trust: vec![],
            availability: Availability::Available,
            health: HealthState::Healthy,
            capacity: BTreeMap::new(),
            metadata: BTreeMap::new(),
        }])
    }
}

impl WorkspaceProvider for WorkspaceOnly {
    fn prepare_workspace(
        &mut self,
        _: &WorkspaceMaterialRequest,
    ) -> epilogos_workcell_core::Result<ProviderAllocation> {
        Ok(ProviderAllocation {
            provider_ref: self.provider_ref.clone(),
            port: ProviderPortKind::Workspace,
            material_ref: "workspace-material:fixture".into(),
            health: HealthState::Healthy,
            properties: BTreeMap::new(),
            provenance: BTreeMap::new(),
        })
    }

    fn observe_workspace(
        &self,
        allocation: &ProviderAllocation,
    ) -> epilogos_workcell_core::Result<ProviderObservation> {
        Ok(ProviderObservation {
            provider_ref: self.provider_ref.clone(),
            material_ref: allocation.material_ref.clone(),
            health: HealthState::Healthy,
            detail: BTreeMap::new(),
        })
    }

    fn release_workspace(
        &mut self,
        allocation: &ProviderAllocation,
        _: &RetentionExpectation,
    ) -> epilogos_workcell_core::Result<ProviderReleaseResult> {
        Ok(ProviderReleaseResult {
            provider_ref: self.provider_ref.clone(),
            material_ref: allocation.material_ref.clone(),
            disposition: ReleaseDisposition::Released,
            changed: true,
        })
    }
}

#[test]
fn workspace_provider_uses_shared_offer_and_allocation_conformance() {
    let mut provider = WorkspaceOnly::new();
    validate_provider_port(&provider).unwrap();

    let request = WorkspaceMaterialRequest {
        demand_ref: epilogos_workcell_core::DemandRef::new("demand:workspace-fixture").unwrap(),
        source: None,
        material_source: None,
        revision: None,
        access: epilogos_workcell_core::WorkspaceAccess::Writable,
        persistence: None,
        retention: RetentionExpectation::Release,
    };
    let allocation = provider.prepare_workspace(&request).unwrap();
    validate_allocation(&provider, &allocation).unwrap();
    assert_eq!(allocation.port, ProviderPortKind::Workspace);
}

#[test]
fn provider_families_are_distinct_trait_surfaces() {
    fn accepts_workspace<T: WorkspaceProvider>(_: &T) {}
    let provider = WorkspaceOnly::new();
    accepts_workspace(&provider);

    let _: Option<&mut dyn ExecutionProvider> = None;
    let _: Option<&mut dyn ProjectRuntimeProvider> = None;
    let _: Option<&mut dyn ServiceProvider> = None;
    let _: Option<&mut dyn ArtifactStorageProvider> = None;
}

struct Mislabelled(WorkspaceOnly);

impl ProviderPort for Mislabelled {
    fn provider_ref(&self) -> &ProviderRef {
        self.0.provider_ref()
    }

    fn port_kind(&self) -> ProviderPortKind {
        ProviderPortKind::Workspace
    }

    fn offers(&self) -> epilogos_workcell_core::Result<Vec<OperationalOffer>> {
        let mut offers = self.0.offers()?;
        offers[0].port = "execution".into();
        Ok(offers)
    }
}

#[test]
fn conformance_rejects_provider_family_mismatch() {
    let provider = Mislabelled(WorkspaceOnly::new());
    assert!(validate_provider_port(&provider).is_err());
}
