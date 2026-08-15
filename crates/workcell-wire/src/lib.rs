use std::collections::BTreeMap;

use epilogos_workcell_core::{
    Binding, BindingGraph, BindingPresence, BindingRef, BindingRelation, Degradation, DemandRef,
    ExternalRef, HealthState, MaterialisedExecutionWorld, OfferRef, PersistenceScope, PlanOmission,
    PlannedConstraint, PlannedExposure, ProviderPortKind, ProviderRef, RequirementNecessity,
    Result, RetentionExpectation, WorkcellError, WorkcellRef, WorldRef,
};
use serde_json::{json, Map, Value};

pub const MATERIAL_WORLD_WIRE_VERSION: &str = "workcell.material-world/v1";

pub fn encode_world(world: &MaterialisedExecutionWorld) -> Result<String> {
    serde_json::to_string_pretty(&world_value(world)?)
        .map_err(|error| WorkcellError::OperationFailed(format!("encode material world: {error}")))
}

pub fn decode_world(input: &str) -> Result<MaterialisedExecutionWorld> {
    let value: Value = serde_json::from_str(input)
        .map_err(|error| WorkcellError::InvalidDemand(format!("decode material world: {error}")))?;
    decode_world_value(&value)
}

pub fn world_value(world: &MaterialisedExecutionWorld) -> Result<Value> {
    let bindings = world
        .binding_graph
        .bindings
        .iter()
        .map(binding_value)
        .collect::<Result<Vec<_>>>()?;
    let relations = world
        .binding_graph
        .relations
        .iter()
        .map(|relation| {
            json!({
                "from": relation.from.as_str(),
                "to": relation.to.as_str(),
                "relation": relation.relation,
            })
        })
        .collect::<Vec<_>>();
    let exposures = world
        .planned_exposures
        .iter()
        .map(planned_exposure_value)
        .collect::<Vec<_>>();
    let constraints = world
        .planned_constraints
        .iter()
        .map(planned_constraint_value)
        .collect::<Vec<_>>();
    let degradations = world
        .plan_degradations
        .iter()
        .map(degradation_value)
        .collect::<Vec<_>>();
    let omissions = world
        .plan_omissions
        .iter()
        .map(omission_value)
        .collect::<Vec<_>>();

    Ok(json!({
        "version": MATERIAL_WORLD_WIRE_VERSION,
        "world_ref": world.world_ref.as_str(),
        "workcell_ref": world.workcell_ref.as_str(),
        "demand_ref": world.demand_ref.as_str(),
        "subjects": world.subjects.iter().map(|(key, value)| (key.clone(), Value::String(value.to_string()))).collect::<Map<_, _>>(),
        "binding_graph": {
            "bindings": bindings,
            "relations": relations,
        },
        "planned_exposures": exposures,
        "planned_constraints": constraints,
        "plan_degradations": degradations,
        "plan_omissions": omissions,
        "persistence": world.persistence.as_ref().map(persistence_str),
        "retention": retention_str(&world.retention),
        "state": health_str(&world.state),
        "provenance": world.provenance,
    }))
}

fn decode_world_value(value: &Value) -> Result<MaterialisedExecutionWorld> {
    let object = object(value, "material world")?;
    let version = string_field(object, "version")?;
    if version != MATERIAL_WORLD_WIRE_VERSION {
        return Err(WorkcellError::Unsupported(format!(
            "material world wire version `{version}` is not supported"
        )));
    }

    let subjects = map_field(object, "subjects")?
        .iter()
        .map(|(key, value)| {
            Ok((
                key.clone(),
                ExternalRef::new(string(value, "subject ref")?)
                    .map_err(WorkcellError::from)?,
            ))
        })
        .collect::<Result<BTreeMap<_, _>>>()?;

    let graph = object_field(object, "binding_graph")?;
    let bindings = array_field(graph, "bindings")?
        .iter()
        .map(decode_binding)
        .collect::<Result<Vec<_>>>()?;
    let relations = array_field(graph, "relations")?
        .iter()
        .map(|value| {
            let relation = object(value, "binding relation")?;
            Ok(BindingRelation {
                from: BindingRef::new(string_field(relation, "from")?)
                    .map_err(WorkcellError::from)?,
                to: BindingRef::new(string_field(relation, "to")?)
                    .map_err(WorkcellError::from)?,
                relation: string_field(relation, "relation")?.to_owned(),
            })
        })
        .collect::<Result<Vec<_>>>()?;

    let planned_exposures = array_field(object, "planned_exposures")?
        .iter()
        .map(decode_planned_exposure)
        .collect::<Result<Vec<_>>>()?;
    let planned_constraints = array_field(object, "planned_constraints")?
        .iter()
        .map(decode_planned_constraint)
        .collect::<Result<Vec<_>>>()?;
    let plan_degradations = array_field(object, "plan_degradations")?
        .iter()
        .map(decode_degradation)
        .collect::<Result<Vec<_>>>()?;
    let plan_omissions = array_field(object, "plan_omissions")?
        .iter()
        .map(decode_omission)
        .collect::<Result<Vec<_>>>()?;

    Ok(MaterialisedExecutionWorld {
        world_ref: WorldRef::new(string_field(object, "world_ref")?).map_err(WorkcellError::from)?,
        workcell_ref: WorkcellRef::new(string_field(object, "workcell_ref")?)
            .map_err(WorkcellError::from)?,
        demand_ref: DemandRef::new(string_field(object, "demand_ref")?)
            .map_err(WorkcellError::from)?,
        subjects,
        binding_graph: BindingGraph {
            bindings,
            relations,
        },
        planned_exposures,
        planned_constraints,
        plan_degradations,
        plan_omissions,
        persistence: optional_string_field(object, "persistence")?
            .map(parse_persistence)
            .transpose()?,
        retention: parse_retention(string_field(object, "retention")?)?,
        state: parse_health(string_field(object, "state")?)?,
        provenance: string_map_field(object, "provenance")?,
    })
}

