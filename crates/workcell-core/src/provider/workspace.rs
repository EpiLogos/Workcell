use std::collections::BTreeMap;

use super::{ProviderAllocation, ProviderObservation, ProviderPort, ProviderReleaseResult};
use crate::{
    DemandRef, ExternalRef, PersistenceScope, Result, RetentionExpectation, WorkspaceAccess,
};

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct WorkspaceMaterialSource {
    pub locator: String,
    pub provenance: BTreeMap<String, String>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct WorkspaceMaterialRequest {
    pub demand_ref: DemandRef,
    pub source: Option<ExternalRef>,
    pub material_source: Option<WorkspaceMaterialSource>,
    pub revision: Option<String>,
    pub access: WorkspaceAccess,
    pub persistence: Option<PersistenceScope>,
    pub retention: RetentionExpectation,
    /// Named branch a writable workspace must materialise on, when the caller's
    /// branch law names one (e.g. `aikit/<run-slug>`). `None` leaves the
    /// provider's default checkout form (a detached worktree for git).
    pub branch_name: Option<String>,
}

pub trait WorkspaceProvider: ProviderPort {
    fn prepare_workspace(
        &mut self,
        request: &WorkspaceMaterialRequest,
    ) -> Result<ProviderAllocation>;
    fn observe_workspace(&self, allocation: &ProviderAllocation) -> Result<ProviderObservation>;
    fn release_workspace(
        &mut self,
        allocation: &ProviderAllocation,
        retention: &RetentionExpectation,
    ) -> Result<ProviderReleaseResult>;
}

/// Boxes compose like the providers they hold. This is what lets a host
/// register an external workspace provider constructed behind a factory
/// without the runtime knowing its concrete type — the same admission seam
/// the execution port already names (`docs/PROVIDER-SDK.md`).
impl<T: WorkspaceProvider + ?Sized> WorkspaceProvider for Box<T> {
    fn prepare_workspace(
        &mut self,
        request: &WorkspaceMaterialRequest,
    ) -> Result<ProviderAllocation> {
        (**self).prepare_workspace(request)
    }

    fn observe_workspace(&self, allocation: &ProviderAllocation) -> Result<ProviderObservation> {
        (**self).observe_workspace(allocation)
    }

    fn release_workspace(
        &mut self,
        allocation: &ProviderAllocation,
        retention: &RetentionExpectation,
    ) -> Result<ProviderReleaseResult> {
        (**self).release_workspace(allocation, retention)
    }
}
