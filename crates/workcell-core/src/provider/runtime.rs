use super::{ProviderAllocation, ProviderObservation, ProviderPort, ProviderReleaseResult};
use crate::{
    DemandRef, LogicalConnectionRequirement, PersistenceScope, Result, RetentionExpectation,
};

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ProjectRuntimeMaterialRequest {
    pub demand_ref: DemandRef,
    pub mode: String,
    pub connectivity: Vec<LogicalConnectionRequirement>,
    pub persistence: Option<PersistenceScope>,
    pub retention: RetentionExpectation,
}

pub trait ProjectRuntimeProvider: ProviderPort {
    fn prepare_runtime(
        &mut self,
        request: &ProjectRuntimeMaterialRequest,
    ) -> Result<ProviderAllocation>;
    fn observe_runtime(&self, allocation: &ProviderAllocation) -> Result<ProviderObservation>;
    fn release_runtime(
        &mut self,
        allocation: &ProviderAllocation,
        retention: &RetentionExpectation,
    ) -> Result<ProviderReleaseResult>;
}
