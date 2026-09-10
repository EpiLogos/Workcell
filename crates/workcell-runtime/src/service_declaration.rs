//! Operator-declared logical services for an ordinary local Workcell.
//!
//! Workcell owns no model, agent or engine ontology. A declaration only binds a
//! caller-owned logical service ref to material facts an operator already knows:
//! which executable or target-native command backs it, where its endpoint is,
//! and how readiness is decided. Nothing here names a vendor, and nothing here
//! asserts that the declared service was ever physically accepted — a
//! declaration is a claim about intent, and the provider still has to find the
//! executable, start it and reach it before any binding is called healthy.

use std::{
    collections::BTreeMap,
    fs,
    path::{Path, PathBuf},
};

use epilogos_workcell_core::{Result, WorkcellError};
use serde_json::Value;

use crate::{
    ExternalManagedService, ExternalServiceAcquisition, ExternalServiceCommand, ManagedHostService,
    TcpEndpointProbe,
};

pub const SERVICE_DECLARATION_SCHEMA: &str = "workcell.service-declaration/v1";

/// Conventional declaration file inside a Workcell state root.
///
/// Both the zero-daemon CLI and the Control Service read the same file because
/// both compose the same collapsed-local Workcell.
pub const SERVICE_DECLARATION_FILE: &str = "services.json";

/// How long a declared service's material process outlives the Workcell process
/// that resolved it.
///
/// This distinction is not cosmetic. A provider-process-scoped service is a
/// child of the Workcell process: in a long-running Control Service it lives as
/// long as the daemon, and in a one-shot CLI invocation it dies when the command
/// returns. A target-owned service is started and supervised by something else,
/// so a later invocation can honestly re-observe it.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ServiceLifetime {
    ProviderProcessScoped,
    TargetOwned,
}

impl ServiceLifetime {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::ProviderProcessScoped => "provider-process-scoped",
            Self::TargetOwned => "target-owned",
        }
    }
}

/// Declared services split by which provider can honestly materialise them.
#[derive(Clone, Debug, Default)]
pub struct DeclaredServices {
    pub managed: Vec<ManagedHostService>,
    pub target_owned: Vec<ExternalManagedService>,
}

impl DeclaredServices {
    pub fn is_empty(&self) -> bool {
        self.managed.is_empty() && self.target_owned.is_empty()
    }

    pub fn len(&self) -> usize {
        self.managed.len() + self.target_owned.len()
    }

    fn extend(&mut self, other: DeclaredServices) {
        self.managed.extend(other.managed);
        self.target_owned.extend(other.target_owned);
    }
}

/// Conventional declaration path for a state root.
pub fn default_service_declaration_path(state_root: impl AsRef<Path>) -> PathBuf {
    state_root.as_ref().join(SERVICE_DECLARATION_FILE)
}

/// Read a declaration file. A missing file is an error; callers that treat the
/// conventional path as optional check for existence first.
pub fn read_service_declarations(path: impl AsRef<Path>) -> Result<DeclaredServices> {
    let path = path.as_ref();
    let raw = fs::read_to_string(path).map_err(|error| {
        WorkcellError::NotFound(format!(
            "read service declaration `{}`: {error}",
            path.display()
        ))
    })?;
    parse_service_declarations(&raw).map_err(|error| annotate(path, error))
}

/// Read the conventional `<state-root>/services.json` when it exists.
pub fn read_state_root_service_declarations(
    state_root: impl AsRef<Path>,
) -> Result<DeclaredServices> {
    let path = default_service_declaration_path(state_root);
    if path.is_file() {
        read_service_declarations(path)
    } else {
        Ok(DeclaredServices::default())
    }
}

