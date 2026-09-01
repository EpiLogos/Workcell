use std::collections::BTreeMap;

use super::ProviderAllocation;
use crate::{ProviderRef, Result};

/// Provider-controlled expiry of one material allocation.
///
/// A lease is material state: it does not replace persistence intent, retention
/// intent or caller semantic identity. Providers with manual/unbounded lifetime
/// return `None` from `observe_lease`.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct MaterialLease {
    pub provider_ref: ProviderRef,
    pub material_ref: String,
    pub expires_at: String,
    pub renewable: bool,
    pub provenance: BTreeMap<String, String>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct LeaseRenewalRequest {
    /// Provider-neutral absolute RFC3339 timestamp when the provider supports
    /// absolute renewal. Providers with another lease mechanism may reject it
    /// explicitly rather than reinterpret the value.
    pub expires_at: String,
}

pub trait MaterialLeaseProvider {
    fn observe_lease(&self, allocation: &ProviderAllocation) -> Result<Option<MaterialLease>>;
    fn renew_lease(
        &mut self,
        allocation: &ProviderAllocation,
        request: &LeaseRenewalRequest,
    ) -> Result<MaterialLease>;
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum MaterialCheckpointState {
    Creating,
    Ready,
    Failed,
    Unknown,
}

/// Reusable material capture returned by a provider checkpoint operation.
/// Provider-native OCI, VM-state or snapshot identifiers remain material
/// provenance and never become World/Project/Candidate identity.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct MaterialCheckpoint {
    pub provider_ref: ProviderRef,
    pub source_material_ref: String,
    pub checkpoint_ref: String,
    pub state: MaterialCheckpointState,
    pub reusable: bool,
    pub provenance: BTreeMap<String, String>,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct CheckpointRequest {
    pub name: Option<String>,
}

pub trait MaterialCheckpointProvider {
    fn checkpoint(
        &mut self,
        allocation: &ProviderAllocation,
        request: &CheckpointRequest,
    ) -> Result<MaterialCheckpoint>;
    fn observe_checkpoint(&self, checkpoint: &MaterialCheckpoint) -> Result<MaterialCheckpoint>;
}
