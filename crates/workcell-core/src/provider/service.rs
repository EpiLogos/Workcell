use super::{ProviderAllocation, ProviderObservation, ProviderPort, ProviderReleaseResult};
use crate::{
    DemandRef, LogicalConnectionRequirement, PersistenceScope, Result, RetentionExpectation,
};

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ServiceMaterialRequest {
    pub demand_ref: DemandRef,
    pub connection: LogicalConnectionRequirement,
    pub persistence: Option<PersistenceScope>,
}

pub trait ServiceProvider: ProviderPort {
    fn resolve_service(&mut self, request: &ServiceMaterialRequest) -> Result<ProviderAllocation>;
    fn observe_service(&self, allocation: &ProviderAllocation) -> Result<ProviderObservation>;
    fn release_service(
        &mut self,
        allocation: &ProviderAllocation,
        retention: &RetentionExpectation,
    ) -> Result<ProviderReleaseResult>;
}