pub fn parse_service_declarations(raw: &str) -> Result<DeclaredServices> {
    let document: Value = serde_json::from_str(raw)
        .map_err(|error| WorkcellError::InvalidDemand(format!("invalid JSON: {error}")))?;
    let object = document.as_object().ok_or_else(|| {
        WorkcellError::InvalidDemand("service declaration must be a JSON object".into())
    })?;

    if let Some(schema) = object.get("schema") {
        let schema = schema.as_str().ok_or_else(|| {
            WorkcellError::InvalidDemand("service declaration `schema` must be a string".into())
        })?;
        if schema != SERVICE_DECLARATION_SCHEMA {
            return Err(WorkcellError::InvalidDemand(format!(
                "unsupported service declaration schema `{schema}`, expected `{SERVICE_DECLARATION_SCHEMA}`"
            )));
        }
    }

    let entries = object
        .get("services")
        .map(|value| {
            value.as_array().ok_or_else(|| {
                WorkcellError::InvalidDemand(
                    "service declaration `services` must be an array".into(),
                )
            })
        })
        .transpose()?
        .cloned()
        .unwrap_or_default();

    let mut declared = DeclaredServices::default();
    let mut seen = Vec::new();
    for entry in entries {
        let one = parse_service(&entry)?;
        let logical_ref = one
            .managed
            .first()
            .map(|service| service.logical_ref.clone())
            .or_else(|| {
                one.target_owned
                    .first()
                    .map(|service| service.logical_ref.clone())
            })
            .unwrap_or_default();
        if seen.contains(&logical_ref) {
            return Err(WorkcellError::InvalidDemand(format!(
                "logical service `{logical_ref}` is declared more than once"
            )));
        }
        seen.push(logical_ref);
        declared.extend(one);
    }
    Ok(declared)
}

fn parse_service(entry: &Value) -> Result<DeclaredServices> {
    let object = entry.as_object().ok_or_else(|| {
        WorkcellError::InvalidDemand("each declared service must be a JSON object".into())
    })?;
    let logical_ref = required_str(object, "logical_ref")?;
    let endpoint = required_str(object, "endpoint")?;
    let lifetime = match object.get("lifetime").and_then(Value::as_str) {
        None => {
            return Err(WorkcellError::InvalidDemand(format!(
                "declared service `{logical_ref}` must state a `lifetime` of `{}` or `{}`",
                ServiceLifetime::ProviderProcessScoped.as_str(),
                ServiceLifetime::TargetOwned.as_str()
            )))
        }
        Some(value) if value == ServiceLifetime::ProviderProcessScoped.as_str() => {
            ServiceLifetime::ProviderProcessScoped
        }
        Some(value) if value == ServiceLifetime::TargetOwned.as_str() => {
            ServiceLifetime::TargetOwned
        }
        Some(other) => {
            return Err(WorkcellError::InvalidDemand(format!(
                "declared service `{logical_ref}` has unknown lifetime `{other}`"
            )))
        }
    };
    let metadata = string_map(object, "metadata")?;

    match lifetime {
        ServiceLifetime::ProviderProcessScoped => {
            let mut service =
                ManagedHostService::new(&logical_ref, &endpoint, required_str(object, "program")?)?;
            for arg in string_list(object, "args")? {
                service = service.with_arg(arg);
            }
            for (key, value) in string_map(object, "env")? {
                service = service.with_env(key, value);
            }
            if let Some(cwd) = optional_str(object, "cwd")? {
                service = service.with_cwd(cwd);
            }
            for (key, value) in metadata {
                service = service.with_metadata(key, value);
            }
            if let Some(readiness) = object.get("readiness") {
                service = service.with_tcp_readiness(parse_tcp_probe(&logical_ref, readiness)?);
            }
            Ok(DeclaredServices {
                managed: vec![service],
                target_owned: Vec::new(),
            })
        }
        ServiceLifetime::TargetOwned => {
            let status = parse_command(&logical_ref, "status", require_field(object, "status")?)?;
            let mut service = ExternalManagedService::new(&logical_ref, &endpoint, status)?;
            if let Some(value) = object.get("readiness") {
                service = service.with_readiness(parse_command(&logical_ref, "readiness", value)?);
            }
            if let Some(value) = object.get("start") {
                service = service.with_start(parse_command(&logical_ref, "start", value)?);
            }
            if let Some(value) = object.get("stop") {
                service = service.with_stop(parse_command(&logical_ref, "stop", value)?);
            }
            if let Some(value) = object.get("restart") {
                service = service.with_restart(parse_command(&logical_ref, "restart", value)?);
            }
            service =
                service.with_acquisition(match object.get("acquisition").and_then(Value::as_str) {
                    None | Some("observe-existing") => ExternalServiceAcquisition::ObserveExisting,
                    Some("ensure-running") => ExternalServiceAcquisition::EnsureRunning,
                    Some(other) => {
                        return Err(WorkcellError::InvalidDemand(format!(
                            "declared service `{logical_ref}` has unknown acquisition `{other}`"
                        )))
                    }
                });
            for (key, value) in metadata {
                service = service.with_metadata(key, value)?;
            }
            Ok(DeclaredServices {
                managed: Vec::new(),
                target_owned: vec![service],
            })
        }
    }
}

