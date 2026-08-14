use std::collections::BTreeMap;

use crate::{DemandRef, OfferRef, PlanRef, ProviderRef, WorkcellRef};

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
pub struct PlannedExposure {
    pub logical_ref: String,
    pub requirement: String,
    pub necessity: RequirementNecessity,
    pub provider_ref: ProviderRef,
    pub offer_ref: OfferRef,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PlannedConstraint {
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
    pub planned_exposures: Vec<PlannedExposure>,
    pub planned_constraints: Vec<PlannedConstraint>,
    pub degradations: Vec<Degradation>,
    pub omissions: Vec<PlanOmission>,
    pub explanation: Vec<String>,
}