fn binding_value(binding: &Binding) -> Result<Value> {
    Ok(json!({
        "binding_ref": binding.binding_ref.as_str(),
        "logical_ref": binding.logical_ref,
        "necessity": necessity_str(binding.necessity),
        "provider_ref": binding.provider_ref.as_str(),
        "offer_ref": binding.offer_ref.as_str(),
        "port": port_str(binding.port)?,
        "material_ref": binding.material_ref,
        "health": health_str(&binding.health),
        "presence": presence_str(&binding.presence),
        "properties": binding.properties,
        "provenance": binding.provenance,
    }))
}

fn decode_binding(value: &Value) -> Result<Binding> {
    let binding = object(value, "binding")?;
    Ok(Binding {
        binding_ref: BindingRef::new(string_field(binding, "binding_ref")?)
            .map_err(WorkcellError::from)?,
        logical_ref: string_field(binding, "logical_ref")?.to_owned(),
        necessity: parse_necessity(string_field(binding, "necessity")?)?,
        provider_ref: ProviderRef::new(string_field(binding, "provider_ref")?)
            .map_err(WorkcellError::from)?,
        offer_ref: OfferRef::new(string_field(binding, "offer_ref")?)
            .map_err(WorkcellError::from)?,
        port: parse_port(string_field(binding, "port")?)?,
        material_ref: string_field(binding, "material_ref")?.to_owned(),
        health: parse_health(string_field(binding, "health")?)?,
        presence: parse_presence(string_field(binding, "presence")?)?,
        properties: string_map_field(binding, "properties")?,
        provenance: string_map_field(binding, "provenance")?,
    })
}

fn planned_exposure_value(value: &PlannedExposure) -> Value {
    json!({
        "logical_ref": value.logical_ref,
        "requirement": value.requirement,
        "necessity": necessity_str(value.necessity),
        "provider_ref": value.provider_ref.as_str(),
        "offer_ref": value.offer_ref.as_str(),
    })
}

fn decode_planned_exposure(value: &Value) -> Result<PlannedExposure> {
    let value = object(value, "planned exposure")?;
    Ok(PlannedExposure {
        logical_ref: string_field(value, "logical_ref")?.to_owned(),
        requirement: string_field(value, "requirement")?.to_owned(),
        necessity: parse_necessity(string_field(value, "necessity")?)?,
        provider_ref: ProviderRef::new(string_field(value, "provider_ref")?)
            .map_err(WorkcellError::from)?,
        offer_ref: OfferRef::new(string_field(value, "offer_ref")?)
            .map_err(WorkcellError::from)?,
    })
}

fn planned_constraint_value(value: &PlannedConstraint) -> Value {
    json!({
        "logical_ref": value.logical_ref,
        "requirement": value.requirement,
        "necessity": necessity_str(value.necessity),
        "provider_ref": value.provider_ref.as_str(),
        "offer_ref": value.offer_ref.as_str(),
    })
}

fn decode_planned_constraint(value: &Value) -> Result<PlannedConstraint> {
    let value = object(value, "planned constraint")?;
    Ok(PlannedConstraint {
        logical_ref: string_field(value, "logical_ref")?.to_owned(),
        requirement: string_field(value, "requirement")?.to_owned(),
        necessity: parse_necessity(string_field(value, "necessity")?)?,
        provider_ref: ProviderRef::new(string_field(value, "provider_ref")?)
            .map_err(WorkcellError::from)?,
        offer_ref: OfferRef::new(string_field(value, "offer_ref")?)
            .map_err(WorkcellError::from)?,
    })
}

