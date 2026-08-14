use std::collections::BTreeMap;

use crate::{
    BindingRef, DemandRef, ExternalRef, OfferRef, PlanRef, ProviderRef, WorkcellRef, WorldRef,
};

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Availability {
    Available,
    Degraded,
    Unavailable,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum HealthState {
    Healthy,
    Degraded,
    Unavailable,
    Unknown,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Capacity {
    pub amount: u64,
    pub unit: Option<String>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct OperationalOffer {
    pub offer_ref: OfferRef,
    pub provider_ref: ProviderRef,
    pub port: String,
    pub affordances: Vec<String>,
    pub connections: Vec<String>,
    pub exposures: Vec<String>,
    pub isolation_trust: Vec<String>,
    pub availability: Availability,
    pub health: HealthState,
    pub capacity: BTreeMap<String, Capacity>,
    pub metadata: BTreeMap<String, String>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Discovery {
    pub workcell_ref: WorkcellRef,
    pub health: HealthState,
    pub offers: Vec<OperationalOffer>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RequirementNecessity {
    Required,
    Preferred,
    Optional,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum PlanStatus {
    Satisfiable,
    Degraded,
    Unsatisfiable,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Degradation {
    pub requirement: String,
    pub necessity: RequirementNecessity,
    pub reason: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PlanOmission {
    pub requirement: String,
    pub necessity: RequirementNecessity,
    pub reason: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PlannedBinding {
    pub logical_ref: String,
    pub requirement: String,
    pub necessity: RequirementNecessity,
    pub provider_ref: ProviderRef,
    pub offer_ref: OfferRef,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct MaterialisationPlan {
    pub plan_ref: PlanRef,
    pub demand_ref: DemandRef,
    pub status: PlanStatus,
    pub planned_bindings: Vec<PlannedBinding>,
    pub degradations: Vec<Degradation>,
    pub omissions: Vec<PlanOmission>,
    pub explanation: Vec<String>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Binding {
    pub binding_ref: BindingRef,
    pub logical_ref: String,
    pub provider_ref: ProviderRef,
    pub material_ref: String,
    pub provenance: BTreeMap<String, String>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct MaterialisedExecutionWorld {
    pub world_ref: WorldRef,
    pub workcell_ref: WorkcellRef,
    pub demand_ref: DemandRef,
    pub subjects: BTreeMap<String, ExternalRef>,
    pub bindings: Vec<Binding>,
    pub state: HealthState,
    pub provenance: BTreeMap<String, String>,
}

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
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ExposureBundle {
    pub world_ref: WorldRef,
    pub surfaces: Vec<Exposure>,
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