fn parse_command(logical_ref: &str, field: &str, value: &Value) -> Result<ExternalServiceCommand> {
    let object = value.as_object().ok_or_else(|| {
        WorkcellError::InvalidDemand(format!(
            "declared service `{logical_ref}` field `{field}` must be a JSON object"
        ))
    })?;
    let mut command = ExternalServiceCommand::new(required_str(object, "program")?)?;
    for arg in string_list(object, "args")? {
        command = command.with_arg(arg);
    }
    for (key, item) in string_map(object, "env")? {
        command = command.with_env(key, item)?;
    }
    Ok(command)
}

fn parse_tcp_probe(logical_ref: &str, value: &Value) -> Result<TcpEndpointProbe> {
    let object = value.as_object().ok_or_else(|| {
        WorkcellError::InvalidDemand(format!(
            "declared service `{logical_ref}` field `readiness` must be a JSON object"
        ))
    })?;
    let host = required_str(object, "host")?;
    let port = object
        .get("port")
        .and_then(Value::as_u64)
        .ok_or_else(|| {
            WorkcellError::InvalidDemand(format!(
                "declared service `{logical_ref}` readiness requires a numeric `port`"
            ))
        })
        .and_then(|port| {
            u16::try_from(port).map_err(|_| {
                WorkcellError::InvalidDemand(format!(
                    "declared service `{logical_ref}` readiness port `{port}` is out of range"
                ))
            })
        })?;
    let mut probe = TcpEndpointProbe::new(host, port)?;
    if let Some(timeout) = object.get("timeout_ms").and_then(Value::as_u64) {
        probe = probe.with_timeout_ms(timeout);
    }
    if let Some(interval) = object.get("interval_ms").and_then(Value::as_u64) {
        probe = probe.with_interval_ms(interval);
    }
    Ok(probe)
}

fn require_field<'a>(object: &'a serde_json::Map<String, Value>, key: &str) -> Result<&'a Value> {
    object
        .get(key)
        .ok_or_else(|| WorkcellError::InvalidDemand(format!("declared service requires `{key}`")))
}

fn required_str(object: &serde_json::Map<String, Value>, key: &str) -> Result<String> {
    require_field(object, key)?
        .as_str()
        .map(str::to_owned)
        .filter(|value| !value.trim().is_empty())
        .ok_or_else(|| {
            WorkcellError::InvalidDemand(format!(
                "declared service field `{key}` must be a non-empty string"
            ))
        })
}

fn optional_str(object: &serde_json::Map<String, Value>, key: &str) -> Result<Option<String>> {
    match object.get(key) {
        None | Some(Value::Null) => Ok(None),
        Some(value) => value
            .as_str()
            .map(|value| Some(value.to_owned()))
            .ok_or_else(|| {
                WorkcellError::InvalidDemand(format!(
                    "declared service field `{key}` must be a string"
                ))
            }),
    }
}

