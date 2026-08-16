use std::collections::BTreeMap;

use epilogos_workcell_core::{
    AffordanceRequirement, CollectionBundle, Degradation, DesiredMaterialState, Discovery,
    ExecutionDemand, ExposureBundle, ExposureRequirement, ExternalRef, IsolationTrustRequirement,
    LogicalConnectionRequirement, MaterialisationPlan, ObservationBundle, OutputRequirement,
    PersistenceScope, PlanOmission, PlanStatus, ProjectRuntimeRequirement, ReconciliationResult,
    ReleaseDisposition, ReleaseResult, RequirementNecessity, ResourceRequirement,
    RetentionExpectation, Tiered, WorkcellError, WorkspaceAccess, WorkspaceRequirement,
};
use epilogos_workcell_wire::world_value;
use serde_json::{json, Map, Value};

pub fn demand_value(demand: &ExecutionDemand) -> Value {
    json!({
        "demand_ref": demand.demand_ref.as_str(),
        "subjects": demand.subjects.iter().map(|(key, value)| (key.clone(), Value::String(value.to_string()))).collect::<Map<_, _>>(),
        "affordances": tiered_strings(
            demand.affordances.required.iter().map(|value| value.as_str()),
            demand.affordances.preferred.iter().map(|value| value.as_str()),
            demand.affordances.optional.iter().map(|value| value.as_str()),
        ),
        "workspace": demand.workspace.as_ref().map(|workspace| json!({
            "source": workspace.source.as_ref().map(ToString::to_string),
            "revision": workspace.revision,
            "access": workspace_access(&workspace.access),
        })),
        "project_runtime": demand.project_runtime.as_ref().map(|runtime| runtime.as_str()),
        "resources": demand.resources.iter().map(|resource| json!({
            "key": resource.key,
            "minimum": resource.minimum,
            "unit": resource.unit,
        })).collect::<Vec<_>>(),
        "connectivity": tiered_strings(
            demand.connectivity.required.iter().map(|value| value.as_str()),
            demand.connectivity.preferred.iter().map(|value| value.as_str()),
            demand.connectivity.optional.iter().map(|value| value.as_str()),
        ),
        "exposure": tiered_strings(
            demand.exposure.required.iter().map(|value| value.as_str()),
            demand.exposure.preferred.iter().map(|value| value.as_str()),
            demand.exposure.optional.iter().map(|value| value.as_str()),
        ),
        "outputs": tiered_strings(
            demand.outputs.required.iter().map(|value| value.as_str()),
            demand.outputs.preferred.iter().map(|value| value.as_str()),
            demand.outputs.optional.iter().map(|value| value.as_str()),
        ),
        "persistence": demand.persistence.as_ref().map(persistence),
        "isolation_trust": demand.isolation_trust.as_ref().map(|value| value.as_str()),
        "retention": retention(&demand.retention),
        "extensions": demand.extensions,
    })
}