fn degradation_value(value: &Degradation) -> Value {
    json!({
        "requirement": value.requirement,
        "necessity": necessity_str(value.necessity),
        "reason": value.reason,
    })
}

fn decode_degradation(value: &Value) -> Result<Degradation> {
    let value = object(value, "degradation")?;
    Ok(Degradation {
        requirement: string_field(value, "requirement")?.to_owned(),
        necessity: parse_necessity(string_field(value, "necessity")?)?,
        reason: string_field(value, "reason")?.to_owned(),
    })
}

fn omission_value(value: &PlanOmission) -> Value {
    json!({
        "requirement": value.requirement,
        "necessity": necessity_str(value.necessity),
        "reason": value.reason,
    })
}

fn decode_omission(value: &Value) -> Result<PlanOmission> {
    let value = object(value, "plan omission")?;
    Ok(PlanOmission {
        requirement: string_field(value, "requirement")?.to_owned(),
        necessity: parse_necessity(string_field(value, "necessity")?)?,
        reason: string_field(value, "reason")?.to_owned(),
    })
}

fn necessity_str(value: RequirementNecessity) -> &'static str {
    match value {
        RequirementNecessity::Required => "required",
        RequirementNecessity::Preferred => "preferred",
        RequirementNecessity::Optional => "optional",
    }
}

fn parse_necessity(value: &str) -> Result<RequirementNecessity> {
    match value {
        "required" => Ok(RequirementNecessity::Required),
        "preferred" => Ok(RequirementNecessity::Preferred),
        "optional" => Ok(RequirementNecessity::Optional),
        other => Err(WorkcellError::InvalidDemand(format!(
            "unknown requirement necessity `{other}`"
        ))),
    }
}

fn health_str(value: &HealthState) -> &'static str {
    match value {
        HealthState::Healthy => "healthy",
        HealthState::Degraded => "degraded",
        HealthState::Unavailable => "unavailable",
        HealthState::Unknown => "unknown",
    }
}

fn parse_health(value: &str) -> Result<HealthState> {
    match value {
        "healthy" => Ok(HealthState::Healthy),
        "degraded" => Ok(HealthState::Degraded),
        "unavailable" => Ok(HealthState::Unavailable),
        "unknown" => Ok(HealthState::Unknown),
        other => Err(WorkcellError::InvalidDemand(format!(
            "unknown health state `{other}`"
        ))),
    }
}

fn presence_str(value: &BindingPresence) -> &'static str {
    match value {
        BindingPresence::Present => "present",
        BindingPresence::Missing => "missing",
        BindingPresence::Released => "released",
        BindingPresence::Suspended => "suspended",
        BindingPresence::Snapshotted => "snapshotted",
        BindingPresence::Stale => "stale",
    }
}

fn parse_presence(value: &str) -> Result<BindingPresence> {
    match value {
        "present" => Ok(BindingPresence::Present),
        "missing" => Ok(BindingPresence::Missing),
        "released" => Ok(BindingPresence::Released),
        "suspended" => Ok(BindingPresence::Suspended),
        "snapshotted" => Ok(BindingPresence::Snapshotted),
        "stale" => Ok(BindingPresence::Stale),
        other => Err(WorkcellError::InvalidDemand(format!(
            "unknown binding presence `{other}`"
        ))),
    }
}

fn port_str(value: ProviderPortKind) -> Result<&'static str> {
    match value {
        ProviderPortKind::Workspace => Ok("workspace"),
        ProviderPortKind::Execution => Ok("execution"),
        ProviderPortKind::ProjectRuntime => Ok("project-runtime"),
        ProviderPortKind::Service => Ok("service"),
        ProviderPortKind::ArtifactStorage => Ok("artifact-storage"),
        _ => Err(WorkcellError::Unsupported(
            "material world contains a provider port unknown to this wire version".into(),
        )),
    }
}

fn parse_port(value: &str) -> Result<ProviderPortKind> {
    match value {
        "workspace" => Ok(ProviderPortKind::Workspace),
        "execution" => Ok(ProviderPortKind::Execution),
        "project-runtime" => Ok(ProviderPortKind::ProjectRuntime),
        "service" => Ok(ProviderPortKind::Service),
        "artifact-storage" => Ok(ProviderPortKind::ArtifactStorage),
        other => Err(WorkcellError::Unsupported(format!(
            "provider port `{other}` is not supported by this material-world wire version"
        ))),
    }
}

fn persistence_str(value: &PersistenceScope) -> &'static str {
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

fn parse_persistence(value: &str) -> Result<PersistenceScope> {
    match value {
        "ephemeral" => Ok(PersistenceScope::Ephemeral),
        "task-or-run" => Ok(PersistenceScope::TaskOrRun),
        "candidate" => Ok(PersistenceScope::Candidate),
        "project" => Ok(PersistenceScope::Project),
        "workcell" => Ok(PersistenceScope::Workcell),
        "factory" => Ok(PersistenceScope::Factory),
        "external" => Ok(PersistenceScope::External),
        other => Err(WorkcellError::InvalidDemand(format!(
            "unknown persistence scope `{other}`"
        ))),
    }
}

