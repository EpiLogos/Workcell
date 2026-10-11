use std::collections::BTreeMap;

use crate::{BindingRef, ProviderOperation, ProviderOperationResult, ProviderRef, WorldRef};

/// One bounded operation on an existing material allocation. These references
/// identify Workcell material; semantic clients retain their own Agent and Run.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct MaterialOperationRequest {
    pub world_ref: WorldRef,
    pub binding_ref: BindingRef,
    pub expected_world_revision: String,
    pub operation_id: String,
    pub grant_ref: String,
    pub operation: ProviderOperation,
    pub timeout_ms: u64,
    pub output_limit_bytes: u64,
}

/// The serving operator's exact, finite admission. A request may name this
/// record, but cannot issue it. Its operation ID admits at most one effect;
/// durable intent and readback remain at the existing material journal.
#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub struct MaterialOperationGrant {
    pub grant_ref: String,
    pub authority_ref: String,
    pub world_ref: WorldRef,
    pub binding_ref: BindingRef,
    pub expected_world_revision: String,
    pub provider_ref: ProviderRef,
    pub material_ref: String,
    pub operation_id: String,
    pub operation: ProviderOperation,
    pub timeout_ms: u64,
    pub output_limit_bytes: u64,
    pub expires_at_unix_ms: u64,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum MaterialOperationState {
    Pending,
    Completed,
    Uncertain,
}

/// Actual retained execution reading. Pending or uncertain means the owner
/// has not established completion; neither permits another consequential call.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct MaterialOperationReading {
    pub world_ref: WorldRef,
    pub binding_ref: BindingRef,
    pub operation_id: String,
    pub request_fingerprint: String,
    pub state: MaterialOperationState,
    pub grant_ref: String,
    pub authority_ref: String,
    pub provider_ref: ProviderRef,
    pub material_ref: String,
    pub result: Option<ProviderOperationResult>,
    pub failure_kind: Option<String>,
    pub failure_observation: BTreeMap<String, String>,
}

impl MaterialOperationGrant {
    pub fn validate_request(
        &self,
        request: &MaterialOperationRequest,
        now_unix_ms: u64,
    ) -> crate::Result<()> {
        if self.grant_ref.trim().is_empty()
            || self.authority_ref.trim().is_empty()
            || self.material_ref.trim().is_empty()
            || self.operation_id.trim().is_empty()
            || self.expected_world_revision.trim().is_empty()
            || self.operation.key.trim().is_empty()
            || self.timeout_ms == 0
            || self.output_limit_bytes == 0
            || self.expires_at_unix_ms <= now_unix_ms
        {
            return Err(crate::WorkcellError::InvalidDemand(
                "material effect grant is empty, unbounded or expired".into(),
            ));
        }
        if self.grant_ref != request.grant_ref
            || self.world_ref != request.world_ref
            || self.binding_ref != request.binding_ref
            || self.expected_world_revision != request.expected_world_revision
            || self.operation_id != request.operation_id
            || self.operation != request.operation
            || self.timeout_ms != request.timeout_ms
            || self.output_limit_bytes != request.output_limit_bytes
        {
            return Err(crate::WorkcellError::InvalidDemand(
                "material request differs from its exact serving grant".into(),
            ));
        }
        Ok(())
    }
}