pub fn decode_demand(value: &Value) -> Result<ExecutionDemand, WorkcellError> {
    let payload = object(value, "ExecutionDemand")?;
    let mut demand = ExecutionDemand::new(
        epilogos_workcell_core::DemandRef::new(string_field(payload, "demand_ref")?)
            .map_err(WorkcellError::from)?,
    );
    demand.subjects = map_field(payload, "subjects")?
        .iter()
        .map(|(key, value)| {
            Ok((
                key.clone(),
                ExternalRef::new(string(value, "subject reference")?)
                    .map_err(WorkcellError::from)?,
            ))
        })
        .collect::<Result<BTreeMap<_, _>, WorkcellError>>()?;
    demand.affordances = decode_tiered(object_field(payload, "affordances")?, |value| {
        AffordanceRequirement::new(value)
    })?;
    demand.workspace = match payload
        .get("workspace")
        .ok_or_else(|| missing("workspace"))?
    {
        Value::Null => None,
        value => {
            let workspace = object(value, "workspace")?;
            Some(WorkspaceRequirement {
                source: optional_string_field(workspace, "source")?
                    .map(|value| ExternalRef::new(value).map_err(WorkcellError::from))
                    .transpose()?,
                revision: optional_string_field(workspace, "revision")?.map(str::to_owned),
                access: parse_workspace_access(string_field(workspace, "access")?)?,
            })
        }
    };
    demand.project_runtime = optional_string_field(payload, "project_runtime")?
        .map(ProjectRuntimeRequirement::new)
        .transpose()?;
    demand.resources = array_field(payload, "resources")?
        .iter()
        .map(|value| {
            let resource = object(value, "resource")?;
            Ok(ResourceRequirement {
                key: string_field(resource, "key")?.to_owned(),
                minimum: optional_u64_field(resource, "minimum")?,
                unit: optional_string_field(resource, "unit")?.map(str::to_owned),
            })
        })
        .collect::<Result<Vec<_>, WorkcellError>>()?;
    demand.connectivity = decode_tiered(object_field(payload, "connectivity")?, |value| {
        LogicalConnectionRequirement::new(value)
    })?;
    demand.exposure = decode_tiered(object_field(payload, "exposure")?, |value| {
        ExposureRequirement::new(value)
    })?;
    demand.outputs = decode_tiered(object_field(payload, "outputs")?, |value| {
        OutputRequirement::new(value)
    })?;
    demand.persistence = optional_string_field(payload, "persistence")?
        .map(parse_persistence)
        .transpose()?;
    demand.isolation_trust = optional_string_field(payload, "isolation_trust")?
        .map(IsolationTrustRequirement::new)
        .transpose()?;
    demand.retention = parse_retention(string_field(payload, "retention")?)?;
    demand.extensions = string_map_field(payload, "extensions")?;
    demand.validate()?;
    Ok(demand)
}

pub fn discovery_value(discovery: &Discovery) -> Value {
    json!({
        "workcell_ref": discovery.workcell_ref.as_str(),
        "health": health(&discovery.health),
        "capacity": discovery.capacity.iter().map(|(key, value)| {
            (key.clone(), json!({"amount": value.amount, "unit": value.unit}))
        }).collect::<Map<_, _>>(),
        "offers": discovery.offers.iter().map(|offer| json!({
            "offer_ref": offer.offer_ref.as_str(),
            "provider_ref": offer.provider_ref.as_str(),
            "port": offer.port,
            "affordances": offer.affordances,
            "connections": offer.connections,
            "exposures": offer.exposures,
            "isolation_trust": offer.isolation_trust,
            "availability": availability(&offer.availability),
            "health": health(&offer.health),
            "capacity": offer.capacity.iter().map(|(key, value)| {
                (key.clone(), json!({"amount": value.amount, "unit": value.unit}))
            }).collect::<Map<_, _>>(),
            "metadata": offer.metadata,
        })).collect::<Vec<_>>(),
    })
}

pub fn status_value(discovery: &Discovery) -> Value {
    let providers = discovery
        .offers
        .iter()
        .map(|offer| offer.provider_ref.as_str())
        .collect::<std::collections::BTreeSet<_>>()
        .len();
    json!({
        "workcell_ref": discovery.workcell_ref.as_str(),
        "health": health(&discovery.health),
        "providers": providers,
        "offers": discovery.offers.len(),
    })
}

pub fn plan_value(plan: &MaterialisationPlan) -> Value {
    json!({
        "plan_ref": plan.plan_ref.as_str(),
        "demand_ref": plan.demand_ref.as_str(),
        "status": plan_status(&plan.status),
        "planned_bindings": plan.planned_bindings.iter().map(|binding| json!({
            "logical_ref": binding.logical_ref,
            "requirement": binding.requirement,
            "necessity": necessity(binding.necessity),
            "provider_ref": binding.provider_ref.as_str(),
            "offer_ref": binding.offer_ref.as_str(),
        })).collect::<Vec<_>>(),
        "planned_exposures": plan.planned_exposures.iter().map(|binding| json!({
            "logical_ref": binding.logical_ref,
            "requirement": binding.requirement,
            "necessity": necessity(binding.necessity),
            "provider_ref": binding.provider_ref.as_str(),
            "offer_ref": binding.offer_ref.as_str(),
        })).collect::<Vec<_>>(),
        "planned_constraints": plan.planned_constraints.iter().map(|binding| json!({
            "logical_ref": binding.logical_ref,
            "requirement": binding.requirement,
            "necessity": necessity(binding.necessity),
            "provider_ref": binding.provider_ref.as_str(),
            "offer_ref": binding.offer_ref.as_str(),
        })).collect::<Vec<_>>(),
        "degradations": plan.degradations.iter().map(degradation_value).collect::<Vec<_>>(),
        "omissions": plan.omissions.iter().map(omission_value).collect::<Vec<_>>(),
        "explanation": plan.explanation,
    })
}

