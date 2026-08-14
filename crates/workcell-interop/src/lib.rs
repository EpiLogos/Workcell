use std::collections::BTreeMap;

use epilogos_workcell_core::{
    AffordanceRequirement, DemandRef, ExecutionDemand, ExposureRequirement, ExternalRef,
    LogicalConnectionRequirement, PersistenceScope, ResourceRequirement, Result,
    RetentionExpectation, WorkspaceAccess, WorkspaceRequirement, WorkcellError, WorkcellRef,
};
use serde_json::{Map, Value};

pub const FACTORY_INTEROP_FIXTURE_VERSION: &str = "factory.interop-fixtures/v1";
pub const FACTORY_INTEROP_PROTOCOL_VERSION: &str = "factory.interop/v1";
pub const FACTORY_INTEROP_SOURCE_REVISION: &str =
    "474a4c2c13854a5ea253d77f5aff4aa491ced2c5";
pub const FACTORY_INTEROP_SOURCE_FIXTURE: &str =
    "contracts/factory/fixtures/interop-v1.json";

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum FactoryIdentityRole {
    Project,
    Run,
    Candidate,
    Agent,
}

impl FactoryIdentityRole {
    fn prefix(self) -> &'static str {
        match self {
            Self::Project => "factory:project:",
            Self::Run => "factory:run:",
            Self::Candidate => "factory:candidate:",
            Self::Agent => "factory:agent:",
        }
    }

    fn name(self) -> &'static str {
        match self {
            Self::Project => "Project",
            Self::Run => "Run",
            Self::Candidate => "Candidate",
            Self::Agent => "Agent",
        }
    }
}

