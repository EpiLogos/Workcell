use std::{error::Error, fmt};

use epilogos_workcell_core::{DesiredMaterialState, ExecutionDemand, WorkcellError, WorldRef};
use serde_json::{json, Value};

use crate::{codec, ControlService, CONTROL_PROTOCOL_VERSION};

pub trait ControlTransport {
    fn round_trip(&mut self, request: &[u8]) -> Result<Vec<u8>, TransportFailure>;
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TransportFailure {
    pub message: String,
}

impl TransportFailure {
    pub fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
        }
    }
}

impl fmt::Display for TransportFailure {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.message)
    }
}

impl Error for TransportFailure {}

pub struct DirectTransport<'a, C> {
    service: &'a mut ControlService<C>,
}

impl<'a, C> DirectTransport<'a, C> {
    pub fn new(service: &'a mut ControlService<C>) -> Self {
        Self { service }
    }
}

impl<C> ControlTransport for DirectTransport<'_, C>
where
    C: epilogos_workcell_core::WorkcellControlPlane,
{
    fn round_trip(&mut self, request: &[u8]) -> Result<Vec<u8>, TransportFailure> {
        Ok(self.service.handle_bytes(request))
    }
}

/// Deterministic alternate byte-path fixture. The Workcell control envelope is
/// unchanged; only the path framing differs.
pub struct LengthPrefixedTransport<'a, C> {
    service: &'a mut ControlService<C>,
}

impl<'a, C> LengthPrefixedTransport<'a, C> {
    pub fn new(service: &'a mut ControlService<C>) -> Self {
        Self { service }
    }
}

impl<C> ControlTransport for LengthPrefixedTransport<'_, C>
where
    C: epilogos_workcell_core::WorkcellControlPlane,
{
    fn round_trip(&mut self, request: &[u8]) -> Result<Vec<u8>, TransportFailure> {
        let framed = frame(request)?;
        let server_request = unframe(&framed)?;
        let response = self.service.handle_bytes(server_request);
        let framed_response = frame(&response)?;
        Ok(unframe(&framed_response)?.to_vec())
    }
}

#[derive(Default)]
pub struct UnavailableTransport;

impl ControlTransport for UnavailableTransport {
    fn round_trip(&mut self, _request: &[u8]) -> Result<Vec<u8>, TransportFailure> {
        Err(TransportFailure::new("control transport is unavailable"))
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ControlClientError {
    TransportUnavailable(String),
    ProtocolIncompatible(String),
    AuthenticationFailed(String),
    Remote(WorkcellError),
    InvalidResponse(String),
}

impl fmt::Display for ControlClientError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::TransportUnavailable(message) => {
                write!(formatter, "transport unavailable: {message}")
            }
            Self::ProtocolIncompatible(message) => {
                write!(formatter, "protocol incompatible: {message}")
            }
            Self::AuthenticationFailed(message) => {
                write!(formatter, "authentication failed: {message}")
            }
            Self::Remote(error) => write!(formatter, "remote Workcell error: {error}"),
            Self::InvalidResponse(message) => {
                write!(formatter, "invalid control response: {message}")
            }
        }
    }
}

impl Error for ControlClientError {}

pub struct ControlClient<T> {
    transport: T,
    authorization: Option<String>,
    protocol_version: String,
    next_request_id: u64,
}