pub fn prepared_world_value(
    world: &epilogos_workcell_core::MaterialisedExecutionWorld,
) -> Result<Value, WorkcellError> {
    world_value(world)
}

pub fn observation_value(bundle: &ObservationBundle) -> Value {
    json!({
        "world_ref": bundle.world_ref.as_str(),
        "observations": bundle.observations.iter().map(|observation| json!({
            "logical_ref": observation.logical_ref,
            "state": health(&observation.state),
            "detail": observation.detail,
        })).collect::<Vec<_>>(),
    })
}

pub fn exposure_value(bundle: &ExposureBundle) -> Value {
    json!({
        "world_ref": bundle.world_ref.as_str(),
        "surfaces": bundle.surfaces.iter().map(|surface| json!({
            "logical_ref": surface.logical_ref,
            "interaction": surface.interaction,
            "material": surface.material,
            "provenance": surface.provenance,
        })).collect::<Vec<_>>(),
        "degradations": bundle.degradations.iter().map(degradation_value).collect::<Vec<_>>(),
        "omissions": bundle.omissions.iter().map(omission_value).collect::<Vec<_>>(),
    })
}

pub fn collection_value(bundle: &CollectionBundle) -> Value {
    json!({
        "world_ref": bundle.world_ref.as_str(),
        "outputs": bundle.outputs.iter().map(|output| json!({
            "logical_ref": output.logical_ref,
            "material_locator": output.material_locator,
            "provenance": output.provenance,
        })).collect::<Vec<_>>(),
        "degradations": bundle.degradations.iter().map(degradation_value).collect::<Vec<_>>(),
        "omissions": bundle.omissions.iter().map(omission_value).collect::<Vec<_>>(),
    })
}

pub fn release_value(result: &ReleaseResult) -> Value {
    json!({
        "world_ref": result.world_ref.as_str(),
        "disposition": match result.disposition {
            ReleaseDisposition::Released => "released",
            ReleaseDisposition::Preserved => "preserved",
            ReleaseDisposition::Suspended => "suspended",
            ReleaseDisposition::Snapshotted => "snapshotted",
        },
        "changed": result.changed,
    })
}

pub fn desired_value(desired: &[DesiredMaterialState]) -> Value {
    json!({
        "desired": desired.iter().map(|item| json!({
            "logical_ref": item.logical_ref,
            "desired": item.desired,
        })).collect::<Vec<_>>()
    })
}

pub fn decode_desired(value: &Value) -> Result<Vec<DesiredMaterialState>, WorkcellError> {
    let payload = object(value, "reconcile payload")?;
    array_field(payload, "desired")?
        .iter()
        .map(|value| {
            let item = object(value, "desired material state")?;
            Ok(DesiredMaterialState {
                logical_ref: string_field(item, "logical_ref")?.to_owned(),
                desired: string_field(item, "desired")?.to_owned(),
            })
        })
        .collect()
}

pub fn reconciliation_value(result: &ReconciliationResult) -> Value {
    json!({
        "deltas": result.deltas.iter().map(|delta| json!({
            "logical_ref": delta.logical_ref,
            "observed": delta.observed,
            "desired": delta.desired,
            "action": delta.action,
        })).collect::<Vec<_>>()
    })
}

pub fn world_ref_value(world_ref: &epilogos_workcell_core::WorldRef) -> Value {
    json!({"world_ref": world_ref.as_str()})
}

