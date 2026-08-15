use epilogos_workcell_core::{WorkcellControlPlane, WorkcellError};
use serde_json::{json, Map, Value};

use crate::{codec, CONTROL_PROTOCOL_VERSION};

pub struct ControlService<C> {
    control: C,
    authorization: Option<String>,
}

impl<C> ControlService<C>
where
    C: WorkcellControlPlane,
{
    pub fn new(control: C) -> Self {
        Self {
            control,
            authorization: None,
        }
    }

    pub fn with_authorization(mut self, authorization: impl Into<String>) -> Self {
        self.authorization = Some(authorization.into());
        self
    }

    pub fn control(&self) -> &C {
        &self.control
    }

    pub fn control_mut(&mut self) -> &mut C {
        &mut self.control
    }

    pub fn into_inner(self) -> C {
        self.control
    }

    pub fn handle_bytes(&mut self, request: &[u8]) -> Vec<u8> {
        let response = self.handle_request(request);
        serde_json::to_vec(&response).unwrap_or_else(|error| {
            format!(
                "{{\"version\":\"{CONTROL_PROTOCOL_VERSION}\",\"request_id\":null,\"ok\":false,\"error\":{{\"kind\":\"service-encoding-failed\",\"message\":\"{}\"}}}}",
                escape_json(&error.to_string())
            )
            .into_bytes()
        })
    }

    fn handle_request(&mut self, request: &[u8]) -> Value {
        let value: Value = match serde_json::from_slice(request) {
            Ok(value) => value,
            Err(error) => {
                return error_response(
                    Value::Null,
                    "invalid-request",
                    format!("control request is not valid JSON: {error}"),
                )
            }
        };
        let envelope = match value.as_object() {
            Some(value) => value,
            None => {
                return error_response(
                    Value::Null,
                    "invalid-request",
                    "control request must be a JSON object".into(),
                )
            }
        };
        let request_id = envelope.get("request_id").cloned().unwrap_or(Value::Null);
        let version = match required_string(envelope, "version") {
            Ok(value) => value,
            Err(message) => return error_response(request_id, "invalid-request", message),
        };
        if version != CONTROL_PROTOCOL_VERSION {
            return error_response(
                request_id,
                "protocol-incompatible",
                format!(
                    "control protocol `{version}` is incompatible with `{CONTROL_PROTOCOL_VERSION}`"
                ),
            );
        }
        if let Some(required) = &self.authorization {
            let supplied = envelope.get("authorization").and_then(Value::as_str);
            if supplied != Some(required.as_str()) {
                return error_response(
                    request_id,
                    "authentication-failed",
                    "control-service authentication failed".into(),
                );
            }
        }
        let operation = match required_string(envelope, "operation") {
            Ok(value) => value,
            Err(message) => return error_response(request_id, "invalid-request", message),
        };
        let payload = envelope.get("payload").unwrap_or(&Value::Null);

        match self.dispatch(operation, payload) {
            Ok(payload) => json!({
                "version": CONTROL_PROTOCOL_VERSION,
                "request_id": request_id,
                "ok": true,
                "payload": payload,
            }),
            Err(error) => {
                let (kind, message) = workcell_error_parts(&error);
                error_response(request_id, kind, message)
            }
        }
    }

    fn dispatch(&mut self, operation: &str, payload: &Value) -> Result<Value, WorkcellError> {
        match operation {
            "status" => self
                .control
                .discover()
                .map(|value| codec::status_value(&value)),
            "discover" => self
                .control
                .discover()
                .map(|value| codec::discovery_value(&value)),
            "plan" => {
                let demand = codec::decode_demand(payload)?;
                self.control
                    .plan(&demand)
                    .map(|value| codec::plan_value(&value))
            }
            "prepare" => {
                let demand = codec::decode_demand(payload)?;
                let world = self.control.prepare(&demand)?;
                codec::prepared_world_value(&world)
            }
            "observe" => {
                let world_ref = codec::decode_world_ref(payload)?;
                self.control
                    .observe(&world_ref)
                    .map(|value| codec::observation_value(&value))
            }
            "expose" => {
                let world_ref = codec::decode_world_ref(payload)?;
                self.control
                    .expose(&world_ref)
                    .map(|value| codec::exposure_value(&value))
            }
            "collect" => {
                let world_ref = codec::decode_world_ref(payload)?;
                self.control
                    .collect(&world_ref)
                    .map(|value| codec::collection_value(&value))
            }
            "release" => {
                let world_ref = codec::decode_world_ref(payload)?;
                self.control
                    .release(&world_ref)
                    .map(|value| codec::release_value(&value))
            }
            "reconcile" => {
                let desired = codec::decode_desired(payload)?;
                self.control
                    .reconcile(&desired)
                    .map(|value| codec::reconciliation_value(&value))
            }
            other => Err(WorkcellError::Unsupported(format!(
                "control operation `{other}` is not supported"
            ))),
        }
    }
}

fn required_string<'a>(map: &'a Map<String, Value>, key: &str) -> Result<&'a str, String> {
    map.get(key)
        .and_then(Value::as_str)
        .ok_or_else(|| format!("control request field `{key}` must be a string"))
}

fn error_response(request_id: Value, kind: &str, message: String) -> Value {
    json!({
        "version": CONTROL_PROTOCOL_VERSION,
        "request_id": request_id,
        "ok": false,
        "error": {
            "kind": kind,
            "message": message,
        }
    })
}

fn workcell_error_parts(error: &WorkcellError) -> (&'static str, String) {
    match error {
        WorkcellError::InvalidDemand(message) => ("invalid-demand", message.clone()),
        WorkcellError::UnsatisfiedDemand(message) => ("unsatisfied-demand", message.clone()),
        WorkcellError::Unavailable(message) => ("unavailable", message.clone()),
        WorkcellError::Degraded(message) => ("degraded", message.clone()),
        WorkcellError::OperationFailed(message) => ("operation-failed", message.clone()),
        WorkcellError::CleanupFailed(message) => ("cleanup-failed", message.clone()),
        WorkcellError::ReconciliationFailed(message) => {
            ("reconciliation-failed", message.clone())
        }
        WorkcellError::NotFound(message) => ("not-found", message.clone()),
        WorkcellError::Unsupported(message) => ("unsupported", message.clone()),
    }
}

fn escape_json(value: &str) -> String {
    value.replace('\\', "\\\\").replace('"', "\\\"")
}
