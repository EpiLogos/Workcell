use std::collections::BTreeMap;

use crate::{
    BindingRef, DemandRef, ExternalRef, HealthState, OfferRef, PersistenceScope,
    PlannedConstraint, PlannedExposure, ProviderPortKind, ProviderRef, RequirementNecessity,
    RetentionExpectation, WorkcellRef, WorldRef,
};

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum BindingPresence {
    Present,
    Missing,
    Released,
    Suspended,
    Snapshotted,
    Stale,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Binding {
    pub binding_ref: BindingRef,
    pub logical_ref: String,
    pub necessity: RequirementNecessity,
    pub provider_ref: ProviderRef,
    pub offer_ref: OfferRef,
    pub port: ProviderPortKind,
    pub material_ref: String,
    pub health: HealthState,
    pub presence: BindingPresence,
    pub properties: BTreeMap<String, String>,
    pub provenance: BTreeMap<String, String>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct BindingRelation {
    pub from: BindingRef,
    pub to: BindingRef,
    pub relation: String,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct BindingGraph {
    pub bindings: Vec<Binding>,
    pub relations: Vec<BindingRelation>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct MaterialisedExecutionWorld {
    pub world_ref: WorldRef,
    pub workcell_ref: WorkcellRef,
    pub demand_ref: DemandRef,
    pub subjects: BTreeMap<String, ExternalRef>,
    pub binding_graph: BindingGraph,
    pub planned_exposures: Vec<PlannedExposure>,
    pub planned_constraints: Vec<PlannedConstraint>,
    pub persistence: Option<PersistenceScope>,
    pub retention: RetentionExpectation,
    pub state: HealthState,
    pub provenance: BTreeMap<String, String>,
}
