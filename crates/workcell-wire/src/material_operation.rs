use epilogos_workcell_core::{
    BindingRef, MaterialOperationGrant, MaterialOperationReading, MaterialOperationRequest,
    MaterialOperationState, MaterialisedExecutionWorld, ProviderOperation, ProviderOperationResult,
    ProviderRef, Result, WorkcellError, WorldRef,
};
use serde_json::{json, Map, Value};
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;

pub fn material_world_revision(world: &MaterialisedExecutionWorld) -> Result<String> {
    let bytes = serde_json::to_vec(&crate::world_value(world)?).map_err(|error| {
        WorkcellError::OperationFailed(format!("encode material revision: {error}"))
    })?;
    Ok(format!(
        "workcell.material-world-sha256/v1:{:x}",
        Sha256::digest(bytes)
    ))
}

pub fn material_operation_request_value(request: &MaterialOperationRequest) -> Value {
    json!({
        "world_ref": request.world_ref.as_str(), "binding_ref": request.binding_ref.as_str(),
        "expected_world_revision": request.expected_world_revision,
        "operation_id": request.operation_id, "grant_ref": request.grant_ref,
        "operation": operation_value(&request.operation), "timeout_ms": request.timeout_ms,
        "output_limit_bytes": request.output_limit_bytes,
    })
}

pub fn material_operation_request_fingerprint(
    request: &MaterialOperationRequest,
) -> Result<String> {
    let bytes =
        serde_json::to_vec(&material_operation_request_value(request)).map_err(|error| {
            WorkcellError::OperationFailed(format!("encode material operation: {error}"))
        })?;
    Ok(format!(
        "workcell.material-operation-sha256/v1:{:x}",
        Sha256::digest(bytes)
    ))
}

pub fn decode_material_operation_request(value: &Value) -> Result<MaterialOperationRequest> {
    let v = object(value)?;
    Ok(MaterialOperationRequest {
        world_ref: WorldRef::new(string(v, "world_ref")?).map_err(WorkcellError::from)?,
        binding_ref: BindingRef::new(string(v, "binding_ref")?).map_err(WorkcellError::from)?,
        expected_world_revision: string(v, "expected_world_revision")?.to_owned(),
        operation_id: string(v, "operation_id")?.to_owned(),
        grant_ref: string(v, "grant_ref")?.to_owned(),
        operation: decode_operation(field(v, "operation")?)?,
        timeout_ms: positive(v, "timeout_ms")?,
        output_limit_bytes: positive(v, "output_limit_bytes")?,
    })
}

pub fn material_operation_grant_value(grant: &MaterialOperationGrant) -> Value {
    json!({
        "grant_ref": grant.grant_ref, "authority_ref": grant.authority_ref,
        "world_ref": grant.world_ref.as_str(), "binding_ref": grant.binding_ref.as_str(),
        "expected_world_revision": grant.expected_world_revision,
        "provider_ref": grant.provider_ref.as_str(), "material_ref": grant.material_ref,
        "operation_id": grant.operation_id, "operation": operation_value(&grant.operation),
        "timeout_ms": grant.timeout_ms, "output_limit_bytes": grant.output_limit_bytes,
        "expires_at_unix_ms": grant.expires_at_unix_ms,
    })
}

pub fn decode_material_operation_grant(value: &Value) -> Result<MaterialOperationGrant> {
    let v = object(value)?;
    Ok(MaterialOperationGrant {
        grant_ref: string(v, "grant_ref")?.to_owned(),
        authority_ref: string(v, "authority_ref")?.to_owned(),
        world_ref: WorldRef::new(string(v, "world_ref")?).map_err(WorkcellError::from)?,
        binding_ref: BindingRef::new(string(v, "binding_ref")?).map_err(WorkcellError::from)?,
        expected_world_revision: string(v, "expected_world_revision")?.to_owned(),
        provider_ref: ProviderRef::new(string(v, "provider_ref")?).map_err(WorkcellError::from)?,
        material_ref: string(v, "material_ref")?.to_owned(),
        operation_id: string(v, "operation_id")?.to_owned(),
        operation: decode_operation(field(v, "operation")?)?,
        timeout_ms: positive(v, "timeout_ms")?,
        output_limit_bytes: positive(v, "output_limit_bytes")?,
        expires_at_unix_ms: positive(v, "expires_at_unix_ms")?,
    })
}