pub fn validate_factory_identity(role: FactoryIdentityRole, value: &str) -> Result<()> {
    if !value.starts_with(role.prefix()) || value.len() == role.prefix().len() {
        return Err(WorkcellError::InvalidDemand(format!(
            "Factory {} ref `{value}` does not satisfy `{}` identity encoding",
            role.name(),
            role.prefix()
        )));
    }
    Ok(())
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct FactoryExecutionDemandView {
    pub demand_ref: String,
    pub project_ref: String,
    pub run_ref: String,
    pub candidate_ref: String,
    pub required_affordances: Vec<String>,
    pub preferred_affordances: Vec<String>,
    pub optional_affordances: Vec<String>,
    pub resource_requirements: Vec<String>,
    pub connectivity: Vec<String>,
    pub workspace_semantics: String,
    pub persistence_semantics: Vec<String>,
    pub exposure_requirements: Vec<String>,
    pub isolation_requirement: String,
    pub retention_policy: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct FactoryWorkcellOfferView {
    pub workcell_ref: String,
    pub offer_revision: u64,
    pub affordances: Vec<String>,
    pub resource_classes: Vec<String>,
    pub connectivity: Vec<String>,
    pub persistence_scopes: Vec<String>,
    pub exposure_modes: Vec<String>,
    pub isolation_modes: Vec<String>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct FactoryBindingView {
    pub binding_id: String,
    pub workcell_ref: String,
    pub logical_ref: String,
    pub concrete_ref: String,
    pub materialized_world_ref: String,
    pub execution_ref: String,
    pub provider_ref: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct InteropConsumptionReport {
    pub fixture_version: String,
    pub protocol_version: String,
    pub source_revision: String,
    pub source_fixture: String,
    pub workcell_ref: String,
    pub binding_logical_ref: String,
    pub provider_ref: String,
    pub concrete_ref: String,
    pub semantic_subjects: BTreeMap<String, String>,
}

#[derive(Clone, Debug)]
pub struct WorkcellInteropFixture {
    raw: Value,
    fixture_version: String,
    protocol_version: String,
    execution_demand: FactoryExecutionDemandView,
    workcell_offer: FactoryWorkcellOfferView,
    binding: FactoryBindingView,
    anti_fixture_ids: Vec<String>,
}

impl WorkcellInteropFixture {
    pub fn parse(input: &str) -> Result<Self> {
        let raw: Value = serde_json::from_str(input).map_err(|error| {
            WorkcellError::InvalidDemand(format!("invalid Factory interop JSON: {error}"))
        })?;
        let root = as_object(&raw, "fixture root")?;
        let fixture_version = required_string(root, "fixtureVersion")?;
        if fixture_version != FACTORY_INTEROP_FIXTURE_VERSION {
            return Err(WorkcellError::Unsupported(format!(
                "unsupported Factory interop fixture version `{fixture_version}`; expected `{FACTORY_INTEROP_FIXTURE_VERSION}`"
            )));
        }
        let contract = required_object(root, "contract")?;
        let protocol_version = required_string(contract, "contractVersion")?;
        if protocol_version != FACTORY_INTEROP_PROTOCOL_VERSION {
            return Err(WorkcellError::Unsupported(format!(
                "unsupported Factory interop protocol version `{protocol_version}`; expected `{FACTORY_INTEROP_PROTOCOL_VERSION}`"
            )));
        }

        let execution_demand = parse_execution_demand(required_object(contract, "executionDemand")?)?;
        validate_factory_identity(FactoryIdentityRole::Project, &execution_demand.project_ref)?;
        validate_factory_identity(FactoryIdentityRole::Run, &execution_demand.run_ref)?;
        validate_factory_identity(FactoryIdentityRole::Candidate, &execution_demand.candidate_ref)?;

        let workcell_offer = parse_workcell_offer(required_object(contract, "workcellOffer")?)?;
        WorkcellRef::new(workcell_offer.workcell_ref.clone())?;
        let binding = parse_binding(required_object(contract, "binding")?)?;
        WorkcellRef::new(binding.workcell_ref.clone())?;
        if binding.workcell_ref != workcell_offer.workcell_ref {
            return Err(WorkcellError::InvalidDemand(format!(
                "shared binding Workcell `{}` does not match offer Workcell `{}`",
                binding.workcell_ref, workcell_offer.workcell_ref
            )));
        }
        if binding.provider_ref.trim().is_empty()
            || binding.logical_ref.trim().is_empty()
            || binding.concrete_ref.trim().is_empty()
            || binding.materialized_world_ref.trim().is_empty()
        {
            return Err(WorkcellError::InvalidDemand(
                "shared Workcell binding contains an empty material/provider identity".into(),
            ));
        }

        let anti_fixture_ids = root
            .get("antiFixtures")
            .and_then(Value::as_array)
            .map(|items| {
                items
                    .iter()
                    .filter_map(|item| item.get("id").and_then(Value::as_str))
                    .map(str::to_owned)
                    .collect()
            })
            .unwrap_or_default();

        Ok(Self {
            raw,
            fixture_version,
            protocol_version,
            execution_demand,
            workcell_offer,
            binding,
            anti_fixture_ids,
        })
    }

    pub fn fixture_version(&self) -> &str {
        &self.fixture_version
    }

    pub fn protocol_version(&self) -> &str {
        &self.protocol_version
    }

    pub fn execution_demand_view(&self) -> &FactoryExecutionDemandView {
        &self.execution_demand
    }

    pub fn workcell_offer_view(&self) -> &FactoryWorkcellOfferView {
        &self.workcell_offer
    }

    pub fn binding_view(&self) -> &FactoryBindingView {
        &self.binding
    }

    pub fn anti_fixture_ids(&self) -> &[String] {
        &self.anti_fixture_ids
    }

    pub fn raw_value(&self) -> &Value {
        &self.raw
    }

    pub fn round_trip_json(&self) -> Result<String> {
        serde_json::to_string_pretty(&self.raw).map_err(|error| {
            WorkcellError::OperationFailed(format!(
                "failed to serialize Factory interop fixture: {error}"
            ))
        })
    }

    /// Adapt the Factory-owned semantic execution demand into Workcell's
    /// provider-neutral material demand without importing Factory ontology into
    /// `workcell-core`.
    ///
    /// V1 Factory values that do not have an isomorphic Workcell field are
    /// retained in namespaced extensions. In particular, `strong-preferred`
    /// remains a preference encoded by the shared preferred
    /// `isolated-execution` affordance; it is not silently promoted to
    /// Workcell's non-tiered required isolation field.
    pub fn to_workcell_execution_demand(&self) -> Result<ExecutionDemand> {
        let source = &self.execution_demand;
        let mut demand = ExecutionDemand::new(DemandRef::new(source.demand_ref.clone())?);
        demand.subjects.insert(
            "project".into(),
            ExternalRef::new(source.project_ref.clone())?,
        );
        demand
            .subjects
            .insert("run".into(), ExternalRef::new(source.run_ref.clone())?);
        demand.subjects.insert(
            "candidate".into(),
            ExternalRef::new(source.candidate_ref.clone())?,
        );

        demand.affordances.required = source
            .required_affordances
            .iter()
            .map(|value| AffordanceRequirement::new(value.clone()))
            .collect::<Result<Vec<_>>>()?;
        demand.affordances.preferred = source
            .preferred_affordances
            .iter()
            .map(|value| AffordanceRequirement::new(value.clone()))
            .collect::<Result<Vec<_>>>()?;
        demand.affordances.optional = source
            .optional_affordances
            .iter()
            .map(|value| AffordanceRequirement::new(value.clone()))
            .collect::<Result<Vec<_>>>()?;

        demand.resources = source
            .resource_requirements
            .iter()
            .map(|value| parse_resource_requirement(value))
            .collect::<Result<Vec<_>>>()?;
        demand.connectivity.required = source
            .connectivity
            .iter()
            .map(|value| LogicalConnectionRequirement::new(value.clone()))
            .collect::<Result<Vec<_>>>()?;
        demand.exposure.required = source
            .exposure_requirements
            .iter()
            .map(|value| ExposureRequirement::new(value.clone()))
            .collect::<Result<Vec<_>>>()?;

        demand.workspace = match source.workspace_semantics.as_str() {
            "writable-project-checkout" => Some(WorkspaceRequirement {
                source: None,
                revision: None,
                access: WorkspaceAccess::Writable,
            }),
            other => {
                return Err(WorkcellError::Unsupported(format!(
                    "Factory interop v1 workspace semantics `{other}` are not supported"
                )))
            }
        };
        demand.persistence = merge_persistence(&source.persistence_semantics)?;
        demand.retention = RetentionExpectation::Release;
        demand.extensions.insert(
            "factory.interop.fixtureVersion".into(),
            self.fixture_version.clone(),
        );
        demand.extensions.insert(
            "factory.interop.protocolVersion".into(),
            self.protocol_version.clone(),
        );
        demand.extensions.insert(
            "factory.workspaceSemantics".into(),
            source.workspace_semantics.clone(),
        );
        demand.extensions.insert(
            "factory.persistenceSemantics".into(),
            source.persistence_semantics.join(","),
        );
        demand.extensions.insert(
            "factory.isolationRequirement".into(),
            source.isolation_requirement.clone(),
        );
        demand.extensions.insert(
            "factory.retentionPolicy".into(),
            source.retention_policy.clone(),
        );
        demand.validate()?;
        Ok(demand)
    }

    pub fn consumption_report(&self) -> InteropConsumptionReport {
        InteropConsumptionReport {
            fixture_version: self.fixture_version.clone(),
            protocol_version: self.protocol_version.clone(),
            source_revision: FACTORY_INTEROP_SOURCE_REVISION.into(),
            source_fixture: FACTORY_INTEROP_SOURCE_FIXTURE.into(),
            workcell_ref: self.workcell_offer.workcell_ref.clone(),
            binding_logical_ref: self.binding.logical_ref.clone(),
            provider_ref: self.binding.provider_ref.clone(),
            concrete_ref: self.binding.concrete_ref.clone(),
            semantic_subjects: BTreeMap::from([
                ("project".into(), self.execution_demand.project_ref.clone()),
                ("run".into(), self.execution_demand.run_ref.clone()),
                ("candidate".into(), self.execution_demand.candidate_ref.clone()),
            ]),
        }
    }
}

fn parse_execution_demand(object: &Map<String, Value>) -> Result<FactoryExecutionDemandView> {
    Ok(FactoryExecutionDemandView {
        demand_ref: required_string(object, "demandRef")?,
        project_ref: required_string(object, "projectRef")?,
        run_ref: required_string(object, "runRef")?,
        candidate_ref: required_string(object, "candidateRef")?,
        required_affordances: required_string_array(object, "requiredAffordances")?,
        preferred_affordances: required_string_array(object, "preferredAffordances")?,
        optional_affordances: required_string_array(object, "optionalAffordances")?,
        resource_requirements: required_string_array(object, "resourceRequirements")?,
        connectivity: required_string_array(object, "connectivity")?,
        workspace_semantics: required_string(object, "workspaceSemantics")?,
        persistence_semantics: required_string_array(object, "persistenceSemantics")?,
        exposure_requirements: required_string_array(object, "exposureRequirements")?,
        isolation_requirement: required_string(object, "isolationRequirement")?,
        retention_policy: required_string(object, "retentionPolicy")?,
    })
}

fn parse_workcell_offer(object: &Map<String, Value>) -> Result<FactoryWorkcellOfferView> {
    Ok(FactoryWorkcellOfferView {
        workcell_ref: required_string(object, "workcellRef")?,
        offer_revision: required_u64(object, "offerRevision")?,
        affordances: required_string_array(object, "affordances")?,
        resource_classes: required_string_array(object, "resourceClasses")?,
        connectivity: required_string_array(object, "connectivity")?,
        persistence_scopes: required_string_array(object, "persistenceScopes")?,
        exposure_modes: required_string_array(object, "exposureModes")?,
        isolation_modes: required_string_array(object, "isolationModes")?,
    })
}

fn parse_binding(object: &Map<String, Value>) -> Result<FactoryBindingView> {
    Ok(FactoryBindingView {
        binding_id: required_string(object, "bindingId")?,
        workcell_ref: required_string(object, "workcellRef")?,
        logical_ref: required_string(object, "logicalRef")?,
        concrete_ref: required_string(object, "concreteRef")?,
        materialized_world_ref: required_string(object, "materializedWorldRef")?,
        execution_ref: required_string(object, "executionRef")?,
        provider_ref: required_string(object, "providerRef")?,
    })
}

fn parse_resource_requirement(value: &str) -> Result<ResourceRequirement> {
    let (key, threshold) = value.split_once(">=").ok_or_else(|| {
        WorkcellError::InvalidDemand(format!(
            "Factory resource requirement `{value}` is not in `key>=amount[unit]` form"
        ))
    })?;
    if key.trim().is_empty() {
        return Err(WorkcellError::InvalidDemand(format!(
            "Factory resource requirement `{value}` has an empty key"
        )));
    }
    let digit_count = threshold
        .chars()
        .take_while(|character| character.is_ascii_digit())
        .count();
    if digit_count == 0 {
        return Err(WorkcellError::InvalidDemand(format!(
            "Factory resource requirement `{value}` has no numeric minimum"
        )));
    }
    let minimum = threshold[..digit_count].parse::<u64>().map_err(|error| {
        WorkcellError::InvalidDemand(format!(
            "Factory resource requirement `{value}` has invalid minimum: {error}"
        ))
    })?;
    let unit = threshold[digit_count..].trim();
    Ok(ResourceRequirement {
        key: key.trim().into(),
        minimum: Some(minimum),
        unit: if unit.is_empty() {
            None
        } else {
            Some(unit.into())
        },
    })
}

fn merge_persistence(values: &[String]) -> Result<Option<PersistenceScope>> {
    if values.is_empty() {
        return Ok(None);
    }
    let mut strongest = PersistenceScope::Ephemeral;
    let mut strength = 0_u8;
    for value in values {
        let (scope, current) = match value.as_str() {
            "ephemeral" => (PersistenceScope::Ephemeral, 0),
            "run" | "task-or-run" => (PersistenceScope::TaskOrRun, 1),
            "candidate" => (PersistenceScope::Candidate, 2),
            "project" => (PersistenceScope::Project, 3),
            "workcell" => (PersistenceScope::Workcell, 4),
            "factory" => (PersistenceScope::Factory, 5),
            "external" => (PersistenceScope::External, 6),
            other => {
                return Err(WorkcellError::Unsupported(format!(
                    "Factory persistence semantic `{other}` is not supported by Workcell interop v1"
                )))
            }
        };
        if current >= strength {
            strongest = scope;
            strength = current;
        }
    }
    Ok(Some(strongest))
}

fn as_object<'a>(value: &'a Value, label: &str) -> Result<&'a Map<String, Value>> {
    value.as_object().ok_or_else(|| {
        WorkcellError::InvalidDemand(format!("Factory interop {label} must be an object"))
    })
}

fn required_object<'a>(
    object: &'a Map<String, Value>,
    key: &str,
) -> Result<&'a Map<String, Value>> {
    object
        .get(key)
        .ok_or_else(|| WorkcellError::InvalidDemand(format!("Factory interop missing `{key}`")))
        .and_then(|value| as_object(value, key))
}