impl<T> ControlClient<T>
where
    T: ControlTransport,
{
    pub fn new(transport: T) -> Self {
        Self {
            transport,
            authorization: None,
            protocol_version: CONTROL_PROTOCOL_VERSION.into(),
            next_request_id: 1,
        }
    }

    pub fn with_authorization(mut self, authorization: impl Into<String>) -> Self {
        self.authorization = Some(authorization.into());
        self
    }

    pub fn with_protocol_version(mut self, version: impl Into<String>) -> Self {
        self.protocol_version = version.into();
        self
    }

    pub fn into_transport(self) -> T {
        self.transport
    }

    pub fn status(&mut self) -> Result<Value, ControlClientError> {
        self.invoke("status", Value::Null)
    }

    pub fn discover(&mut self) -> Result<Value, ControlClientError> {
        self.invoke("discover", Value::Null)
    }

    pub fn plan(&mut self, demand: &ExecutionDemand) -> Result<Value, ControlClientError> {
        self.invoke("plan", codec::demand_value(demand))
    }

    pub fn prepare(&mut self, demand: &ExecutionDemand) -> Result<Value, ControlClientError> {
        self.invoke("prepare", codec::demand_value(demand))
    }

    pub fn observe(&mut self, world_ref: &WorldRef) -> Result<Value, ControlClientError> {
        self.invoke("observe", codec::world_ref_value(world_ref))
    }

    pub fn expose(&mut self, world_ref: &WorldRef) -> Result<Value, ControlClientError> {
        self.invoke("expose", codec::world_ref_value(world_ref))
    }

    pub fn collect(&mut self, world_ref: &WorldRef) -> Result<Value, ControlClientError> {
        self.invoke("collect", codec::world_ref_value(world_ref))
    }

    pub fn release(&mut self, world_ref: &WorldRef) -> Result<Value, ControlClientError> {
        self.invoke("release", codec::world_ref_value(world_ref))
    }

    pub fn reconcile(
        &mut self,
        desired: &[DesiredMaterialState],
    ) -> Result<Value, ControlClientError> {
        self.invoke("reconcile", codec::desired_value(desired))
    }

    pub fn invoke(&mut self, operation: &str, payload: Value) -> Result<Value, ControlClientError> {
        let request_id = format!("request:{}", self.next_request_id);
        self.next_request_id += 1;
        let request = json!({
            "version": self.protocol_version,
            "request_id": request_id,
            "authorization": self.authorization,
            "operation": operation,
            "payload": payload,
        });
        let encoded = serde_json::to_vec(&request).map_err(|error| {
            ControlClientError::InvalidResponse(format!("encode control request: {error}"))
        })?;
        let response = self
            .transport
            .round_trip(&encoded)
            .map_err(|error| ControlClientError::TransportUnavailable(error.to_string()))?;
        self.decode_response(&request_id, &response)
    }

    fn decode_response(
        &self,
        request_id: &str,
        response: &[u8],
    ) -> Result<Value, ControlClientError> {
        let value: Value = serde_json::from_slice(response).map_err(|error| {
            ControlClientError::InvalidResponse(format!("response is not valid JSON: {error}"))
        })?;
        let object = value.as_object().ok_or_else(|| {
            ControlClientError::InvalidResponse("response must be a JSON object".into())
        })?;
        let version = object
            .get("version")
            .and_then(Value::as_str)
            .ok_or_else(|| {
                ControlClientError::InvalidResponse("response version is missing".into())
            })?;
        if version != CONTROL_PROTOCOL_VERSION {
            return Err(ControlClientError::ProtocolIncompatible(format!(
                "response protocol `{version}` is incompatible with `{CONTROL_PROTOCOL_VERSION}`"
            )));
        }
        if object.get("request_id").and_then(Value::as_str) != Some(request_id) {
            return Err(ControlClientError::InvalidResponse(
                "response request_id does not match request".into(),
            ));
        }
        match object.get("ok").and_then(Value::as_bool) {
            Some(true) => object.get("payload").cloned().ok_or_else(|| {
                ControlClientError::InvalidResponse("successful response has no payload".into())
            }),
            Some(false) => self.decode_error(object),
            None => Err(ControlClientError::InvalidResponse(
                "response ok field is missing or invalid".into(),
            )),
        }
    }

    fn decode_error(
        &self,
        response: &serde_json::Map<String, Value>,
    ) -> Result<Value, ControlClientError> {
        let error = response
            .get("error")
            .and_then(Value::as_object)
            .ok_or_else(|| ControlClientError::InvalidResponse("error body is missing".into()))?;
        let kind = error
            .get("kind")
            .and_then(Value::as_str)
            .ok_or_else(|| ControlClientError::InvalidResponse("error kind is missing".into()))?;
        let message = error
            .get("message")
            .and_then(Value::as_str)
            .ok_or_else(|| ControlClientError::InvalidResponse("error message is missing".into()))?
            .to_owned();
        match kind {
            "protocol-incompatible" => Err(ControlClientError::ProtocolIncompatible(message)),
            "authentication-failed" => Err(ControlClientError::AuthenticationFailed(message)),
            "invalid-demand" => Err(ControlClientError::Remote(WorkcellError::InvalidDemand(
                message,
            ))),
            "unsatisfied-demand" => Err(ControlClientError::Remote(
                WorkcellError::UnsatisfiedDemand(message),
            )),
            "unavailable" => Err(ControlClientError::Remote(WorkcellError::Unavailable(
                message,
            ))),
            "degraded" => Err(ControlClientError::Remote(WorkcellError::Degraded(message))),
            "operation-failed" => Err(ControlClientError::Remote(WorkcellError::OperationFailed(
                message,
            ))),
            "cleanup-failed" => Err(ControlClientError::Remote(WorkcellError::CleanupFailed(
                message,
            ))),
            "reconciliation-failed" => Err(ControlClientError::Remote(
                WorkcellError::ReconciliationFailed(message),
            )),
            "not-found" => Err(ControlClientError::Remote(WorkcellError::NotFound(message))),
            "unsupported" => Err(ControlClientError::Remote(WorkcellError::Unsupported(
                message,
            ))),
            other => Err(ControlClientError::InvalidResponse(format!(
                "unknown control error kind `{other}`: {message}"
            ))),
        }
    }
}

fn frame(payload: &[u8]) -> Result<Vec<u8>, TransportFailure> {
    let length = u32::try_from(payload.len())
        .map_err(|_| TransportFailure::new("control frame exceeds u32 length"))?;
    let mut framed = Vec::with_capacity(payload.len() + 4);
    framed.extend_from_slice(&length.to_be_bytes());
    framed.extend_from_slice(payload);
    Ok(framed)
}

fn unframe(framed: &[u8]) -> Result<&[u8], TransportFailure> {
    let prefix: [u8; 4] = framed
        .get(..4)
        .ok_or_else(|| TransportFailure::new("control frame has no length prefix"))?
        .try_into()
        .map_err(|_| TransportFailure::new("control frame length prefix is invalid"))?;
    let expected = u32::from_be_bytes(prefix) as usize;
    let payload = &framed[4..];
    if payload.len() != expected {
        return Err(TransportFailure::new(format!(
            "control frame length mismatch: expected {expected}, got {}",
            payload.len()
        )));
    }
    Ok(payload)
}
