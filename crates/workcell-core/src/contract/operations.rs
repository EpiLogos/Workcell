use std::collections::BTreeMap;

use crate::{Degradation, HealthState, PlanOmission, ProviderRef, WorldRef};

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct MaterialObservation {
    pub logical_ref: String,
    pub state: HealthState,
    pub detail: BTreeMap<String, String>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ObservationBundle {
    pub world_ref: WorldRef,
    pub observations: Vec<MaterialObservation>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Exposure {
    pub logical_ref: String,
    pub interaction: String,
    pub material: BTreeMap<String, String>,
    pub provenance: BTreeMap<String, String>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ExposureBundle {
    pub world_ref: WorldRef,
    pub surfaces: Vec<Exposure>,
    pub degradations: Vec<Degradation>,
    pub omissions: Vec<PlanOmission>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CollectedOutput {
    pub logical_ref: String,
    pub material_locator: String,
    pub provenance: BTreeMap<String, String>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CollectionBundle {
    pub world_ref: WorldRef,
    pub outputs: Vec<CollectedOutput>,
    pub degradations: Vec<Degradation>,
    pub omissions: Vec<PlanOmission>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ReleaseDisposition {
    Released,
    Preserved,
    Suspended,
    Snapshotted,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ReleaseResult {
    pub world_ref: WorldRef,
    pub disposition: ReleaseDisposition,
    pub changed: bool,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DesiredMaterialState {
    pub logical_ref: String,
    pub desired: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ReconciliationDelta {
    pub logical_ref: String,
    pub observed: Option<String>,
    pub desired: String,
    pub action: Option<String>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ReconciliationResult {
    pub deltas: Vec<ReconciliationDelta>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct MaterialProvenanceRef {
    pub provider_ref: ProviderRef,
    pub material_ref: String,
}
