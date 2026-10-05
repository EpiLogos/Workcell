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
    io::Read,
    path::{Path, PathBuf},
};

use epilogos_workcell_core::{Result, WorkcellError};
use epilogos_workcell_opensandbox::OpenSandboxConfig;
use serde_json::Value;

use crate::external_service::INSTANCE_SCHEMA;

use crate::{
    ExternalManagedService, ExternalServiceAcquisition, ExternalServiceCommand, ManagedHostService,
    TcpEndpointProbe, OPENSANDBOX_PROVIDER_REF,
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
/// `execution` carries an optional OpenSandbox lifecycle deployment the
/// Workcell materialises sandbox execution through.
#[derive(Clone, Debug, Default)]
pub struct DeclaredServices {
    pub managed: Vec<ManagedHostService>,
    pub target_owned: Vec<ExternalManagedService>,
    pub execution: Vec<OpenSandboxConfig>,
}

impl DeclaredServices {
    pub fn is_empty(&self) -> bool {
        self.managed.is_empty() && self.target_owned.is_empty() && self.execution.is_empty()
    }

    pub fn len(&self) -> usize {
        self.managed.len() + self.target_owned.len() + self.execution.len()
    }

    fn extend(&mut self, other: DeclaredServices) {
        self.managed.extend(other.managed);
        self.target_owned.extend(other.target_owned);
        self.execution.extend(other.execution);
    }
}

/// Conventional declaration path for a state root.
pub fn default_service_declaration_path(state_root: impl AsRef<Path>) -> PathBuf {
    state_root.as_ref().join(SERVICE_DECLARATION_FILE)
}

/// Read a declaration file. Missing material is an error; regular symlink and
/// hardlink aliases retain their ordinary read-only behavior.
pub fn read_service_declarations(path: impl AsRef<Path>) -> Result<DeclaredServices> {
    read_declaration_material(path.as_ref(), false)
}

/// Read the conventional `<state-root>/services.json`. Only an actual initial
/// NotFound means no declaration; other IO failures and nonregular material
/// refuse the composition rather than declaring an empty service set.
pub fn read_state_root_service_declarations(
    state_root: impl AsRef<Path>,
) -> Result<DeclaredServices> {
    let path = default_service_declaration_path(state_root);
    read_declaration_material(&path, true)
}

fn read_declaration_material(path: &Path, optional: bool) -> Result<DeclaredServices> {
    let initial = match fs::metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if optional && error.kind() == std::io::ErrorKind::NotFound => {
            return Ok(DeclaredServices::default())
        }
        Err(error) => return Err(declaration_io(path, "inspect", error)),
    };
    if !initial.is_file() {
        return Err(nonregular_declaration(path));
    }

    let mut options = fs::OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        // A replacement FIFO must not block this read-only open. The same
        // held file is checked below; stable regular aliases remain allowed.
        options.custom_flags(libc::O_NONBLOCK);
    }
    let mut file = options
        .open(path)
        .map_err(|error| declaration_io(path, "open", error))?;
    let held = file
        .metadata()
        .map_err(|error| declaration_io(path, "inspect held file", error))?;
    if !held.is_file() {
        return Err(nonregular_declaration(path));
    }
    let mut raw = String::new();
    file.read_to_string(&mut raw)
        .map_err(|error| declaration_io(path, "read", error))?;
    parse_service_declarations(&raw).map_err(|error| annotate(path, error))
}

fn nonregular_declaration(path: &Path) -> WorkcellError {
    WorkcellError::InvalidDemand(format!(
        "service declaration `{}` must be a regular file",
        path.display()
    ))
}

