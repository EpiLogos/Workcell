use std::collections::{BTreeMap, BTreeSet};

use crate::{DemandRef, ExternalRef, Result, WorkcellError};

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

macro_rules! semantic_requirement {
    ($name:ident) => {
        #[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
        pub struct $name(String);

        impl $name {
            pub fn new(value: impl Into<String>) -> Result<Self> {
                let value = value.into();
                if value.trim().is_empty() {
                    return Err(WorkcellError::InvalidDemand(
                        concat!(stringify!($name), " must not be empty").into(),
                    ));
                }
                Ok(Self(value))
            }

            pub fn as_str(&self) -> &str {
                &self.0
            }
        }
    };
}

semantic_requirement!(AffordanceRequirement);
semantic_requirement!(LogicalConnectionRequirement);
semantic_requirement!(ExposureRequirement);
semantic_requirement!(OutputRequirement);
semantic_requirement!(IsolationTrustRequirement);
semantic_requirement!(ProjectRuntimeRequirement);

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

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum StorageAccess {
    ReadOnly,
    Writable,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum StorageSharing {
    Exclusive,
    Shared,
}

/// Provider-neutral requirement for storage attached to the material world.
///
/// This does not describe source/workspace identity and is not an output
/// collection channel. Providers decide the concrete volume/filesystem/storage
/// backend and mount locator below the material binding boundary.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct StorageRequirement {
    pub logical_ref: String,
    pub access: StorageAccess,
    pub sharing: StorageSharing,
    pub minimum_capacity: Option<u64>,
    pub unit: Option<String>,
    pub persistence: Option<PersistenceScope>,
    pub retention: RetentionExpectation,
}

impl StorageRequirement {
    pub fn new(logical_ref: impl Into<String>) -> Result<Self> {
        let requirement = Self {
            logical_ref: logical_ref.into(),
            access: StorageAccess::Writable,
            sharing: StorageSharing::Exclusive,
            minimum_capacity: None,
            unit: None,
            persistence: None,
            retention: RetentionExpectation::Release,
        };
        requirement.validate()?;
        Ok(requirement)
    }