fn retention_str(value: &RetentionExpectation) -> &'static str {
    match value {
        RetentionExpectation::Release => "release",
        RetentionExpectation::Preserve => "preserve",
        RetentionExpectation::SuspendIfSupported => "suspend-if-supported",
        RetentionExpectation::SnapshotIfSupported => "snapshot-if-supported",
    }
}

fn parse_retention(value: &str) -> Result<RetentionExpectation> {
    match value {
        "release" => Ok(RetentionExpectation::Release),
        "preserve" => Ok(RetentionExpectation::Preserve),
        "suspend-if-supported" => Ok(RetentionExpectation::SuspendIfSupported),
        "snapshot-if-supported" => Ok(RetentionExpectation::SnapshotIfSupported),
        other => Err(WorkcellError::InvalidDemand(format!(
            "unknown retention expectation `{other}`"
        ))),
    }
}

fn object<'a>(value: &'a Value, label: &str) -> Result<&'a Map<String, Value>> {
    value.as_object().ok_or_else(|| {
        WorkcellError::InvalidDemand(format!("{label} must be a JSON object"))
    })
}

fn object_field<'a>(object: &'a Map<String, Value>, key: &str) -> Result<&'a Map<String, Value>> {
    object(object.get(key).ok_or_else(|| missing(key))?, key)
}

fn map_field<'a>(object: &'a Map<String, Value>, key: &str) -> Result<&'a Map<String, Value>> {
    object_field(object, key)
}

fn array_field<'a>(object: &'a Map<String, Value>, key: &str) -> Result<&'a Vec<Value>> {
    object
        .get(key)
        .ok_or_else(|| missing(key))?
        .as_array()
        .ok_or_else(|| WorkcellError::InvalidDemand(format!("field `{key}` must be an array")))
}

fn string_field<'a>(object: &'a Map<String, Value>, key: &str) -> Result<&'a str> {
    string(object.get(key).ok_or_else(|| missing(key))?, key)
}

fn optional_string_field<'a>(object: &'a Map<String, Value>, key: &str) -> Result<Option<&'a str>> {
    match object.get(key).ok_or_else(|| missing(key))? {
        Value::Null => Ok(None),
        value => string(value, key).map(Some),
    }
}

fn string<'a>(value: &'a Value, label: &str) -> Result<&'a str> {
    value.as_str().ok_or_else(|| {
        WorkcellError::InvalidDemand(format!("{label} must be a JSON string"))
    })
}

fn string_map_field(object: &Map<String, Value>, key: &str) -> Result<BTreeMap<String, String>> {
    map_field(object, key)?
        .iter()
        .map(|(name, value)| Ok((name.clone(), string(value, key)?.to_owned())))
        .collect()
}

fn missing(key: &str) -> WorkcellError {
    WorkcellError::InvalidDemand(format!("material-world field `{key}` is missing"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use epilogos_workcell_core::{BindingGraph, DemandRef, RetentionExpectation};

    #[test]
    fn empty_material_world_round_trips_without_semantic_translation() {
        let mut subjects = BTreeMap::new();
        subjects.insert(
            "caller-role".into(),
            ExternalRef::new("caller-owned:anything").unwrap(),
        );
        let world = MaterialisedExecutionWorld {
            world_ref: WorldRef::new("world:wire").unwrap(),
            workcell_ref: WorkcellRef::new("workcell:wire").unwrap(),
            demand_ref: DemandRef::new("demand:wire").unwrap(),
            subjects,
            binding_graph: BindingGraph::default(),
            planned_exposures: vec![],
            planned_constraints: vec![],
            plan_degradations: vec![],
            plan_omissions: vec![],
            persistence: None,
            retention: RetentionExpectation::Release,
            state: HealthState::Healthy,
            provenance: BTreeMap::new(),
        };

        let encoded = encode_world(&world).unwrap();
        assert_eq!(decode_world(&encoded).unwrap(), world);
    }

    #[test]
    fn incompatible_material_world_version_fails_explicitly() {
        let input = r#"{
          "version": "workcell.material-world/v99",
          "world_ref": "world:x",
          "workcell_ref": "workcell:x",
          "demand_ref": "demand:x",
          "subjects": {},
          "binding_graph": {"bindings": [], "relations": []},
          "planned_exposures": [],
          "planned_constraints": [],
          "plan_degradations": [],
          "plan_omissions": [],
          "persistence": null,
          "retention": "release",
          "state": "healthy",
          "provenance": {}
        }"#;
        assert!(matches!(
            decode_world(input),
            Err(WorkcellError::Unsupported(_))
        ));
    }
}