fn declaration_io(path: &Path, operation: &str, error: std::io::Error) -> WorkcellError {
    let detail = format!(
        "{operation} service declaration `{}`: {error}; io_kind={:?}; raw_os_error={:?}",
        path.display(),
        error.kind(),
        error.raw_os_error()
    );
    if error.kind() == std::io::ErrorKind::NotFound {
        WorkcellError::NotFound(detail)
    } else {
        WorkcellError::Unavailable(detail)
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

    let executions = object
        .get("execution")
        .map(|value| {
            value.as_array().ok_or_else(|| {
                WorkcellError::InvalidDemand(
                    "service declaration `execution` must be an array".into(),
                )
            })
        })
        .transpose()?
        .cloned()
        .unwrap_or_default();
    for entry in executions {
        declared.execution.push(parse_execution_deployment(&entry)?);
    }
    Ok(declared)
}

/// One operator-declared execution deployment. The only declared kind today is
/// an OpenSandbox lifecycle deployment; every field maps onto
/// `epilogos_workcell_opensandbox::OpenSandboxConfig`, and the defaults are
/// that config's own defaults — a declaration names the deployment, it does
/// not invent a provider contract.
fn parse_execution_deployment(entry: &Value) -> Result<OpenSandboxConfig> {
    let object = entry.as_object().ok_or_else(|| {
        WorkcellError::InvalidDemand(
            "each declared execution deployment must be a JSON object".into(),
        )
    })?;
    let kind = object
        .get("kind")
        .and_then(Value::as_str)
        .unwrap_or("opensandbox");
    if kind != "opensandbox" {
        return Err(WorkcellError::InvalidDemand(format!(
            "declared execution deployment has unknown kind `{kind}`; known kinds: opensandbox"
        )));
    }
    let provider_ref = epilogos_workcell_core::ProviderRef::new(
        optional_str(object, "provider_ref")?
            .unwrap_or_else(|| OPENSANDBOX_PROVIDER_REF.to_owned()),
    )?;
    let mut config = OpenSandboxConfig::local(
        provider_ref,
        optional_str(object, "image")?
            .unwrap_or_else(|| "opensandbox/code-interpreter:v1.1.0".to_owned()),
        {
            let declared_entrypoint = string_list(object, "entrypoint")?;
            if declared_entrypoint.is_empty() && object.get("entrypoint").is_none() {
                vec!["/opt/code-interpreter/code-interpreter.sh".to_owned()]
            } else {
                declared_entrypoint
            }
        },
    )?;
    if let Some(url) = optional_str(object, "lifecycle_base_url")? {
        config.lifecycle_base_url = url;
    }
    config.use_server_proxy = object
        .get("use_server_proxy")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    if let Some(env) = optional_str(object, "api_key_env")? {
        config.api_key_env = Some(env);
    }
    for (key, value) in string_map(object, "env")? {
        config.environment.insert(key, value);
    }
    for (key, value) in string_map(object, "metadata")? {
        config.metadata.insert(key, value);
    }
    if let Some(capacity) = object.get("capacity") {
        let capacity = capacity.as_object().ok_or_else(|| {
            WorkcellError::InvalidDemand(
                "declared execution deployment field `capacity` must be a JSON object".into(),
            )
        })?;
        for (key, value) in capacity {
            let value = value.as_object().ok_or_else(|| {
                WorkcellError::InvalidDemand(format!(
                    "declared execution deployment capacity `{key}` must be an object"
                ))
            })?;
            let amount = value.get("amount").and_then(Value::as_u64).ok_or_else(|| {
                WorkcellError::InvalidDemand(format!(
                    "declared execution deployment capacity `{key}` requires a numeric `amount`"
                ))
            })?;
            config.capacity.insert(
                key.clone(),
                epilogos_workcell_core::Capacity {
                    amount,
                    unit: value.get("unit").and_then(Value::as_str).map(str::to_owned),
                },
            );
        }
    }
    if let Some(egress) = object.get("egress") {
        let egress = egress.as_object().ok_or_else(|| {
            WorkcellError::InvalidDemand(
                "declared execution deployment field `egress` must be a JSON object".into(),
            )
        })?;
        let policy = epilogos_workcell_opensandbox::OpenSandboxEgressPolicy {
            default_deny: egress
                .get("default_deny")
                .and_then(Value::as_bool)
                .unwrap_or(false),
            allowed_hosts: string_list(egress, "allow")?,
            credential_proxy: egress
                .get("credential_proxy")
                .and_then(Value::as_bool)
                .unwrap_or(false),
        };
        config.egress = Some(policy);
    }
    config
        .validate()
        .map_err(|error| WorkcellError::InvalidDemand(error.to_string()))?;
    Ok(config)
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
            if object.contains_key("target_instance") {
                return Err(WorkcellError::InvalidDemand(
                    "target_instance applies only to target-owned services".into(),
                ));
            }
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
                execution: Vec::new(),
            })
        }
        ServiceLifetime::TargetOwned => {
            let status = parse_command(&logical_ref, "status", require_field(object, "status")?)?;
            let mut service = ExternalManagedService::new(&logical_ref, &endpoint, status)?;
            if let Some(value) = object.get("target_instance") {
                let contract = value.as_object().ok_or_else(|| {
                    WorkcellError::InvalidDemand("target_instance must be a JSON object".into())
                })?;
                if required_str(contract, "schema")? != INSTANCE_SCHEMA {
                    return Err(WorkcellError::InvalidDemand(
                        "unsupported target_instance schema".into(),
                    ));
                }
                service = service.with_target_instance(parse_command(
                    &logical_ref,
                    "target_instance.capture",
                    require_field(contract, "capture")?,
                )?);
            }
            if let Some(value) = object.get("readiness") {
                service = service.with_readiness(parse_command(&logical_ref, "readiness", value)?);
                let timeout_ms = value.get("timeout_ms").and_then(Value::as_u64);
                let interval_ms = value.get("interval_ms").and_then(Value::as_u64);
                match (timeout_ms, interval_ms) {
                    (Some(timeout), Some(interval)) => {
                        service = service.with_readiness_timing(timeout, interval);
                    }
                    (None, None) => {}
                    _ => {
                        return Err(WorkcellError::InvalidDemand(format!(
                            "declared service `{logical_ref}` readiness needs both `timeout_ms` and `interval_ms`"
                        )))
                    }
                }
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
                execution: Vec::new(),
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
    fn target_instance_contract_requires_its_schema_and_target_owned_lifetime() {
        for invalid in [
            r#"{"services":[{"logical_ref":"service:x","endpoint":"http://127.0.0.1:1","lifetime":"target-owned","status":{"program":"controller"},"target_instance":{"capture":{"program":"controller"}}}]}"#,
            r#"{"services":[{"logical_ref":"service:x","endpoint":"http://127.0.0.1:1","lifetime":"target-owned","status":{"program":"controller"},"target_instance":{"schema":"unknown/v1","capture":{"program":"controller"}}}]}"#,
            r#"{"services":[{"logical_ref":"service:x","endpoint":"http://127.0.0.1:1","lifetime":"provider-process-scoped","program":"controller","target_instance":{"schema":"workcell.external-target-instance/v1","capture":{"program":"controller"}}}]}"#,
        ] {
            assert!(parse_service_declarations(invalid).is_err());
        }
    }

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

    #[test]
    fn an_execution_deployment_parses_into_the_opensandbox_config() {
        let declared = parse_service_declarations(
            r#"{
              "schema": "workcell.service-declaration/v1",
              "execution": [
                {
                  "kind": "opensandbox",
                  "lifecycle_base_url": "http://127.0.0.1:8080/v1",
                  "use_server_proxy": true,
                  "api_key_env": "OPEN_SANDBOX_API_KEY",
                  "env": {"WORKCELL_PROVENANCE": "declared"},
                  "metadata": {"declared_by": "operator"}
                }
              ]
            }"#,
        )
        .unwrap();

        assert_eq!(declared.execution.len(), 1);
        let deployment = &declared.execution[0];
        assert_eq!(deployment.provider_ref.as_str(), "provider:opensandbox");
        assert_eq!(deployment.lifecycle_base_url, "http://127.0.0.1:8080/v1");
        assert!(deployment.use_server_proxy);
        assert_eq!(
            deployment.api_key_env.as_deref(),
            Some("OPEN_SANDBOX_API_KEY")
        );
        match &deployment.startup {
            epilogos_workcell_opensandbox::OpenSandboxStartupSource::Image { uri } => {
                assert_eq!(uri, "opensandbox/code-interpreter:v1.1.0");
            }
            other => panic!("declared image must parse as an image startup source: {other:?}"),
        }
        assert_eq!(
            deployment.metadata.get("declared_by"),
            Some(&"operator".to_owned())
        );
    }

    #[test]
    fn an_unknown_execution_kind_is_refused() {
        let error = parse_service_declarations(r#"{"execution":[{"kind":"hypervisor-mystery"}]}"#)
            .unwrap_err();
        assert!(
            error.to_string().contains("unknown kind"),
            "unexpected error: {error}"
        );
    }
}

#[cfg(all(test, unix))]
mod native_file_tests {
    use super::*;
    use crate::bounded_process::status_test_support::{self as support, Fixture};
    use crate::{CollapsedLocalConfig, CollapsedLocalWorkcell};
    use epilogos_workcell_core::WorkcellRef;
    use std::os::unix::ffi::OsStrExt;
    use std::os::unix::fs::{symlink, FileTypeExt, PermissionsExt};
    use std::time::Duration;

    fn declaration(marker: &Path) -> Vec<u8> {
        serde_json::to_vec(&serde_json::json!({
            "schema": SERVICE_DECLARATION_SCHEMA,
            "services": [{
                "logical_ref": "service:actual-file-read",
                "endpoint": "http://127.0.0.1:1",
                "lifetime": "target-owned",
                "status": {
                    "program": "/bin/sh",
                    "args": ["-c", "printf executed > \"$1\"", "status", marker]
                }
            }]
        }))
        .unwrap()
    }

    fn assert_actual_io(error: WorkcellError, observed: &std::io::Error) {
        match error {
            WorkcellError::Unavailable(detail) => {
                assert!(detail.contains(&format!("io_kind={:?}", observed.kind())));
                assert!(detail.contains(&format!("raw_os_error={:?}", observed.raw_os_error())));
                assert!(detail.contains(&observed.to_string()));
            }
            other => panic!("actual non-NotFound IO was misclassified: {other}"),
        }
    }

    #[test]
    fn actual_optional_service_file_absence_is_empty_and_explicit_absence_is_notfound() {
        let fixture = Fixture::new("declaration-absence");
        let path = default_service_declaration_path(&fixture.root);
        assert!(!path.exists());
        assert!(read_state_root_service_declarations(&fixture.root)
            .unwrap()
            .is_empty());
        assert!(matches!(
            read_service_declarations(&path),
            Err(WorkcellError::NotFound(_))
        ));
        assert!(!path.exists());
        fixture.finish();
    }

    #[test]
    fn actual_regular_declaration_and_stable_alias_preserve_same_services() {
        let fixture = Fixture::new("declaration-aliases");
        let marker = fixture.root.join("must-not-execute");
        let body = declaration(&marker);
        let path = default_service_declaration_path(&fixture.root);
        fs::write(&path, &body).unwrap();
        let hardlink = fixture.root.join("hardlink.json");
        let alias = fixture.root.join("alias.json");
        fs::hard_link(&path, &hardlink).unwrap();
        symlink(&path, &alias).unwrap();
        let expected = read_state_root_service_declarations(&fixture.root).unwrap();
        assert_eq!(expected.len(), 1);
        assert!(expected.managed.is_empty() && expected.execution.is_empty());
        assert_eq!(
            expected.target_owned[0].logical_ref,
            "service:actual-file-read"
        );
        assert_eq!(expected.target_owned[0].endpoint, "http://127.0.0.1:1");
        for selected in [&path, &hardlink, &alias] {
            let actual = read_service_declarations(selected).unwrap();
            assert_eq!(actual.len(), 1);
            assert_eq!(actual.target_owned, expected.target_owned);
            assert_eq!(fs::read(selected).unwrap(), body);
        }
        #[cfg(target_os = "linux")]
        {
            let non_utf8 = fixture
                .root
                .join(std::ffi::OsStr::from_bytes(b"declaration-\xff.json"));
            fs::hard_link(&path, &non_utf8).unwrap();
            let actual = read_service_declarations(&non_utf8).unwrap();
            assert_eq!(actual.len(), 1);
            assert_eq!(actual.target_owned, expected.target_owned);
            assert_eq!(fs::read(&non_utf8).unwrap(), body);
        }
        #[cfg(target_os = "macos")]
        {
            use std::os::unix::fs::MetadataExt;

            let non_utf8 = fixture
                .root
                .join(std::ffi::OsStr::from_bytes(b"declaration-\xff.json"));
            let before = fs::metadata(&path).unwrap();
            let refused = fs::hard_link(&path, &non_utf8).unwrap_err();
            let name_created = fs::read_dir(&fixture.root).unwrap().any(|entry| {
                entry.unwrap().file_name().as_bytes() == non_utf8.file_name().unwrap().as_bytes()
            });
            let after = fs::metadata(&path).unwrap();
            fs::write(
                fixture.root.join("non-utf8-name-refusal.json"),
                serde_json::to_vec(&serde_json::json!({
                    "io_kind": format!("{:?}", refused.kind()),
                    "raw_os_error": refused.raw_os_error(),
                    "name_created": name_created,
                    "source_before": [before.dev(), before.ino(), before.nlink()],
                    "source_after": [after.dev(), after.ino(), after.nlink()]
                }))
                .unwrap(),
            )
            .unwrap();
            assert_eq!(refused.raw_os_error(), Some(libc::EILSEQ));
            assert!(!name_created);
            assert_eq!(
                (before.dev(), before.ino(), before.nlink()),
                (after.dev(), after.ino(), after.nlink())
            );
            assert_eq!(fs::read(&path).unwrap(), body);
        }
        assert!(
            !marker.exists(),
            "reading declarations must not execute a command"
        );
        fixture.finish();
    }

    #[test]
    fn actual_directory_declaration_is_refused_in_both_read_modes() {
        let fixture = Fixture::new("declaration-directory");
        let path = default_service_declaration_path(&fixture.root);
        fs::create_dir(&path).unwrap();
        assert!(matches!(
            read_service_declarations(&path),
            Err(WorkcellError::InvalidDemand(_))
        ));
        assert!(matches!(
            read_state_root_service_declarations(&fixture.root),
            Err(WorkcellError::InvalidDemand(_))
        ));
        assert!(fs::metadata(&path).unwrap().is_dir());
        assert_eq!(fs::read_dir(&path).unwrap().count(), 0);
        fixture.finish();
    }

    #[test]
    fn actual_fifo_declaration_is_refused_without_waiting_for_a_writer() {
        if !support::isolated(
            "service_declaration::native_file_tests::actual_fifo_declaration_is_refused_without_waiting_for_a_writer",
            Duration::from_secs(5),
        ) {
            return;
        }
        let fixture = Fixture::new("declaration-fifo");
        let path = default_service_declaration_path(&fixture.root);
        let name = std::ffi::CString::new(path.as_os_str().as_bytes()).unwrap();
        assert_eq!(unsafe { libc::mkfifo(name.as_ptr(), 0o600) }, 0);
        assert!(fs::metadata(&path).unwrap().file_type().is_fifo());
        assert!(matches!(
            read_service_declarations(&path),
            Err(WorkcellError::InvalidDemand(_))
        ));
        assert!(matches!(
            read_state_root_service_declarations(&fixture.root),
            Err(WorkcellError::InvalidDemand(_))
        ));
        assert!(fs::metadata(&path).unwrap().file_type().is_fifo());
        fixture.finish();
    }

    #[test]
    fn actual_nondirectory_parent_is_unavailable_not_optional_absence() {
        let fixture = Fixture::new("declaration-enotdir");
        let parent = fixture.root.join("regular-parent");
        let body = b"actual regular parent remains unchanged";
        fs::write(&parent, body).unwrap();
        let path = default_service_declaration_path(&parent);
        let observed = fs::metadata(&path).unwrap_err();
        assert_eq!(observed.raw_os_error(), Some(libc::ENOTDIR));
        assert_ne!(observed.kind(), std::io::ErrorKind::NotFound);
        assert_actual_io(read_service_declarations(&path).unwrap_err(), &observed);
        assert_actual_io(
            read_state_root_service_declarations(&parent).unwrap_err(),
            &observed,
        );
        assert_eq!(fs::read(&parent).unwrap(), body);
        fixture.finish();
    }

    #[test]
    fn actual_denied_declaration_preserves_actual_permission_failure() {
        assert_ne!(
            unsafe { libc::geteuid() },
            0,
            "real permission refusal requires the supported nonroot test host"
        );
        let fixture = Fixture::new("declaration-eacces");
        let path = default_service_declaration_path(&fixture.root);
        let body = declaration(&fixture.root.join("must-not-execute"));
        fs::write(&path, &body).unwrap();
        struct RestorePermissions {
            path: PathBuf,
            original: fs::Permissions,
        }
        impl Drop for RestorePermissions {
            fn drop(&mut self) {
                fs::set_permissions(&self.path, self.original.clone())
                    .expect("restore actual owned permission fixture");
            }
        }
        let restore = RestorePermissions {
            path: path.clone(),
            original: fs::metadata(&path).unwrap().permissions(),
        };
        fs::set_permissions(&path, fs::Permissions::from_mode(0o000)).unwrap();
        let observed = fs::read_to_string(&path).unwrap_err();
        assert_eq!(observed.kind(), std::io::ErrorKind::PermissionDenied);
        assert!(observed.raw_os_error().is_some());
        assert_actual_io(read_service_declarations(&path).unwrap_err(), &observed);
        assert_actual_io(
            read_state_root_service_declarations(&fixture.root).unwrap_err(),
            &observed,
        );
        drop(restore);
        assert_eq!(fs::read(&path).unwrap(), body);
        fixture.finish();
    }

    #[test]
    fn actual_invalid_utf8_read_is_not_relabelled_missing() {
        let fixture = Fixture::new("declaration-invalid-utf8");
        let path = default_service_declaration_path(&fixture.root);
        let body = b"{\xff}";
        fs::write(&path, body).unwrap();
        let observed = fs::read_to_string(&path).unwrap_err();
        assert_eq!(observed.kind(), std::io::ErrorKind::InvalidData);
        assert_actual_io(read_service_declarations(&path).unwrap_err(), &observed);
        assert_actual_io(
            read_state_root_service_declarations(&fixture.root).unwrap_err(),
            &observed,
        );
        assert_eq!(fs::read(&path).unwrap(), body);
        fixture.finish();
    }

    #[test]
    fn actual_local_composition_does_not_erase_inaccessible_declaration() {
        let fixture = Fixture::new("declaration-local-composition");
        let state = fixture.root.join("state");
        fs::create_dir(&state).unwrap();
        let path = default_service_declaration_path(&state);
        fs::create_dir(&path).unwrap();
        let config = || {
            CollapsedLocalConfig::new(
                WorkcellRef::new("workcell:actual-declaration-read").unwrap(),
                &state,
            )
        };
        assert!(matches!(
            CollapsedLocalWorkcell::new(config()),
            Err(WorkcellError::InvalidDemand(_))
        ));
        assert!(matches!(
            CollapsedLocalWorkcell::new(config().with_service_declaration_file(&path)),
            Err(WorkcellError::InvalidDemand(_))
        ));
        assert!(fs::metadata(&path).unwrap().is_dir());
        let reduced = CollapsedLocalWorkcell::new(config().without_service_declarations()).unwrap();
        drop(reduced);
        let absent = fixture.root.join("absent-state");
        let ordinary = CollapsedLocalWorkcell::new(CollapsedLocalConfig::new(
            WorkcellRef::new("workcell:actual-absent-declaration").unwrap(),
            &absent,
        ))
        .unwrap();
        drop(ordinary);
        assert!(!default_service_declaration_path(&absent).exists());
        assert!(fs::metadata(&path).unwrap().is_dir());
        fixture.finish();
    }
}
