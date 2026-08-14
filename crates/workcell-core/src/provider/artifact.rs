use std::collections::BTreeMap;

use super::{ProviderAllocation, ProviderObservation, ProviderPort, ProviderReleaseResult};
use crate::{DemandRef, PersistenceScope, ProviderRef, Result, RetentionExpectation};

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ArtifactChannelRequest {
    pub demand_ref: DemandRef,
    pub logical_channel: String,
    pub persistence: Option<PersistenceScope>,
    pub retention: RetentionExpectation,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ProviderCollectedMaterial {
    pub provider_ref: ProviderRef,
    pub material_ref: String,
    pub logical_output: String,
    pub locator: String,
    pub provenance: BTreeMap<String, String>,
}

pub trait ArtifactStorageProvider: ProviderPort {
    fn prepare_artifact_channel(
        &mut self,
        request: &ArtifactChannelRequest,
    ) -> Result<ProviderAllocation>;
    fn collect_material(
        &self,
        allocation: &ProviderAllocation,
    ) -> Result<Vec<ProviderCollectedMaterial>>;
    fn observe_artifact_channel(
        &self,
        allocation: &ProviderAllocation,
    ) -> Result<ProviderObservation>;
    fn release_artifact_channel(
        &mut self,
        allocation: &ProviderAllocation,
        retention: &RetentionExpectation,
    ) -> Result<ProviderReleaseResult>;
}