    pub fn validate(&self) -> Result<()> {
        if self.logical_ref.trim().is_empty() {
            return Err(WorkcellError::InvalidDemand(
                "storage logical_ref must not be empty".into(),
            ));
        }
        if self
            .unit
            .as_deref()
            .is_some_and(|unit| unit.trim().is_empty())
        {
            return Err(WorkcellError::InvalidDemand(
                "storage capacity unit must not be empty when supplied".into(),
            ));
        }
        if self.minimum_capacity.is_some() && self.unit.is_none() {
            return Err(WorkcellError::InvalidDemand(
                "storage minimum capacity requires a unit".into(),
            ));
        }
        Ok(())
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ResourceRequirement {
    pub key: String,
    pub minimum: Option<u64>,
    pub unit: Option<String>,
}

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
    pub storage: Tiered<StorageRequirement>,
    pub project_runtime: Option<ProjectRuntimeRequirement>,
    pub resources: Vec<ResourceRequirement>,
    pub connectivity: Tiered<LogicalConnectionRequirement>,
    pub exposure: Tiered<ExposureRequirement>,
    pub outputs: Tiered<OutputRequirement>,
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
            storage: Tiered::default(),
            project_runtime: None,
            resources: Vec::new(),
            connectivity: Tiered::default(),
            exposure: Tiered::default(),
            outputs: Tiered::default(),
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

    pub fn validate(&self) -> Result<()> {
        for role in self.subjects.keys() {
            if role.trim().is_empty() {
                return Err(WorkcellError::InvalidDemand(
                    "semantic subject role must not be empty".into(),
                ));
            }
        }

        validate_tiered("affordance", &self.affordances, |item| item.as_str())?;
        validate_storage(&self.storage)?;
        validate_tiered("connectivity", &self.connectivity, |item| item.as_str())?;
        validate_tiered("exposure", &self.exposure, |item| item.as_str())?;
        validate_tiered("output", &self.outputs, |item| item.as_str())?;

        if let Some(workspace) = &self.workspace {
            if let Some(revision) = &workspace.revision {
                if revision.trim().is_empty() {
                    return Err(WorkcellError::InvalidDemand(
                        "workspace revision must not be empty".into(),
                    ));
                }
                if workspace.source.is_none() {
                    return Err(WorkcellError::InvalidDemand(
                        "workspace revision requires a source reference".into(),
                    ));
                }
            }
        }

        for resource in &self.resources {
            if resource.key.trim().is_empty() {
                return Err(WorkcellError::InvalidDemand(
                    "resource requirement key must not be empty".into(),
                ));
            }
            if let Some(unit) = &resource.unit {
                if unit.trim().is_empty() {
                    return Err(WorkcellError::InvalidDemand(
                        "resource requirement unit must not be empty".into(),
                    ));
                }
            }
        }

        for (key, value) in &self.extensions {
            if key.trim().is_empty() || value.trim().is_empty() {
                return Err(WorkcellError::InvalidDemand(
                    "extension keys and values must not be empty".into(),
                ));
            }
        }

        Ok(())
    }
}

fn validate_tiered<T, F>(label: &str, tiered: &Tiered<T>, key: F) -> Result<()>
where
    F: Fn(&T) -> &str,
{
    let mut seen = BTreeSet::new();
    for (tier_name, items) in [
        ("required", &tiered.required),
        ("preferred", &tiered.preferred),
        ("optional", &tiered.optional),
    ] {
        for item in items {
            let value = key(item);
            if value.trim().is_empty() {
                return Err(WorkcellError::InvalidDemand(format!(
                    "{label} {tier_name} entry must not be empty"
                )));
            }
            if !seen.insert(value.to_owned()) {
                return Err(WorkcellError::InvalidDemand(format!(
                    "{label} requirement `{value}` appears more than once across necessity tiers"
                )));
            }
        }
    }
    Ok(())
}

fn validate_storage(storage: &Tiered<StorageRequirement>) -> Result<()> {
    let mut seen = BTreeSet::new();
    for (tier_name, items) in [
        ("required", &storage.required),
        ("preferred", &storage.preferred),
        ("optional", &storage.optional),
    ] {
        for item in items {
            item.validate()?;
            if !seen.insert(item.logical_ref.clone()) {
                return Err(WorkcellError::InvalidDemand(format!(
                    "storage requirement `{}` appears more than once across necessity tiers ({tier_name})",
                    item.logical_ref
                )));
            }
        }
    }
    Ok(())
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

    fn affordance(value: &str) -> AffordanceRequirement {
        AffordanceRequirement::new(value).unwrap()
    }

    #[test]
    fn generic_demand_requires_no_factory_subjects() {
        let demand = ExecutionDemand::new(DemandRef::new("demand:standalone").unwrap());
        assert!(demand.subjects.is_empty());
        demand.validate().unwrap();
    }

    #[test]
    fn candidate_view_preserves_the_supplied_ref() {
        let candidate = ExternalRef::new("client-owned:candidate-7").unwrap();
        let demand = candidate_materialisation_demand(
            DemandRef::new("demand:candidate-7").unwrap(),
            candidate.clone(),
        );
        assert_eq!(demand.subjects.get("candidate"), Some(&candidate));
        demand.validate().unwrap();
    }

    #[test]
    fn semantic_requirement_tokens_must_be_nonempty() {
        assert!(AffordanceRequirement::new(" ").is_err());
        assert!(LogicalConnectionRequirement::new("").is_err());
        assert!(ExposureRequirement::new("\t").is_err());
        assert!(OutputRequirement::new(" ").is_err());
        assert!(IsolationTrustRequirement::new("  ").is_err());
        assert!(ProjectRuntimeRequirement::new(" ").is_err());
    }

    #[test]
    fn one_requirement_cannot_occupy_multiple_necessity_tiers() {
        let mut demand = ExecutionDemand::new(DemandRef::new("demand:tiers").unwrap());
        demand.affordances.required.push(affordance("shell"));
        demand.affordances.preferred.push(affordance("shell"));
        let error = demand.validate().unwrap_err();
        assert!(matches!(error, WorkcellError::InvalidDemand(_)));
    }

    #[test]
    fn attached_storage_is_distinct_and_tiered() {
        let mut demand = ExecutionDemand::new(DemandRef::new("demand:storage").unwrap());
        let mut storage = StorageRequirement::new("state:project").unwrap();
        storage.sharing = StorageSharing::Shared;
        storage.minimum_capacity = Some(20);
        storage.unit = Some("GiB".into());
        storage.persistence = Some(PersistenceScope::Project);
        storage.retention = RetentionExpectation::Preserve;
        demand.storage.required.push(storage);
        demand.validate().unwrap();
        assert_eq!(demand.storage.required[0].logical_ref, "state:project");
    }

    #[test]
    fn storage_identity_cannot_repeat_across_tiers() {
        let mut demand = ExecutionDemand::new(DemandRef::new("demand:storage-tier").unwrap());
        demand
            .storage
            .required
            .push(StorageRequirement::new("state:shared").unwrap());
        demand
            .storage
            .optional
            .push(StorageRequirement::new("state:shared").unwrap());
        assert!(matches!(
            demand.validate(),
            Err(WorkcellError::InvalidDemand(_))
        ));
    }

    #[test]
    fn logical_connectivity_stays_logical() {
        let mut demand = ExecutionDemand::new(DemandRef::new("demand:network").unwrap());
        demand
            .connectivity
            .required
            .push(LogicalConnectionRequirement::new("state:graph").unwrap());
        demand
            .connectivity
            .preferred
            .push(LogicalConnectionRequirement::new("search:web").unwrap());
        demand.validate().unwrap();
        assert_eq!(demand.connectivity.required[0].as_str(), "state:graph");
    }

    #[test]
    fn project_runtime_mode_is_open_and_provider_neutral() {
        let mut demand = ExecutionDemand::new(DemandRef::new("demand:runtime").unwrap());
        demand.project_runtime = Some(ProjectRuntimeRequirement::new("agent").unwrap());
        demand.validate().unwrap();
        assert_eq!(demand.project_runtime.unwrap().as_str(), "agent");
    }

    #[test]
    fn output_channels_are_material_requests_not_artifact_identity() {
        let mut demand = ExecutionDemand::new(DemandRef::new("demand:outputs").unwrap());
        demand
            .outputs
            .required
            .push(OutputRequirement::new("logs:run").unwrap());
        demand.validate().unwrap();
        assert_eq!(demand.outputs.required[0].as_str(), "logs:run");
    }

    #[test]
    fn workspace_revision_requires_source_provenance() {
        let mut demand = ExecutionDemand::new(DemandRef::new("demand:workspace").unwrap());
        demand.workspace = Some(WorkspaceRequirement {
            source: None,
            revision: Some("abc123".into()),
            access: WorkspaceAccess::Writable,
        });
        assert!(matches!(
            demand.validate(),
            Err(WorkcellError::InvalidDemand(_))
        ));
    }

    #[test]
    fn persistence_and_retention_are_provider_neutral_material_semantics() {
        let mut demand = ExecutionDemand::new(DemandRef::new("demand:persistence").unwrap());
        demand.persistence = Some(PersistenceScope::TaskOrRun);
        demand.retention = RetentionExpectation::SnapshotIfSupported;
        demand.validate().unwrap();
        assert_eq!(demand.persistence, Some(PersistenceScope::TaskOrRun));
    }

    #[test]
    fn resource_and_extension_shape_is_validated() {
        let mut demand = ExecutionDemand::new(DemandRef::new("demand:resources").unwrap());
        demand.resources.push(ResourceRequirement {
            key: "memory".into(),
            minimum: Some(16),
            unit: Some("GiB".into()),
        });
        demand
            .extensions
            .insert("future.material.key".into(), "v1".into());
        demand.validate().unwrap();

        demand.resources[0].unit = Some(" ".into());
        assert!(matches!(
            demand.validate(),
            Err(WorkcellError::InvalidDemand(_))
        ));
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
