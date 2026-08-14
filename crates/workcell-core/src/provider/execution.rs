use std::collections::BTreeMap;

use super::{ProviderAllocation, ProviderObservation, ProviderPort, ProviderReleaseResult};
use crate::{
    DemandRef, IsolationTrustRequirement, LogicalConnectionRequirement, ProviderRef,
    ResourceRequirement, RetentionExpectation, Result,
};

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ExecutionMaterialRequest {
    pub demand_ref: DemandRef,
    pub affordances: Vec<String>,
    pub resources: Vec<ResourceRequirement>,
    pub connectivity: Vec<LogicalConnectionRequirement>,
    pub isolation_trust: Option<IsolationTrustRequirement>,
    pub retention: RetentionExpectation,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ProviderOperation {
    pub key: String,
    pub parameters: BTreeMap<String, String>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ProviderOperationResult {
    pub provider_ref: ProviderRef,
    pub material_ref: String,
    pub operation: String,
    pub output: BTreeMap<String, String>,
    pub provenance: BTreeMap<String, String>,
}

pub trait ExecutionProvider: ProviderPort {
    fn prepare_execution(
        &mut self,
        request: &ExecutionMaterialRequest,
    ) -> Result<ProviderAllocation>;
    fn execute_operation(
        &mut self,
        allocation: &ProviderAllocation,
        operation: &ProviderOperation,
    ) -> Result<ProviderOperationResult>;
    fn observe_execution(&self, allocation: &ProviderAllocation) -> Result<ProviderObservation>;
    fn release_execution(
        &mut self,
        allocation: &ProviderAllocation,
        retention: &RetentionExpectation,
    ) -> Result<ProviderReleaseResult>;
}
