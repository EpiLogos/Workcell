use super::{ProviderAllocation, ProviderObservation, ProviderPort, ProviderReleaseResult};
use crate::{
    DemandRef, ExternalRef, PersistenceScope, Result, RetentionExpectation, WorkspaceAccess,
};

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct WorkspaceMaterialRequest {
    pub demand_ref: DemandRef,
    pub source: Option<ExternalRef>,
    pub revision: Option<String>,
    pub access: WorkspaceAccess,
    pub persistence: Option<PersistenceScope>,
    pub retention: RetentionExpectation,
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