fn required_string(object: &Map<String, Value>, key: &str) -> Result<String> {
    object
        .get(key)
        .and_then(Value::as_str)
        .filter(|value| !value.trim().is_empty())
        .map(str::to_owned)
        .ok_or_else(|| {
            WorkcellError::InvalidDemand(format!(
                "Factory interop `{key}` must be a non-empty string"
            ))
        })
}

fn required_u64(object: &Map<String, Value>, key: &str) -> Result<u64> {
    object.get(key).and_then(Value::as_u64).ok_or_else(|| {
        WorkcellError::InvalidDemand(format!(
            "Factory interop `{key}` must be a non-negative integer"
        ))
    })
}

fn required_string_array(object: &Map<String, Value>, key: &str) -> Result<Vec<String>> {
    let values = object.get(key).and_then(Value::as_array).ok_or_else(|| {
        WorkcellError::InvalidDemand(format!("Factory interop `{key}` must be an array"))
    })?;
    values
        .iter()
        .map(|value| {
            value
                .as_str()
                .filter(|value| !value.trim().is_empty())
                .map(str::to_owned)
                .ok_or_else(|| {
                    WorkcellError::InvalidDemand(format!(
                        "Factory interop `{key}` entries must be non-empty strings"
                    ))
                })
        })
        .collect()
}