pub fn decode_world_ref(value: &Value) -> Result<epilogos_workcell_core::WorldRef, WorkcellError> {
    let object = object(value, "world reference payload")?;
    epilogos_workcell_core::WorldRef::new(string_field(object, "world_ref")?)
        .map_err(WorkcellError::from)
}

fn tiered_strings<'a>(
    required: impl Iterator<Item = &'a str>,
    preferred: impl Iterator<Item = &'a str>,
    optional: impl Iterator<Item = &'a str>,
) -> Value {
    json!({
        "required": required.collect::<Vec<_>>(),
        "preferred": preferred.collect::<Vec<_>>(),
        "optional": optional.collect::<Vec<_>>(),
    })
}

fn decode_tiered<T, F>(
    object: &Map<String, Value>,
    mut parse: F,
) -> Result<Tiered<T>, WorkcellError>
where
    F: FnMut(&str) -> Result<T, WorkcellError>,
{
    Ok(Tiered {
        required: string_array_field(object, "required")?
            .into_iter()
            .map(&mut parse)
            .collect::<Result<Vec<_>, _>>()?,
        preferred: string_array_field(object, "preferred")?
            .into_iter()
            .map(&mut parse)
            .collect::<Result<Vec<_>, _>>()?,
        optional: string_array_field(object, "optional")?
            .into_iter()
            .map(&mut parse)
            .collect::<Result<Vec<_>, _>>()?,
    })
}

fn degradation_value(value: &Degradation) -> Value {
    json!({
        "requirement": value.requirement,
        "necessity": necessity(value.necessity),
        "reason": value.reason,
    })
}

fn omission_value(value: &PlanOmission) -> Value {
    json!({
        "requirement": value.requirement,
        "necessity": necessity(value.necessity),
        "reason": value.reason,
    })
}

fn health(value: &epilogos_workcell_core::HealthState) -> &'static str {
    match value {
        epilogos_workcell_core::HealthState::Healthy => "healthy",
        epilogos_workcell_core::HealthState::Degraded => "degraded",
        epilogos_workcell_core::HealthState::Unavailable => "unavailable",
        epilogos_workcell_core::HealthState::Unknown => "unknown",
    }
}

fn availability(value: &epilogos_workcell_core::Availability) -> &'static str {
    match value {
        epilogos_workcell_core::Availability::Available => "available",
        epilogos_workcell_core::Availability::Degraded => "degraded",
        epilogos_workcell_core::Availability::Unavailable => "unavailable",
    }
}

fn plan_status(value: &PlanStatus) -> &'static str {
    match value {
        PlanStatus::Satisfiable => "satisfiable",
        PlanStatus::Degraded => "degraded",
        PlanStatus::Unsatisfiable => "unsatisfiable",
    }
}

fn necessity(value: RequirementNecessity) -> &'static str {
    match value {
        RequirementNecessity::Required => "required",
        RequirementNecessity::Preferred => "preferred",
        RequirementNecessity::Optional => "optional",
    }
}

fn workspace_access(value: &WorkspaceAccess) -> &'static str {
    match value {
        WorkspaceAccess::ReadOnly => "read-only",
        WorkspaceAccess::Writable => "writable",
    }
}

fn parse_workspace_access(value: &str) -> Result<WorkspaceAccess, WorkcellError> {
    match value {
        "read-only" => Ok(WorkspaceAccess::ReadOnly),
        "writable" => Ok(WorkspaceAccess::Writable),
        other => Err(invalid(format!("unknown workspace access `{other}`"))),
    }
}

fn persistence(value: &PersistenceScope) -> &'static str {
    match value {
        PersistenceScope::Ephemeral => "ephemeral",
        PersistenceScope::TaskOrRun => "task-or-run",
        PersistenceScope::Candidate => "candidate",
        PersistenceScope::Project => "project",
        PersistenceScope::Workcell => "workcell",
        PersistenceScope::Factory => "factory",
        PersistenceScope::External => "external",
    }
}

