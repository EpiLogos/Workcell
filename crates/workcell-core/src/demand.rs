use std::collections::BTreeMap;

use crate::{DemandRef, ExternalRef};

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Tiered<T> {
    pub required: Vec<T>,
    pub preferred: Vec<T>,
    pub optional: Vec<T>,
}

impl<T> Default for Tiered<T> {
    fn default() -> Self {
        Self {
            required: Vec::new(),
            preferred: Vec::new(),
            optional: Vec::new(),
        }
    }
}

#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct AffordanceRequirement(pub String);

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum WorkspaceAccess {
    ReadOnly,
    Writable,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct WorkspaceRequirement {
    pub source: Option<ExternalRef>,
    pub revision: Option<String>,
    pub access: WorkspaceAccess,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ResourceRequirement {
    pub key: String,
    pub minimum: Option<u64>,
    pub unit: Option<String>,
}

#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct LogicalConnectionRequirement(pub String);

#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct ExposureRequirement(pub String);

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum PersistenceScope {
    Ephemeral,
    TaskOrRun,
    Candidate,
    Project,
    Workcell,
    Factory,
    External,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct IsolationTrustRequirement(pub String);

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum RetentionExpectation {
    Release,
    Preserve,
    SuspendIfSupported,
    SnapshotIfSupported,
}

/// Provider-neutral request for a material executable world.
///
/// `subjects` is deliberately open: keys are caller-defined provenance roles
/// and values are opaque external refs. Workcell does not require a Factory
/// ontology in order to materialise an ordinary generic demand.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ExecutionDemand {
    pub demand_ref: DemandRef,
    pub subjects: BTreeMap<String, ExternalRef>,
    pub affordances: Tiered<AffordanceRequirement>,
    pub workspace: Option<WorkspaceRequirement>,
    pub resources: Vec<ResourceRequirement>,
    pub connectivity: Tiered<LogicalConnectionRequirement>,
    pub exposure: Tiered<ExposureRequirement>,
    pub persistence: Option<PersistenceScope>,
    pub isolation_trust: Option<IsolationTrustRequirement>,
    pub retention: RetentionExpectation,
    pub extensions: BTreeMap<String, String>,
}

impl ExecutionDemand {
    pub fn new(demand_ref: DemandRef) -> Self {
        Self {
            demand_ref,
            subjects: BTreeMap::new(),
            affordances: Tiered::default(),
            workspace: None,
            resources: Vec::new(),
            connectivity: Tiered::default(),
            exposure: Tiered::default(),
            persistence: None,
            isolation_trust: None,
            retention: RetentionExpectation::Release,
            extensions: BTreeMap::new(),
        }
    }

    pub fn with_subject(mut self, role: impl Into<String>, subject: ExternalRef) -> Self {
        self.subjects.insert(role.into(), subject);
        self
    }
}

/// A Candidate materialisation is a use of ExecutionDemand, not a competing
/// material-demand primitive. This constructor merely adds an opaque role.
pub fn candidate_materialisation_demand(
    demand_ref: DemandRef,
    candidate_ref: ExternalRef,
) -> ExecutionDemand {
    ExecutionDemand::new(demand_ref).with_subject("candidate", candidate_ref)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn generic_demand_requires_no_factory_subjects() {
        let demand = ExecutionDemand::new(DemandRef::new("demand:standalone").unwrap());
        assert!(demand.subjects.is_empty());
    }

    #[test]
    fn candidate_view_preserves_the_supplied_ref() {
        let candidate = ExternalRef::new("client-owned:candidate-7").unwrap();
        let demand = candidate_materialisation_demand(
            DemandRef::new("demand:candidate-7").unwrap(),
            candidate.clone(),
        );
        assert_eq!(demand.subjects.get("candidate"), Some(&candidate));
    }

    #[test]
    fn demand_source_has_no_concrete_provider_vocabulary() {
        let source = include_str!("demand.rs").to_ascii_lowercase();
        let forbidden_parts = [
            ("container", "_id"),
            ("bridge", "_ip"),
            ("worktree", "_path"),
            ("microvm", "_id"),
        ];
        for (left, right) in forbidden_parts {
            let term = format!("{left}{right}");
            assert!(
                !source.contains(&term),
                "provider detail leaked into demand: {term}"
            );
        }
    }
}