pub fn material_operation_reading_value(reading: &MaterialOperationReading) -> Value {
    json!({
        "world_ref": reading.world_ref.as_str(), "binding_ref": reading.binding_ref.as_str(),
        "operation_id": reading.operation_id, "request_fingerprint": reading.request_fingerprint,
        "state": match reading.state { MaterialOperationState::Pending => "pending",
            MaterialOperationState::Completed => "completed", MaterialOperationState::Uncertain => "uncertain" },
        "grant_ref": reading.grant_ref, "authority_ref": reading.authority_ref,
        "provider_ref": reading.provider_ref.as_str(), "material_ref": reading.material_ref,
        "result": reading.result.as_ref().map(|result| json!({
            "provider_ref": result.provider_ref.as_str(), "material_ref": result.material_ref,
            "operation": result.operation, "output": result.output, "provenance": result.provenance,
        })), "failure_kind": reading.failure_kind, "failure_observation": reading.failure_observation,
    })
}

pub fn decode_material_operation_reading(value: &Value) -> Result<MaterialOperationReading> {
    let v = object(value)?;
    let state = match string(v, "state")? {
        "pending" => MaterialOperationState::Pending,
        "completed" => MaterialOperationState::Completed,
        "uncertain" => MaterialOperationState::Uncertain,
        _ => return Err(invalid("unknown material operation state")),
    };
    let result = match field(v, "result")? {
        Value::Null => None,
        value => {
            let r = object(value)?;
            Some(ProviderOperationResult {
                provider_ref: ProviderRef::new(string(r, "provider_ref")?)
                    .map_err(WorkcellError::from)?,
                material_ref: string(r, "material_ref")?.to_owned(),
                operation: string(r, "operation")?.to_owned(),
                output: string_map(field(r, "output")?)?,
                provenance: string_map(field(r, "provenance")?)?,
            })
        }
    };
    if (state == MaterialOperationState::Completed) != result.is_some() {
        return Err(invalid(
            "only a completed material operation carries a result",
        ));
    }
    let failure_kind = match field(v, "failure_kind")? {
        Value::Null => None,
        Value::String(s) if !s.trim().is_empty() => Some(s.clone()),
        _ => return Err(invalid("invalid material operation failure kind")),
    };
    let reading = MaterialOperationReading {
        world_ref: WorldRef::new(string(v, "world_ref")?).map_err(WorkcellError::from)?,
        binding_ref: BindingRef::new(string(v, "binding_ref")?).map_err(WorkcellError::from)?,
        operation_id: string(v, "operation_id")?.to_owned(),
        request_fingerprint: string(v, "request_fingerprint")?.to_owned(),
        state,
        grant_ref: string(v, "grant_ref")?.to_owned(),
        authority_ref: string(v, "authority_ref")?.to_owned(),
        provider_ref: ProviderRef::new(string(v, "provider_ref")?).map_err(WorkcellError::from)?,
        material_ref: string(v, "material_ref")?.to_owned(),
        result,
        failure_kind,
        failure_observation: string_map(field(v, "failure_observation")?)?,
    };
    if reading.result.as_ref().is_some_and(|r| {
        r.provider_ref != reading.provider_ref || r.material_ref != reading.material_ref
    }) {
        return Err(invalid(
            "material result does not belong to its retained allocation",
        ));
    }
    Ok(reading)
}

fn operation_value(operation: &ProviderOperation) -> Value {
    json!({"key": operation.key, "parameters": operation.parameters})
}
fn decode_operation(value: &Value) -> Result<ProviderOperation> {
    let v = object(value)?;
    Ok(ProviderOperation {
        key: string(v, "key")?.to_owned(),
        parameters: string_map(field(v, "parameters")?)?,
    })
}
fn invalid(message: &str) -> WorkcellError {
    WorkcellError::InvalidDemand(message.to_owned())
}
fn object(value: &Value) -> Result<&Map<String, Value>> {
    value
        .as_object()
        .ok_or_else(|| invalid("material operation must be an object"))
}
fn field<'a>(v: &'a Map<String, Value>, name: &str) -> Result<&'a Value> {
    v.get(name)
        .ok_or_else(|| invalid(&format!("missing material operation field {name}")))
}
fn string<'a>(v: &'a Map<String, Value>, name: &str) -> Result<&'a str> {
    field(v, name)?
        .as_str()
        .filter(|s| !s.trim().is_empty())
        .ok_or_else(|| {
            invalid(&format!(
                "material operation field {name} must be a nonempty string"
            ))
        })
}
fn positive(v: &Map<String, Value>, name: &str) -> Result<u64> {
    field(v, name)?.as_u64().filter(|n| *n > 0).ok_or_else(|| {
        invalid(&format!(
            "material operation field {name} must be a positive integer"
        ))
    })
}
fn string_map(value: &Value) -> Result<BTreeMap<String, String>> {
    object(value)?
        .iter()
        .map(|(k, v)| {
            Ok((
                k.clone(),
                v.as_str()
                    .ok_or_else(|| invalid("material operation map values must be strings"))?
                    .to_owned(),
            ))
        })
        .collect()
}
