use super::{ProviderAllocation, ProviderObservation, ProviderPort, ProviderReleaseResult};
use crate::{DemandRef, PersistenceScope, Result, RetentionExpectation, StorageRequirement};

/// Material allocation request for storage attached to a prepared working world.
///
/// This is intentionally distinct from WorkspaceProvider (source/revision
/// materialisation) and ArtifactStorageProvider (collected outputs). A storage
/// binding may be mounted/shared independently of either relation.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AttachedStorageRequest {
    pub demand_ref: DemandRef,
    pub requirement: StorageRequirement,
    pub persistence: Option<PersistenceScope>,
    pub retention: RetentionExpectation,
}

pub trait StorageProvider: ProviderPort {
    fn prepare_storage(&mut self, request: &AttachedStorageRequest) -> Result<ProviderAllocation>;
    fn observe_storage(&self, allocation: &ProviderAllocation) -> Result<ProviderObservation>;
    fn release_storage(
        &mut self,
        allocation: &ProviderAllocation,
        retention: &RetentionExpectation,
    ) -> Result<ProviderReleaseResult>;
}
