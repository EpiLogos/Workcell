use std::collections::BTreeMap;

use epilogos_workcell_core::{
    Capacity, HealthState, PreparedWorldControlPlane, Result, WorkcellError, WorkcellRef,
};

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub struct PlacementRef(String);

impl PlacementRef {
    pub fn new(value: impl Into<String>) -> Result<Self> {
        let value = value.into();
        if value.trim().is_empty() {
            return Err(WorkcellError::InvalidDemand(
                "deployment placement reference must not be empty".into(),
            ));
        }
        Ok(Self(value))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DeploymentProfile {
    pub id: String,
    pub workcell_ref: WorkcellRef,
    pub health: HealthState,
    pub capacity: BTreeMap<String, Capacity>,
    pub placements: BTreeMap<String, PlacementRef>,
    pub metadata: BTreeMap<String, String>,
}

impl DeploymentProfile {
    pub fn new(id: impl Into<String>, workcell_ref: WorkcellRef) -> Result<Self> {
        let id = id.into();
        if id.trim().is_empty() {
            return Err(WorkcellError::InvalidDemand(
                "deployment profile id must not be empty".into(),
            ));
        }
        Ok(Self {
            id,
            workcell_ref,
            health: HealthState::Healthy,
            capacity: BTreeMap::new(),
            placements: BTreeMap::new(),
            metadata: BTreeMap::new(),
        })
    }

    pub fn with_health(mut self, health: HealthState) -> Self {
        self.health = health;
        self
    }

    pub fn with_capacity(
        mut self,
        key: impl Into<String>,
        amount: u64,
        unit: Option<impl Into<String>>,
    ) -> Result<Self> {
        let key = key.into();
        if key.trim().is_empty() {
            return Err(WorkcellError::InvalidDemand(
                "deployment capacity key must not be empty".into(),
            ));
        }
        let unit = unit.map(Into::into);
        if unit.as_ref().is_some_and(|value| value.trim().is_empty()) {
            return Err(WorkcellError::InvalidDemand(
                "deployment capacity unit must not be empty".into(),
            ));
        }
        self.capacity.insert(key, Capacity { amount, unit });
        Ok(self)
    }

    pub fn with_placement(
        mut self,
        role: impl Into<String>,
        placement: PlacementRef,
    ) -> Result<Self> {
        let role = role.into();
        if role.trim().is_empty() {
            return Err(WorkcellError::InvalidDemand(
                "deployment placement role must not be empty".into(),
            ));
        }
        self.placements.insert(role, placement);
        Ok(self)
    }

    pub fn with_metadata(
        mut self,
        key: impl Into<String>,
        value: impl Into<String>,
    ) -> Result<Self> {
        let key = key.into();
        let value = value.into();
        if key.trim().is_empty() || value.trim().is_empty() {
            return Err(WorkcellError::InvalidDemand(
                "deployment profile metadata keys and values must not be empty".into(),
            ));
        }
        self.metadata.insert(key, value);
        Ok(self)
    }

    pub fn control_plane(&self) -> Result<PreparedWorldControlPlane> {
        PreparedWorldControlPlane::new(self.workcell_ref.clone())
            .with_discovery_state(self.health.clone(), self.capacity.clone())
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DeploymentParityEntry {
    pub profile_id: String,
    pub workcell_ref: WorkcellRef,
    pub health: HealthState,
    pub capacity: BTreeMap<String, Capacity>,
    pub placements: BTreeMap<String, PlacementRef>,
}

pub fn deployment_parity_report(
    profiles: impl IntoIterator<Item = DeploymentProfile>,
) -> Result<Vec<DeploymentParityEntry>> {
    let mut report = Vec::new();
    let mut ids = std::collections::BTreeSet::new();
    let mut refs = std::collections::BTreeSet::new();
    for profile in profiles {
        if !ids.insert(profile.id.clone()) {
            return Err(WorkcellError::InvalidDemand(format!(
                "duplicate deployment profile id `{}`",
                profile.id
            )));
        }
        if !refs.insert(profile.workcell_ref.to_string()) {
            return Err(WorkcellError::InvalidDemand(format!(
                "duplicate deployment Workcell ref `{}`",
                profile.workcell_ref
            )));
        }
        report.push(DeploymentParityEntry {
            profile_id: profile.id,
            workcell_ref: profile.workcell_ref,
            health: profile.health,
            capacity: profile.capacity,
            placements: profile.placements,
        });
    }
    report.sort_by(|left, right| left.profile_id.cmp(&right.profile_id));
    Ok(report)
}
