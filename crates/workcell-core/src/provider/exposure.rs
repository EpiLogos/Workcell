use std::collections::BTreeMap;

use super::{ProviderAllocation, ProviderPort};
use crate::{
    DemandRef, ExposureRequirement, ProviderRef, RequirementNecessity, Result,
};

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ProviderExposureRequest {
    pub demand_ref: DemandRef,
    pub requirement: ExposureRequirement,
    pub necessity: RequirementNecessity,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ProviderExposedSurface {
    pub provider_ref: ProviderRef,
    pub material_ref: String,
    pub logical_ref: String,
    pub interaction: String,
    pub material: BTreeMap<String, String>,
    pub provenance: BTreeMap<String, String>,
}

/// Optional provider facet for projecting interaction surfaces from material
/// that already exists. Exposure does not create a second semantic resource.
pub trait MaterialExposureProvider: ProviderPort {
    fn expose_material(
        &self,
        allocation: &ProviderAllocation,
        request: &ProviderExposureRequest,
    ) -> Result<ProviderExposedSurface>;
}
