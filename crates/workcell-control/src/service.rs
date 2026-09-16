use epilogos_workcell_core::{WorkcellControlPlane, WorkcellError};
use epilogos_workcell_wire::ConnectionGrant;
use serde_json::{json, Map, Value};

use crate::grants::{ConnectionGrants, GrantDecision};
use crate::{codec, software_version, CONNECTION_HANDSHAKE_OPERATION, CONTROL_PROTOCOL_VERSION};

/// One access decision for one request. `FullAccess` is the pre-existing
/// open/static-token behaviour; `Scoped` is a connection grant's scope.
enum AccessDecision {
    FullAccess,
    Scoped(Box<ConnectionGrant>),
    Refused(String),
}

pub struct ControlService<C> {
    control: C,
    authorization: Option<String>,
    grants: Option<ConnectionGrants>,
}

impl<C> ControlService<C>
where
    C: WorkcellControlPlane,
{
    pub fn new(control: C) -> Self {
        Self {
            control,
            authorization: None,
            grants: None,
        }
    }

    pub fn with_authorization(mut self, authorization: impl Into<String>) -> Self {
        self.authorization = Some(authorization.into());
        self
    }

    /// Serve cross-cell clients through the connection grants registry at
    /// this cell's state root. Every request re-checks the registry, so a
    /// `workcell revoke` in another process takes effect at the next use.
    pub fn with_connection_grants(mut self, grants: ConnectionGrants) -> Self {
        self.grants = Some(grants);
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
        let operation = match required_string(envelope, "operation") {
            Ok(value) => value,
            Err(message) => return error_response(request_id, "invalid-request", message),
        };
        let payload = envelope.get("payload").unwrap_or(&Value::Null);
        let supplied = envelope.get("authorization").and_then(Value::as_str);

        let decision = match self.access_decision(supplied, operation) {
            Ok(decision) => decision,
            Err(error) => {
                let (kind, message) = workcell_error_parts(&error);
                return error_response(request_id, kind, message);
            }
        };

        // The handshake is the compatibility disclosure. It never errors on
        // authorisation: a refused client still learns the protocol and
        // software versions so its refusal can be loud and precise — but it
        // learns no capability and no workcell identity.
        if operation == CONNECTION_HANDSHAKE_OPERATION {
            return self.handshake_response(request_id, decision);
        }

        let scope = match decision {
            AccessDecision::FullAccess => None,
            AccessDecision::Scoped(grant) => Some(grant),
            AccessDecision::Refused(reason) => {
                return error_response(request_id, "authentication-failed", reason)
            }
        };

        match self.dispatch(operation, payload, scope.as_deref()) {
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

    /// Decide access for one operation. The static token (when configured)
    /// keeps full access; otherwise a wired grants registry decides, and a
    /// service with neither remains the open collapsed-local form.
    fn access_decision(
        &self,
        credential: Option<&str>,
        operation: &str,
    ) -> Result<AccessDecision, WorkcellError> {
        let static_ok = self
            .authorization
            .as_ref()
            .is_some_and(|required| credential == Some(required.as_str()));
        if static_ok {
            return Ok(AccessDecision::FullAccess);
        }
        match &self.grants {
            None => {
                if self.authorization.is_some() {
                    Ok(AccessDecision::Refused(
                        "control-service authentication failed".into(),
                    ))
                } else {
                    Ok(AccessDecision::FullAccess)
                }
            }
            Some(grants) => match grants.authorise(credential, operation)? {
                GrantDecision::Allowed { grant } => Ok(AccessDecision::Scoped(grant)),
                GrantDecision::NotPermitted { grant: boxed } if operation == CONNECTION_HANDSHAKE_OPERATION => {
                    // The handshake authorises on credential validity, not
                    // operation membership: an authorised client gets the
                    // identity and scope disclosure, whatever its grant
                    // permits.
                    Ok(AccessDecision::Scoped(boxed))
                }
                GrantDecision::NotPermitted { grant } => Ok(AccessDecision::Refused(format!(
                    "grant `{}` does not permit operation `{operation}` on this Workcell",
                    grant.grant_ref
                ))),
                GrantDecision::Revoked { grant } => Ok(AccessDecision::Refused(format!(
                    "credential matches grant `{}`, which was revoked on this Workcell",
                    grant.grant_ref
                ))),
                GrantDecision::Expired { grant } => Ok(AccessDecision::Refused(format!(
                    "credential matches grant `{}`, which expired on this Workcell; ask the serving operator to run `workcell authorise` again",
                    grant.grant_ref
                ))),
                GrantDecision::UnknownCredential => Ok(AccessDecision::Refused(
                    "no active connection grant matches the presented credential; ask the serving operator to run `workcell authorise`".into(),
                )),
                GrantDecision::NoCredential => Ok(AccessDecision::Refused(
                    "this Workcell requires a connection credential and none was presented".into(),
                )),
            },
        }
    }

    fn handshake_response(&self, request_id: Value, decision: AccessDecision) -> Value {
        let (authorised, workcell_ref, grant, reason) = match decision {
            AccessDecision::FullAccess => (true, self.disclosed_workcell_ref(), None, None),
            AccessDecision::Scoped(grant) => (
                true,
                self.disclosed_workcell_ref(),
                Some(json!({
                    "grant_ref": grant.grant_ref,
                    "client_label": grant.client_label,
                    "operations": grant.operations,
                    "advertise": grant.advertise,
                    "expires_at_unix_ms": grant.expires_at_unix_ms,
                })),
                None,
            ),
            AccessDecision::Refused(reason) => (false, None, None, Some(reason)),
        };
        json!({
            "version": CONTROL_PROTOCOL_VERSION,
            "request_id": request_id,
            "ok": true,
            "payload": {
                "protocol": CONTROL_PROTOCOL_VERSION,
                "software": software_version(),
                "authorised": authorised,
                "workcell_ref": workcell_ref,
                "grant": grant,
                "reason": reason,
            },
        })
    }

    fn disclosed_workcell_ref(&self) -> Option<String> {
        self.control
            .discover()
            .ok()
            .map(|discovery| discovery.workcell_ref.to_string())
    }

    /// Discovery filtered to the grant's advertisement. Disclosure is part
    /// of the grant: what an unscoped or unfiltered request sees is this
    /// cell's own advertisement, which is not authorisation to use it.
    fn filtered_discovery(&self, scope: Option<&ConnectionGrant>) -> Result<Value, WorkcellError> {
        let discovery = self.control.discover()?;
        let mut value = codec::discovery_value(&discovery);
        if let Some(advertise) = scope
            .map(|grant| &grant.advertise)
            .filter(|advertise| !advertise.is_empty())
        {
            if let Some(offers) = value.get_mut("offers").and_then(Value::as_array_mut) {
                offers.retain(|offer| {
                    offer
                        .get("port")
                        .and_then(Value::as_str)
                        .is_some_and(|port| advertise.iter().any(|allowed| allowed == port))
                });
            }
        }
        Ok(value)
    }

    fn dispatch(
        &mut self,
        operation: &str,
        payload: &Value,
        scope: Option<&ConnectionGrant>,
    ) -> Result<Value, WorkcellError> {
        match operation {
            "status" => {
                let discovery = self.filtered_discovery(scope)?;
                let offers = discovery["offers"].as_array().cloned().unwrap_or_default();
                let providers = offers
                    .iter()
                    .filter_map(|offer| offer.get("provider_ref").and_then(Value::as_str))
                    .collect::<std::collections::BTreeSet<_>>()
                    .len();
                Ok(json!({
                    "workcell_ref": discovery["workcell_ref"],
                    "health": discovery["health"],
                    "providers": providers,
                    "offers": offers.len(),
                }))
            }
            "discover" => self.filtered_discovery(scope),
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
            "inspect" => {
                let world_ref = codec::decode_world_ref(payload)?;
                let world = self.control.inspect(&world_ref)?;
                codec::prepared_world_value(&world)
            }
            "recover" => {
                let world_ref = codec::decode_world_ref(payload)?;
                let world = self.control.recover(&world_ref)?;
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
        WorkcellError::ReconciliationFailed(message) => ("reconciliation-failed", message.clone()),
        WorkcellError::NotFound(message) => ("not-found", message.clone()),
        WorkcellError::Unsupported(message) => ("unsupported", message.clone()),
    }
}

fn escape_json(value: &str) -> String {
    value.replace('\\', "\\\\").replace('"', "\\\"")
}