fn parse_persistence(value: &str) -> Result<PersistenceScope, WorkcellError> {
    match value {
        "ephemeral" => Ok(PersistenceScope::Ephemeral),
        "task-or-run" => Ok(PersistenceScope::TaskOrRun),
        "candidate" => Ok(PersistenceScope::Candidate),
        "project" => Ok(PersistenceScope::Project),
        "workcell" => Ok(PersistenceScope::Workcell),
        "factory" => Ok(PersistenceScope::Factory),
        "external" => Ok(PersistenceScope::External),
        other => Err(invalid(format!("unknown persistence scope `{other}`"))),
    }
}

fn retention(value: &RetentionExpectation) -> &'static str {
    match value {
        RetentionExpectation::Release => "release",
        RetentionExpectation::Preserve => "preserve",
        RetentionExpectation::SuspendIfSupported => "suspend-if-supported",
        RetentionExpectation::SnapshotIfSupported => "snapshot-if-supported",
    }
}

fn parse_retention(value: &str) -> Result<RetentionExpectation, WorkcellError> {
    match value {
        "release" => Ok(RetentionExpectation::Release),
        "preserve" => Ok(RetentionExpectation::Preserve),
        "suspend-if-supported" => Ok(RetentionExpectation::SuspendIfSupported),
        "snapshot-if-supported" => Ok(RetentionExpectation::SnapshotIfSupported),
        other => Err(invalid(format!("unknown retention expectation `{other}`"))),
    }
}

fn object<'a>(value: &'a Value, label: &str) -> Result<&'a Map<String, Value>, WorkcellError> {
    value
        .as_object()
        .ok_or_else(|| invalid(format!("{label} must be an object")))
}

fn object_field<'a>(
    map: &'a Map<String, Value>,
    key: &str,
) -> Result<&'a Map<String, Value>, WorkcellError> {
    object(map.get(key).ok_or_else(|| missing(key))?, key)
}

fn map_field<'a>(
    map: &'a Map<String, Value>,
    key: &str,
) -> Result<&'a Map<String, Value>, WorkcellError> {
    object_field(map, key)
}

fn array_field<'a>(
    map: &'a Map<String, Value>,
    key: &str,
) -> Result<&'a Vec<Value>, WorkcellError> {
    map.get(key)
        .ok_or_else(|| missing(key))?
        .as_array()
        .ok_or_else(|| invalid(format!("field `{key}` must be an array")))
}

fn string_array_field<'a>(
    map: &'a Map<String, Value>,
    key: &str,
) -> Result<Vec<&'a str>, WorkcellError> {
    array_field(map, key)?
        .iter()
        .map(|value| string(value, key))
        .collect()
}

fn string_field<'a>(map: &'a Map<String, Value>, key: &str) -> Result<&'a str, WorkcellError> {
    string(map.get(key).ok_or_else(|| missing(key))?, key)
}

fn optional_string_field<'a>(
    map: &'a Map<String, Value>,
    key: &str,
) -> Result<Option<&'a str>, WorkcellError> {
    match map.get(key).ok_or_else(|| missing(key))? {
        Value::Null => Ok(None),
        value => string(value, key).map(Some),
    }
}

fn optional_u64_field(map: &Map<String, Value>, key: &str) -> Result<Option<u64>, WorkcellError> {
    match map.get(key).ok_or_else(|| missing(key))? {
        Value::Null => Ok(None),
        value => value
            .as_u64()
            .map(Some)
            .ok_or_else(|| invalid(format!("field `{key}` must be an unsigned integer or null"))),
    }
}

fn string<'a>(value: &'a Value, label: &str) -> Result<&'a str, WorkcellError> {
    value
        .as_str()
        .ok_or_else(|| invalid(format!("{label} must be a string")))
}

fn string_map_field(
    map: &Map<String, Value>,
    key: &str,
) -> Result<BTreeMap<String, String>, WorkcellError> {
    map_field(map, key)?
        .iter()
        .map(|(name, value)| Ok((name.clone(), string(value, key)?.to_owned())))
        .collect()
}

fn missing(key: &str) -> WorkcellError {
    invalid(format!("control payload field `{key}` is missing"))
}

fn invalid(message: String) -> WorkcellError {
    WorkcellError::InvalidDemand(message)
}