fn string_list(object: &serde_json::Map<String, Value>, key: &str) -> Result<Vec<String>> {
    let Some(value) = object.get(key) else {
        return Ok(Vec::new());
    };
    let array = value.as_array().ok_or_else(|| {
        WorkcellError::InvalidDemand(format!("declared service field `{key}` must be an array"))
    })?;
    array
        .iter()
        .map(|item| {
            item.as_str().map(str::to_owned).ok_or_else(|| {
                WorkcellError::InvalidDemand(format!(
                    "declared service field `{key}` must contain only strings"
                ))
            })
        })
        .collect()
}

fn string_map(
    object: &serde_json::Map<String, Value>,
    key: &str,
) -> Result<BTreeMap<String, String>> {
    let Some(value) = object.get(key) else {
        return Ok(BTreeMap::new());
    };
    let map = value.as_object().ok_or_else(|| {
        WorkcellError::InvalidDemand(format!(
            "declared service field `{key}` must be a JSON object"
        ))
    })?;
    map.iter()
        .map(|(name, item)| {
            item.as_str()
                .map(|item| (name.clone(), item.to_owned()))
                .ok_or_else(|| {
                    WorkcellError::InvalidDemand(format!(
                        "declared service field `{key}.{name}` must be a string"
                    ))
                })
        })
        .collect()
}

fn annotate(path: &Path, error: WorkcellError) -> WorkcellError {
    WorkcellError::InvalidDemand(format!("service declaration `{}`: {error}", path.display()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn managed_and_target_owned_services_parse_into_their_own_providers() {
        let declared = parse_service_declarations(
            r#"{
              "schema": "workcell.service-declaration/v1",
              "services": [
                {
                  "logical_ref": "inference:local-chat",
                  "lifetime": "provider-process-scoped",
                  "endpoint": "http://127.0.0.1:21434",
                  "program": "/usr/bin/true",
                  "args": ["serve"],
                  "env": {"BIND": "127.0.0.1:21434"},
                  "metadata": {"declared_by": "operator"},
                  "readiness": {"host": "127.0.0.1", "port": 21434, "timeout_ms": 5000}
                },
                {
                  "logical_ref": "inference:shared-chat",
                  "lifetime": "target-owned",
                  "endpoint": "http://127.0.0.1:11434",
                  "status": {"program": "/usr/bin/true", "args": ["status"]}
                }
              ]
            }"#,
        )
        .unwrap();

        assert_eq!(declared.managed.len(), 1);
        assert_eq!(declared.target_owned.len(), 1);
        let managed = &declared.managed[0];
        assert_eq!(managed.logical_ref, "inference:local-chat");
        assert_eq!(managed.args, vec!["serve".to_owned()]);
        assert_eq!(managed.readiness.as_ref().unwrap().port, 21434);
        assert_eq!(
            declared.target_owned[0].logical_ref,
            "inference:shared-chat"
        );
    }

    #[test]
    fn a_declaration_without_a_lifetime_is_refused_rather_than_guessed() {
        let error = parse_service_declarations(
            r#"{"services":[{"logical_ref":"inference:x","endpoint":"http://127.0.0.1:1","program":"/usr/bin/true"}]}"#,
        )
        .unwrap_err();
        assert!(
            error.to_string().contains("lifetime"),
            "unexpected error: {error}"
        );
    }

    #[test]
    fn duplicate_logical_refs_are_refused() {
        let error = parse_service_declarations(
            r#"{"services":[
              {"logical_ref":"inference:x","lifetime":"provider-process-scoped","endpoint":"http://127.0.0.1:1","program":"/usr/bin/true"},
              {"logical_ref":"inference:x","lifetime":"provider-process-scoped","endpoint":"http://127.0.0.1:2","program":"/usr/bin/true"}
            ]}"#,
        )
        .unwrap_err();
        assert!(
            error.to_string().contains("declared more than once"),
            "unexpected error: {error}"
        );
    }

    #[test]
    fn an_unknown_schema_is_refused() {
        let error = parse_service_declarations(r#"{"schema":"something-else/v9","services":[]}"#)
            .unwrap_err();
        assert!(
            error
                .to_string()
                .contains("unsupported service declaration schema"),
            "unexpected error: {error}"
        );
    }
}
