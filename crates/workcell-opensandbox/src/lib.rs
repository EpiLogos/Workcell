use std::{
    collections::BTreeMap,
    env,
    io::{Read, Write},
    net::TcpStream,
};

use epilogos_workcell_core::{
    Availability, Capacity, CheckpointRequest, ExecutionMaterialRequest, ExecutionProvider,
    HealthState, LeaseRenewalRequest, MaterialCheckpoint, MaterialCheckpointProvider,
    MaterialCheckpointState, MaterialLease, MaterialLeaseProvider, OfferRef, OperationalOffer,
    ProviderAllocation, ProviderObservation, ProviderOperation, ProviderOperationResult,
    ProviderPort, ProviderPortKind, ProviderRef, ProviderReleaseResult, ReleaseDisposition,
    ResourceRequirement, Result, RetentionExpectation, WorkcellError,
};
use serde_json::{json, Map, Value};

/// Exact OpenSandbox source/spec revision inspected for this provider cut.
pub const OPENSANDBOX_SOURCE_REVISION: &str = "173a576d3afcd1fb9ab116b4c1353b2f4b0848d1";
pub const OPENSANDBOX_LIFECYCLE_SPEC_BLOB: &str = "8723921f2599e2e349428af3b864f30a5987e9a8";
pub const OPENSANDBOX_EXECD_SPEC_BLOB: &str = "ccfbce2d4330ec8a70ea359206fab17afa9e7a98";
pub const OPENSANDBOX_LIFECYCLE_API_VERSION: &str = "0.1.0";
pub const OPENSANDBOX_EXECD_PORT: u16 = 44772;
pub const OPENSANDBOX_API_KEY_HEADER: &str = "OPEN-SANDBOX-API-KEY";
pub const OPENSANDBOX_DEFAULT_API_KEY_ENV: &str = "OPEN_SANDBOX_API_KEY";

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum OpenSandboxStartupSource {
    Image { uri: String },
    Snapshot { snapshot_id: String },
}

impl OpenSandboxStartupSource {
    fn validate(&self) -> Result<()> {
        let value = match self {
            Self::Image { uri } => uri,
            Self::Snapshot { snapshot_id } => snapshot_id,
        };
        if value.trim().is_empty() {
            return Err(WorkcellError::InvalidDemand(
                "OpenSandbox startup source must not be empty".into(),
            ));
        }
        Ok(())
    }
}

/// Provider-local OpenSandbox materialisation configuration.
///
/// Image/snapshot/runtime details remain here, below ExecutionDemand. The API
/// key is referenced by environment-variable name and is read only at request
/// time; the raw credential is never stored in Workcell material receipts.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct OpenSandboxConfig {
    pub provider_ref: ProviderRef,
    pub lifecycle_base_url: String,
    pub startup: OpenSandboxStartupSource,
    pub entrypoint: Vec<String>,
    pub timeout_seconds: Option<u64>,
    pub environment: BTreeMap<String, String>,
    pub metadata: BTreeMap<String, String>,
    pub use_server_proxy: bool,
    pub api_key_env: Option<String>,
    pub capacity: BTreeMap<String, Capacity>,
    pub isolation_offers: Vec<String>,
}

impl OpenSandboxConfig {
    pub fn local(
        provider_ref: ProviderRef,
        image_uri: impl Into<String>,
        entrypoint: Vec<String>,
    ) -> Result<Self> {
        let config = Self {
            provider_ref,
            lifecycle_base_url: "http://127.0.0.1:8080/v1".into(),
            startup: OpenSandboxStartupSource::Image {
                uri: image_uri.into(),
            },
            entrypoint,
            timeout_seconds: Some(3600),
            environment: BTreeMap::new(),
            metadata: BTreeMap::new(),
            use_server_proxy: false,
            api_key_env: Some(OPENSANDBOX_DEFAULT_API_KEY_ENV.into()),
            capacity: BTreeMap::new(),
            isolation_offers: vec!["sandbox".into()],
        };
        config.validate()?;
        Ok(config)
    }

    pub fn from_snapshot(
        provider_ref: ProviderRef,
        lifecycle_base_url: impl Into<String>,
        checkpoint_ref: impl Into<String>,
    ) -> Result<Self> {
        let config = Self {
            provider_ref,
            lifecycle_base_url: lifecycle_base_url.into(),
            startup: OpenSandboxStartupSource::Snapshot {
                snapshot_id: checkpoint_ref.into(),
            },
            entrypoint: Vec::new(),
            timeout_seconds: Some(3600),
            environment: BTreeMap::new(),
            metadata: BTreeMap::new(),
            use_server_proxy: false,
            api_key_env: Some(OPENSANDBOX_DEFAULT_API_KEY_ENV.into()),
            capacity: BTreeMap::new(),
            isolation_offers: vec!["sandbox".into()],
        };
        config.validate()?;
        Ok(config)
    }

    pub fn validate(&self) -> Result<()> {
        self.startup.validate()?;
        parse_http_url(&self.lifecycle_base_url)?;
        if matches!(self.startup, OpenSandboxStartupSource::Image { .. })
            && (self.entrypoint.is_empty()
                || self.entrypoint.iter().any(|item| item.trim().is_empty()))
        {
            return Err(WorkcellError::InvalidDemand(
                "OpenSandbox image startup requires a non-empty entrypoint".into(),
            ));
        }
        if self
            .api_key_env
            .as_deref()
            .is_some_and(|name| name.trim().is_empty())
        {
            return Err(WorkcellError::InvalidDemand(
                "OpenSandbox API-key environment name must not be empty".into(),
            ));
        }
        if self
            .metadata
            .keys()
            .chain(self.environment.keys())
            .any(|key| key.trim().is_empty())
        {
            return Err(WorkcellError::InvalidDemand(
                "OpenSandbox environment/metadata keys must not be empty".into(),
            ));
        }
        Ok(())
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct OpenSandboxHttpRequest {
    pub method: String,
    pub url: String,
    pub headers: BTreeMap<String, String>,
    pub body: Vec<u8>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct OpenSandboxHttpResponse {
    pub status: u16,
    pub headers: BTreeMap<String, String>,
    pub body: Vec<u8>,
}

/// Injectable protocol transport. Tests and future TLS-capable transports can
/// implement this seam without changing OpenSandbox/Workcell semantics.
pub trait OpenSandboxTransport {
    fn request(&self, request: OpenSandboxHttpRequest) -> Result<OpenSandboxHttpResponse>;
}

/// Lean HTTP/1.1 transport for the reference local OpenSandbox server.
///
/// It deliberately accepts `http://` only. Remote TLS is a transport concern:
/// use another implementation rather than weakening the provider contract or
/// shelling out to the `osb` CLI.
#[derive(Clone, Debug, Default)]
pub struct StdHttpOpenSandboxTransport;

impl OpenSandboxTransport for StdHttpOpenSandboxTransport {
    fn request(&self, request: OpenSandboxHttpRequest) -> Result<OpenSandboxHttpResponse> {
        let parsed = parse_http_url(&request.url)?;
        let mut stream =
            TcpStream::connect((parsed.host.as_str(), parsed.port)).map_err(|error| {
                WorkcellError::Unavailable(format!(
                    "connect OpenSandbox HTTP endpoint {}:{}: {error}",
                    parsed.host, parsed.port
                ))
            })?;
        let mut head = format!(
            "{} {} HTTP/1.1\r\nHost: {}\r\nConnection: close\r\n",
            request.method, parsed.path_and_query, parsed.host
        );
        for (name, value) in &request.headers {
            head.push_str(name);
            head.push_str(": ");
            head.push_str(value);
            head.push_str("\r\n");
        }
        if !request.body.is_empty() {
            head.push_str("Content-Type: application/json\r\n");
        }
        head.push_str(&format!("Content-Length: {}\r\n\r\n", request.body.len()));
        stream
            .write_all(head.as_bytes())
            .and_then(|_| stream.write_all(&request.body))
            .map_err(|error| {
                WorkcellError::OperationFailed(format!("write OpenSandbox HTTP request: {error}"))
            })?;

        let mut bytes = Vec::new();
        stream.read_to_end(&mut bytes).map_err(|error| {
            WorkcellError::OperationFailed(format!("read OpenSandbox HTTP response: {error}"))
        })?;
        parse_http_response(&bytes)
    }
}

pub struct OpenSandboxExecutionProvider<T> {
    config: OpenSandboxConfig,
    transport: T,
}

impl<T> OpenSandboxExecutionProvider<T>
where
    T: OpenSandboxTransport,
{
    pub fn new(config: OpenSandboxConfig, transport: T) -> Result<Self> {
        config.validate()?;
        Ok(Self { config, transport })
    }

    pub fn config(&self) -> &OpenSandboxConfig {
        &self.config
    }

    /// Resolve a provider-native service endpoint without turning Workcell into
    /// the application data plane. Header values are intentionally retained
    /// only inside this provider object and are not returned by this public
    /// reading; callers receive the URL plus required header names.
    pub fn endpoint_reading(
        &self,
        allocation: &ProviderAllocation,
        port: u16,
    ) -> Result<OpenSandboxEndpointReading> {
        self.require_allocation(allocation)?;
        let endpoint = self.resolve_endpoint_secret(allocation, port)?;
        Ok(OpenSandboxEndpointReading {
            endpoint: endpoint.endpoint,
            required_header_names: endpoint.headers.keys().cloned().collect(),
            provenance: provider_provenance(),
        })
    }

    fn require_allocation(&self, allocation: &ProviderAllocation) -> Result<()> {
        if allocation.provider_ref != self.config.provider_ref
            || allocation.port != ProviderPortKind::Execution
            || allocation.material_ref.trim().is_empty()
        {
            return Err(WorkcellError::OperationFailed(
                "OpenSandbox allocation escaped its provider/material identity".into(),
            ));
        }
        Ok(())
    }

    fn lifecycle_request(
        &self,
        method: &str,
        path: &str,
        body: Option<Value>,
    ) -> Result<OpenSandboxHttpResponse> {
        let mut headers = BTreeMap::new();
        if let Some(env_name) = &self.config.api_key_env {
            let value = env::var(env_name).map_err(|_| {
                WorkcellError::Unavailable(format!(
                    "OpenSandbox API key environment `{env_name}` is not available"
                ))
            })?;
            headers.insert(OPENSANDBOX_API_KEY_HEADER.into(), value);
        }
        let body = match body {
            Some(value) => serde_json::to_vec(&value).map_err(|error| {
                WorkcellError::OperationFailed(format!(
                    "encode OpenSandbox lifecycle request: {error}"
                ))
            })?,
            None => Vec::new(),
        };
        self.transport.request(OpenSandboxHttpRequest {
            method: method.into(),
            url: join_url(&self.config.lifecycle_base_url, path),
            headers,
            body,
        })
    }

    fn lifecycle_json(&self, method: &str, path: &str, body: Option<Value>) -> Result<Value> {
        let response = self.lifecycle_request(method, path, body)?;
        require_success(&response, path)?;
        if response.body.is_empty() {
            return Ok(Value::Null);
        }
        serde_json::from_slice(&response.body).map_err(|error| {
            WorkcellError::OperationFailed(format!(
                "decode OpenSandbox lifecycle response for `{path}`: {error}"
            ))
        })
    }

    fn resolve_endpoint_secret(
        &self,
        allocation: &ProviderAllocation,
        port: u16,
    ) -> Result<ResolvedEndpoint> {
        let path = format!(
            "/sandboxes/{}/endpoints/{port}?use_server_proxy={}",
            allocation.material_ref, self.config.use_server_proxy
        );
        let value = self.lifecycle_json("GET", &path, None)?;
        let object = value.as_object().ok_or_else(|| {
            WorkcellError::OperationFailed("OpenSandbox endpoint response must be an object".into())
        })?;
        let endpoint = string_field(object, "endpoint")?.to_owned();
        let headers = match object.get("headers") {
            None | Some(Value::Null) => BTreeMap::new(),
            Some(Value::Object(headers)) => headers
                .iter()
                .map(|(name, value)| {
                    value
                        .as_str()
                        .map(|value| (name.clone(), value.to_owned()))
                        .ok_or_else(|| {
                            WorkcellError::OperationFailed(
                                "OpenSandbox endpoint auth header values must be strings".into(),
                            )
                        })
                })
                .collect::<Result<BTreeMap<_, _>>>()?,
            Some(_) => {
                return Err(WorkcellError::OperationFailed(
                    "OpenSandbox endpoint headers must be an object".into(),
                ))
            }
        };
        Ok(ResolvedEndpoint { endpoint, headers })
    }

    fn run_command(
        &self,
        allocation: &ProviderAllocation,
        operation: &ProviderOperation,
    ) -> Result<ProviderOperationResult> {
        let command = operation
            .parameters
            .get("command")
            .map(String::as_str)
            .filter(|value| !value.trim().is_empty())
            .ok_or_else(|| {
                WorkcellError::InvalidDemand(
                    "OpenSandbox command operation requires non-empty `command`".into(),
                )
            })?;
        let endpoint = self.resolve_endpoint_secret(allocation, OPENSANDBOX_EXECD_PORT)?;
        let base = normalize_endpoint_url(&endpoint.endpoint);
        let mut body = Map::new();
        body.insert("command".into(), Value::String(command.to_owned()));
        body.insert("background".into(), Value::Bool(false));
        if let Some(cwd) = operation.parameters.get("cwd") {
            body.insert("cwd".into(), Value::String(cwd.clone()));
        }
        if let Some(timeout) = operation.parameters.get("timeout_ms") {
            let timeout = timeout.parse::<u64>().map_err(|_| {
                WorkcellError::InvalidDemand(
                    "OpenSandbox command timeout_ms must be an unsigned integer".into(),
                )
            })?;
            body.insert("timeout".into(), Value::Number(timeout.into()));
        }
        let payload = serde_json::to_vec(&Value::Object(body)).map_err(|error| {
            WorkcellError::OperationFailed(format!("encode OpenSandbox execd command: {error}"))
        })?;
        let mut headers = endpoint.headers;
        headers.insert("Accept".into(), "text/event-stream".into());
        let response = self.transport.request(OpenSandboxHttpRequest {
            method: "POST".into(),
            url: join_url(&base, "/command"),
            headers,
            body: payload,
        })?;
        require_success(&response, "execd /command")?;
        let stream = parse_execd_stream(&response.body)?;
        let mut output = BTreeMap::new();
        if let Some(id) = stream.execution_id {
            output.insert("execution_id".into(), id);
        }
        output.insert("stdout".into(), stream.stdout);
        output.insert("stderr".into(), stream.stderr);
        if !stream.results.is_empty() {
            output.insert("result".into(), stream.results.join("\n"));
        }
        if let Some(error) = stream.error {
            output.insert("error".into(), error);
        }
        if stream.complete {
            output.insert("complete".into(), "true".into());
        }
        Ok(ProviderOperationResult {
            provider_ref: self.config.provider_ref.clone(),
            material_ref: allocation.material_ref.clone(),
            operation: "command".into(),
            output,
            provenance: provider_provenance(),
        })
    }

    fn sandbox_observation(&self, allocation: &ProviderAllocation) -> Result<ProviderObservation> {
        self.require_allocation(allocation)?;
        let path = format!("/sandboxes/{}", allocation.material_ref);
        let value = self.lifecycle_json("GET", &path, None)?;
        let object = value.as_object().ok_or_else(|| {
            WorkcellError::OperationFailed("OpenSandbox sandbox response must be an object".into())
        })?;
        let state = sandbox_state(object);
        let mut detail = BTreeMap::new();
        detail.insert("provider_state".into(), state.clone());
        if let Some(expires_at) = object.get("expiresAt").and_then(Value::as_str) {
            detail.insert("lease_expires_at".into(), expires_at.into());
        }
        Ok(ProviderObservation {
            provider_ref: self.config.provider_ref.clone(),
            material_ref: allocation.material_ref.clone(),
            health: health_from_state(&state),
            detail,
        })
    }
}

impl<T> ProviderPort for OpenSandboxExecutionProvider<T>
where
    T: OpenSandboxTransport,
{
    fn provider_ref(&self) -> &ProviderRef {
        &self.config.provider_ref
    }

    fn port_kind(&self) -> ProviderPortKind {
        ProviderPortKind::Execution
    }

    fn offers(&self) -> Result<Vec<OperationalOffer>> {
        let probe = self.lifecycle_request("GET", "/sandboxes?page=1&pageSize=1", None);
        let (availability, health, diagnostic) = match probe {
            Ok(response) if (200..300).contains(&response.status) => {
                (Availability::Available, HealthState::Healthy, None)
            }
            Ok(response) => (
                Availability::Unavailable,
                HealthState::Unavailable,
                Some(format!("lifecycle probe returned HTTP {}", response.status)),
            ),
            Err(error) => (
                Availability::Unavailable,
                HealthState::Unavailable,
                Some(error.to_string()),
            ),
        };
        let mut metadata = provider_provenance();
        if let Some(diagnostic) = diagnostic {
            metadata.insert("observation_error".into(), diagnostic);
        }
        Ok(vec![OperationalOffer {
            offer_ref: OfferRef::new(format!("offer:{}:execution", self.config.provider_ref))?,
            provider_ref: self.config.provider_ref.clone(),
            port: ProviderPortKind::Execution.as_str().into(),
            affordances: vec![
                "shell".into(),
                "filesystem".into(),
                "persistence:ephemeral".into(),
                "persistence:task-or-run".into(),
                "retention:preserve".into(),
                "retention:suspend".into(),
                "retention:snapshot".into(),
            ],
            connections: Vec::new(),
            exposures: vec!["endpoint".into()],
            isolation_trust: self.config.isolation_offers.clone(),
            availability,
            health,
            capacity: self.config.capacity.clone(),
            metadata,
        }])
    }
}

impl<T> ExecutionProvider for OpenSandboxExecutionProvider<T>
where
    T: OpenSandboxTransport,
{
    fn prepare_execution(
        &mut self,
        request: &ExecutionMaterialRequest,
    ) -> Result<ProviderAllocation> {
        let body = create_sandbox_body(&self.config, request)?;
        let value = self.lifecycle_json("POST", "/sandboxes", Some(body))?;
        let object = value.as_object().ok_or_else(|| {
            WorkcellError::OperationFailed("OpenSandbox create response must be an object".into())
        })?;
        let material_ref = string_field(object, "id")?.to_owned();
        let state = sandbox_state(object);
        let mut properties = BTreeMap::new();
        properties.insert("provider_state".into(), state.clone());
        properties.insert("execd_port".into(), OPENSANDBOX_EXECD_PORT.to_string());
        if let Some(expires_at) = object.get("expiresAt").and_then(Value::as_str) {
            properties.insert("lease_expires_at".into(), expires_at.into());
        }
        Ok(ProviderAllocation {
            provider_ref: self.config.provider_ref.clone(),
            port: ProviderPortKind::Execution,
            material_ref,
            health: health_from_state(&state),
            properties,
            provenance: provider_provenance(),
        })
    }

    fn execute_operation(
        &mut self,
        allocation: &ProviderAllocation,
        operation: &ProviderOperation,
    ) -> Result<ProviderOperationResult> {
        self.require_allocation(allocation)?;
        match operation.key.as_str() {
            "command" => self.run_command(allocation, operation),
            other => Err(WorkcellError::Unsupported(format!(
                "OpenSandbox execution operation `{other}` is not implemented; native execd remains the data-plane authority"
            ))),
        }
    }

    fn observe_execution(&self, allocation: &ProviderAllocation) -> Result<ProviderObservation> {
        self.sandbox_observation(allocation)
    }

    fn release_execution(
        &mut self,
        allocation: &ProviderAllocation,
        retention: &RetentionExpectation,
    ) -> Result<ProviderReleaseResult> {
        self.require_allocation(allocation)?;
        match retention {
            RetentionExpectation::Release => {
                let path = format!("/sandboxes/{}", allocation.material_ref);
                let response = self.lifecycle_request("DELETE", &path, None)?;
                if response.status != 404 {
                    require_success(&response, &path)?;
                }
                Ok(ProviderReleaseResult {
                    provider_ref: self.config.provider_ref.clone(),
                    material_ref: allocation.material_ref.clone(),
                    disposition: ReleaseDisposition::Released,
                    changed: response.status != 404,
                })
            }
            RetentionExpectation::Preserve => Ok(ProviderReleaseResult {
                provider_ref: self.config.provider_ref.clone(),
                material_ref: allocation.material_ref.clone(),
                disposition: ReleaseDisposition::Preserved,
                changed: false,
            }),
            RetentionExpectation::SuspendIfSupported => {
                let path = format!("/sandboxes/{}/pause", allocation.material_ref);
                let response = self.lifecycle_request("POST", &path, None)?;
                require_success(&response, &path)?;
                Ok(ProviderReleaseResult {
                    provider_ref: self.config.provider_ref.clone(),
                    material_ref: allocation.material_ref.clone(),
                    disposition: ReleaseDisposition::Suspended,
                    changed: true,
                })
            }
            RetentionExpectation::SnapshotIfSupported => {
                let checkpoint = self.checkpoint(allocation, &CheckpointRequest::default())?;
                Ok(ProviderReleaseResult {
                    provider_ref: self.config.provider_ref.clone(),
                    material_ref: allocation.material_ref.clone(),
                    disposition: ReleaseDisposition::Snapshotted,
                    changed: checkpoint.state != MaterialCheckpointState::Failed,
                })
            }
        }
    }
}

impl<T> MaterialLeaseProvider for OpenSandboxExecutionProvider<T>
where
    T: OpenSandboxTransport,
{
    fn observe_lease(&self, allocation: &ProviderAllocation) -> Result<Option<MaterialLease>> {
        let observation = self.sandbox_observation(allocation)?;
        let Some(expires_at) = observation.detail.get("lease_expires_at") else {
            return Ok(None);
        };
        Ok(Some(MaterialLease {
            provider_ref: self.config.provider_ref.clone(),
            material_ref: allocation.material_ref.clone(),
            expires_at: expires_at.clone(),
            renewable: true,
            provenance: provider_provenance(),
        }))
    }

    fn renew_lease(
        &mut self,
        allocation: &ProviderAllocation,
        request: &LeaseRenewalRequest,
    ) -> Result<MaterialLease> {
        self.require_allocation(allocation)?;
        if request.expires_at.trim().is_empty() {
            return Err(WorkcellError::InvalidDemand(
                "lease renewal expires_at must not be empty".into(),
            ));
        }
        let path = format!("/sandboxes/{}/renew-expiration", allocation.material_ref);
        let value = self.lifecycle_json(
            "POST",
            &path,
            Some(json!({"expiresAt": request.expires_at})),
        )?;
        let object = value.as_object().ok_or_else(|| {
            WorkcellError::OperationFailed("OpenSandbox renewal response must be an object".into())
        })?;
        Ok(MaterialLease {
            provider_ref: self.config.provider_ref.clone(),
            material_ref: allocation.material_ref.clone(),
            expires_at: string_field(object, "expiresAt")?.to_owned(),
            renewable: true,
            provenance: provider_provenance(),
        })
    }
}

impl<T> MaterialCheckpointProvider for OpenSandboxExecutionProvider<T>
where
    T: OpenSandboxTransport,
{
    fn checkpoint(
        &mut self,
        allocation: &ProviderAllocation,
        request: &CheckpointRequest,
    ) -> Result<MaterialCheckpoint> {
        self.require_allocation(allocation)?;
        let path = format!("/sandboxes/{}/snapshots", allocation.material_ref);
        let body = request
            .name
            .as_ref()
            .map(|name| json!({"name": name}))
            .unwrap_or_else(|| json!({}));
        let value = self.lifecycle_json("POST", &path, Some(body))?;
        checkpoint_from_value(&self.config.provider_ref, &allocation.material_ref, &value)
    }

    fn observe_checkpoint(&self, checkpoint: &MaterialCheckpoint) -> Result<MaterialCheckpoint> {
        if checkpoint.provider_ref != self.config.provider_ref {
            return Err(WorkcellError::OperationFailed(
                "checkpoint belongs to a different provider".into(),
            ));
        }
        let path = format!("/snapshots/{}", checkpoint.checkpoint_ref);
        let value = self.lifecycle_json("GET", &path, None)?;
        checkpoint_from_value(
            &self.config.provider_ref,
            &checkpoint.source_material_ref,
            &value,
        )
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct OpenSandboxEndpointReading {
    pub endpoint: String,
    pub required_header_names: Vec<String>,
    pub provenance: BTreeMap<String, String>,
}

struct ResolvedEndpoint {
    endpoint: String,
    headers: BTreeMap<String, String>,
}

fn create_sandbox_body(
    config: &OpenSandboxConfig,
    request: &ExecutionMaterialRequest,
) -> Result<Value> {
    let mut body = Map::new();
    match &config.startup {
        OpenSandboxStartupSource::Image { uri } => {
            body.insert("image".into(), json!({"uri": uri}));
        }
        OpenSandboxStartupSource::Snapshot { snapshot_id } => {
            body.insert("snapshotId".into(), Value::String(snapshot_id.clone()));
        }
    }
    if !config.entrypoint.is_empty() {
        body.insert("entrypoint".into(), json!(config.entrypoint));
    }
    if let Some(timeout) = config.timeout_seconds {
        body.insert("timeout".into(), Value::Number(timeout.into()));
    }
    if !config.environment.is_empty() {
        body.insert("env".into(), json!(config.environment));
    }
    let mut metadata = config.metadata.clone();
    metadata.insert("workcell.demand_ref".into(), request.demand_ref.to_string());
    body.insert("metadata".into(), json!(metadata));

    let limits = resource_limits(&request.resources)?;
    if !limits.is_empty() {
        body.insert("resourceLimits".into(), Value::Object(limits));
    }
    Ok(Value::Object(body))
}

fn resource_limits(resources: &[ResourceRequirement]) -> Result<Map<String, Value>> {
    let mut out = Map::new();
    for resource in resources {
        let Some(minimum) = resource.minimum else {
            return Err(WorkcellError::InvalidDemand(format!(
                "OpenSandbox resource `{}` requires an explicit minimum",
                resource.key
            )));
        };
        let value = match resource.key.as_str() {
            "cpu" => resource_quantity(minimum, resource.unit.as_deref(), "cpu")?,
            "memory" => resource_quantity(minimum, resource.unit.as_deref(), "memory")?,
            "gpu" => resource_quantity(minimum, resource.unit.as_deref(), "gpu")?,
            other => {
                return Err(WorkcellError::Unsupported(format!(
                    "OpenSandbox adapter does not map Workcell resource `{other}` to the pinned lifecycle protocol"
                )))
            }
        };
        out.insert(resource.key.clone(), Value::String(value));
    }
    Ok(out)
}

fn resource_quantity(minimum: u64, unit: Option<&str>, label: &str) -> Result<String> {
    match unit {
        None | Some("count") => Ok(minimum.to_string()),
        Some("m") if label == "cpu" => Ok(format!("{minimum}m")),
        Some("MiB") if label == "memory" => Ok(format!("{minimum}Mi")),
        Some("GiB") if label == "memory" => Ok(format!("{minimum}Gi")),
        Some(other) => Err(WorkcellError::Unsupported(format!(
            "OpenSandbox adapter does not map `{label}` unit `{other}`"
        ))),
    }
}

fn checkpoint_from_value(
    provider_ref: &ProviderRef,
    source_material_ref: &str,
    value: &Value,
) -> Result<MaterialCheckpoint> {
    let object = value.as_object().ok_or_else(|| {
        WorkcellError::OperationFailed("OpenSandbox snapshot response must be an object".into())
    })?;
    let checkpoint_ref = string_field(object, "id")?.to_owned();
    let state_value = object
        .get("status")
        .and_then(Value::as_object)
        .and_then(|status| status.get("state"))
        .and_then(Value::as_str)
        .unwrap_or("Unknown");
    let state = match state_value.to_ascii_lowercase().as_str() {
        "creating" => MaterialCheckpointState::Creating,
        "ready" => MaterialCheckpointState::Ready,
        "failed" => MaterialCheckpointState::Failed,
        _ => MaterialCheckpointState::Unknown,
    };
    let mut provenance = provider_provenance();
    provenance.insert("provider_snapshot_state".into(), state_value.into());
    Ok(MaterialCheckpoint {
        provider_ref: provider_ref.clone(),
        source_material_ref: source_material_ref.into(),
        checkpoint_ref,
        state,
        reusable: state != MaterialCheckpointState::Failed,
        provenance,
    })
}

fn sandbox_state(object: &Map<String, Value>) -> String {
    object
        .get("status")
        .and_then(Value::as_object)
        .and_then(|status| status.get("state"))
        .and_then(Value::as_str)
        .unwrap_or("Unknown")
        .to_owned()
}

fn health_from_state(state: &str) -> HealthState {
    match state.to_ascii_lowercase().as_str() {
        "running" | "paused" => HealthState::Healthy,
        "creating" | "pending" | "pausing" | "resuming" | "stopping" => HealthState::Degraded,
        "terminated" | "failed" => HealthState::Unavailable,
        _ => HealthState::Unknown,
    }
}

fn provider_provenance() -> BTreeMap<String, String> {
    BTreeMap::from([
        ("provider".into(), "opensandbox".into()),
        (
            "upstream.repository".into(),
            "opensandbox-group/OpenSandbox".into(),
        ),
        (
            "upstream.revision".into(),
            OPENSANDBOX_SOURCE_REVISION.into(),
        ),
        (
            "upstream.lifecycle_spec_blob".into(),
            OPENSANDBOX_LIFECYCLE_SPEC_BLOB.into(),
        ),
        (
            "upstream.execd_spec_blob".into(),
            OPENSANDBOX_EXECD_SPEC_BLOB.into(),
        ),
        (
            "upstream.lifecycle_api".into(),
            OPENSANDBOX_LIFECYCLE_API_VERSION.into(),
        ),
    ])
}

fn require_success(response: &OpenSandboxHttpResponse, operation: &str) -> Result<()> {
    if (200..300).contains(&response.status) {
        return Ok(());
    }
    let detail = String::from_utf8_lossy(&response.body);
    let safe_detail = if detail.len() > 512 {
        format!("{}…", &detail[..512])
    } else {
        detail.into_owned()
    };
    let message = format!(
        "OpenSandbox `{operation}` returned HTTP {}{}",
        response.status,
        if safe_detail.trim().is_empty() {
            String::new()
        } else {
            format!(": {safe_detail}")
        }
    );
    match response.status {
        404 => Err(WorkcellError::NotFound(message)),
        409 | 429 => Err(WorkcellError::Unavailable(message)),
        400 | 401 | 403 => Err(WorkcellError::OperationFailed(message)),
        _ => Err(WorkcellError::OperationFailed(message)),
    }
}

fn string_field<'a>(object: &'a Map<String, Value>, field: &str) -> Result<&'a str> {
    object.get(field).and_then(Value::as_str).ok_or_else(|| {
        WorkcellError::OperationFailed(format!(
            "OpenSandbox response field `{field}` must be a string"
        ))
    })
}

#[derive(Default)]
struct ExecdStream {
    execution_id: Option<String>,
    stdout: String,
    stderr: String,
    results: Vec<String>,
    error: Option<String>,
    complete: bool,
}

fn parse_execd_stream(bytes: &[u8]) -> Result<ExecdStream> {
    let text = std::str::from_utf8(bytes).map_err(|error| {
        WorkcellError::OperationFailed(format!("OpenSandbox execd SSE is not UTF-8: {error}"))
    })?;
    let mut stream = ExecdStream::default();
    for line in text.lines() {
        let Some(data) = line.strip_prefix("data:") else {
            continue;
        };
        let data = data.trim();
        if data.is_empty() || data == "[DONE]" {
            continue;
        }
        let event: Value = serde_json::from_str(data).map_err(|error| {
            WorkcellError::OperationFailed(format!("decode OpenSandbox execd SSE event: {error}"))
        })?;
        let Some(object) = event.as_object() else {
            continue;
        };
        match object.get("type").and_then(Value::as_str).unwrap_or("") {
            "init" => {
                if let Some(id) = object.get("text").and_then(Value::as_str) {
                    if !id.is_empty() {
                        stream.execution_id = Some(id.into());
                    }
                }
            }
            "stdout" => {
                if let Some(value) = object.get("text").and_then(Value::as_str) {
                    stream.stdout.push_str(value);
                }
            }
            "stderr" => {
                if let Some(value) = object.get("text").and_then(Value::as_str) {
                    stream.stderr.push_str(value);
                }
            }
            "result" => {
                if let Some(results) = object.get("results").and_then(Value::as_object) {
                    if let Some(value) = results
                        .get("text/plain")
                        .or_else(|| results.get("text"))
                        .and_then(Value::as_str)
                    {
                        stream.results.push(value.into());
                    }
                }
            }
            "execution_complete" => stream.complete = true,
            "error" => {
                if let Some(error) = object.get("error").and_then(Value::as_object) {
                    let name = error
                        .get("ename")
                        .or_else(|| error.get("name"))
                        .and_then(Value::as_str)
                        .unwrap_or("error");
                    let value = error
                        .get("evalue")
                        .or_else(|| error.get("value"))
                        .and_then(Value::as_str)
                        .unwrap_or("");
                    stream.error = Some(format!("{name}: {value}"));
                }
            }
            _ => {}
        }
    }
    Ok(stream)
}

#[derive(Clone, Debug)]
struct ParsedHttpUrl {
    host: String,
    port: u16,
    path_and_query: String,
}

fn parse_http_url(url: &str) -> Result<ParsedHttpUrl> {
    let remainder = url.strip_prefix("http://").ok_or_else(|| {
        WorkcellError::Unsupported(format!(
            "StdHttpOpenSandboxTransport supports local/plain HTTP only; provide another transport for `{url}`"
        ))
    })?;
    let (authority, path) = remainder
        .split_once('/')
        .map(|(authority, path)| (authority, format!("/{path}")))
        .unwrap_or((remainder, "/".into()));
    if authority.trim().is_empty() {
        return Err(WorkcellError::InvalidDemand(
            "OpenSandbox HTTP URL host must not be empty".into(),
        ));
    }
    let (host, port) = match authority.rsplit_once(':') {
        Some((host, port)) if !host.contains(']') => {
            let port = port.parse::<u16>().map_err(|_| {
                WorkcellError::InvalidDemand(format!(
                    "OpenSandbox HTTP URL has invalid port `{port}`"
                ))
            })?;
            (host.to_owned(), port)
        }
        _ => (authority.to_owned(), 80),
    };
    Ok(ParsedHttpUrl {
        host,
        port,
        path_and_query: path,
    })
}

fn join_url(base: &str, path: &str) -> String {
    format!(
        "{}/{}",
        base.trim_end_matches('/'),
        path.trim_start_matches('/')
    )
}

fn normalize_endpoint_url(endpoint: &str) -> String {
    if endpoint.starts_with("http://") || endpoint.starts_with("https://") {
        endpoint.trim_end_matches('/').into()
    } else {
        format!("http://{}", endpoint.trim_end_matches('/'))
    }
}

fn parse_http_response(bytes: &[u8]) -> Result<OpenSandboxHttpResponse> {
    let split = bytes
        .windows(4)
        .position(|window| window == b"\r\n\r\n")
        .ok_or_else(|| {
            WorkcellError::OperationFailed(
                "OpenSandbox HTTP response has no header boundary".into(),
            )
        })?;
    let head = std::str::from_utf8(&bytes[..split]).map_err(|error| {
        WorkcellError::OperationFailed(format!("OpenSandbox HTTP headers are not UTF-8: {error}"))
    })?;
    let mut lines = head.split("\r\n");
    let status_line = lines.next().ok_or_else(|| {
        WorkcellError::OperationFailed("OpenSandbox HTTP response has no status line".into())
    })?;
    let status = status_line
        .split_whitespace()
        .nth(1)
        .ok_or_else(|| {
            WorkcellError::OperationFailed("OpenSandbox HTTP status line is invalid".into())
        })?
        .parse::<u16>()
        .map_err(|_| WorkcellError::OperationFailed("OpenSandbox HTTP status is invalid".into()))?;
    let mut headers = BTreeMap::new();
    for line in lines {
        if let Some((name, value)) = line.split_once(':') {
            headers.insert(name.trim().to_ascii_lowercase(), value.trim().to_owned());
        }
    }
    let raw_body = &bytes[split + 4..];
    let body = if headers
        .get("transfer-encoding")
        .is_some_and(|value| value.eq_ignore_ascii_case("chunked"))
    {
        decode_chunked(raw_body)?
    } else {
        raw_body.to_vec()
    };
    Ok(OpenSandboxHttpResponse {
        status,
        headers,
        body,
    })
}

fn decode_chunked(mut input: &[u8]) -> Result<Vec<u8>> {
    let mut output = Vec::new();
    loop {
        let line_end = input
            .windows(2)
            .position(|window| window == b"\r\n")
            .ok_or_else(|| {
                WorkcellError::OperationFailed("invalid chunked OpenSandbox response".into())
            })?;
        let size_text = std::str::from_utf8(&input[..line_end])
            .map_err(|_| WorkcellError::OperationFailed("invalid HTTP chunk size".into()))?;
        let size_text = size_text.split(';').next().unwrap_or(size_text).trim();
        let size = usize::from_str_radix(size_text, 16)
            .map_err(|_| WorkcellError::OperationFailed("invalid HTTP chunk size".into()))?;
        input = &input[line_end + 2..];
        if size == 0 {
            break;
        }
        if input.len() < size + 2 {
            return Err(WorkcellError::OperationFailed(
                "truncated chunked OpenSandbox response".into(),
            ));
        }
        output.extend_from_slice(&input[..size]);
        input = &input[size + 2..];
    }
    Ok(output)
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Mutex};

    use super::*;
    use epilogos_workcell_core::{DemandRef, IsolationTrustRequirement};

    #[derive(Clone, Default)]
    struct FixtureTransport {
        requests: Arc<Mutex<Vec<OpenSandboxHttpRequest>>>,
        responses: Arc<Mutex<Vec<OpenSandboxHttpResponse>>>,
    }

    impl FixtureTransport {
        fn with_responses(responses: Vec<OpenSandboxHttpResponse>) -> Self {
            Self {
                requests: Arc::new(Mutex::new(Vec::new())),
                responses: Arc::new(Mutex::new(responses.into_iter().rev().collect())),
            }
        }

        fn requests(&self) -> Vec<OpenSandboxHttpRequest> {
            self.requests.lock().unwrap().clone()
        }
    }

    impl OpenSandboxTransport for FixtureTransport {
        fn request(&self, request: OpenSandboxHttpRequest) -> Result<OpenSandboxHttpResponse> {
            self.requests.lock().unwrap().push(request);
            self.responses
                .lock()
                .unwrap()
                .pop()
                .ok_or_else(|| WorkcellError::OperationFailed("fixture response exhausted".into()))
        }
    }

    fn response(status: u16, value: Value) -> OpenSandboxHttpResponse {
        OpenSandboxHttpResponse {
            status,
            headers: BTreeMap::new(),
            body: if value.is_null() {
                Vec::new()
            } else {
                serde_json::to_vec(&value).unwrap()
            },
        }
    }

    fn config() -> OpenSandboxConfig {
        let mut config = OpenSandboxConfig::local(
            ProviderRef::new("provider:opensandbox").unwrap(),
            "opensandbox/code-interpreter:v1.1.0",
            vec!["/opt/code-interpreter/code-interpreter.sh".into()],
        )
        .unwrap();
        config.api_key_env = None;
        config.capacity.insert(
            "memory".into(),
            Capacity {
                amount: 16,
                unit: Some("GiB".into()),
            },
        );
        config
    }

    fn request() -> ExecutionMaterialRequest {
        ExecutionMaterialRequest {
            demand_ref: DemandRef::new("demand:project-world").unwrap(),
            affordances: vec!["shell".into()],
            resources: vec![ResourceRequirement {
                key: "memory".into(),
                minimum: Some(2),
                unit: Some("GiB".into()),
            }],
            connectivity: Vec::new(),
            isolation_trust: Some(IsolationTrustRequirement::new("sandbox").unwrap()),
            retention: RetentionExpectation::SnapshotIfSupported,
        }
    }

    #[test]
    fn create_is_protocol_first_and_preserves_provider_provenance() {
        let transport = FixtureTransport::with_responses(vec![response(
            202,
            json!({
                "id": "sbx_123",
                "status": {"state": "Running"},
                "expiresAt": "2026-09-01T12:00:00Z"
            }),
        )]);
        let inspect = transport.clone();
        let mut provider = OpenSandboxExecutionProvider::new(config(), transport).unwrap();
        let allocation = provider.prepare_execution(&request()).unwrap();
        assert_eq!(allocation.material_ref, "sbx_123");
        assert_eq!(allocation.port, ProviderPortKind::Execution);
        assert_eq!(
            allocation
                .provenance
                .get("upstream.revision")
                .map(String::as_str),
            Some(OPENSANDBOX_SOURCE_REVISION)
        );
        let requests = inspect.requests();
        assert_eq!(requests.len(), 1);
        assert_eq!(requests[0].method, "POST");
        assert!(requests[0].url.ends_with("/v1/sandboxes"));
        let body: Value = serde_json::from_slice(&requests[0].body).unwrap();
        assert_eq!(body["image"]["uri"], "opensandbox/code-interpreter:v1.1.0");
        assert_eq!(body["resourceLimits"]["memory"], "2Gi");
        assert_eq!(
            body["metadata"]["workcell.demand_ref"],
            "demand:project-world"
        );
    }

    #[test]
    fn lease_is_observed_and_renewed_without_becoming_semantic_identity() {
        let transport = FixtureTransport::with_responses(vec![
            response(
                200,
                json!({
                    "id":"sbx_lease",
                    "status":{"state":"Running"},
                    "expiresAt":"2026-09-01T12:00:00Z"
                }),
            ),
            response(200, json!({"expiresAt":"2026-09-01T14:00:00Z"})),
        ]);
        let mut provider = OpenSandboxExecutionProvider::new(config(), transport).unwrap();
        let allocation = ProviderAllocation {
            provider_ref: provider.provider_ref().clone(),
            port: ProviderPortKind::Execution,
            material_ref: "sbx_lease".into(),
            health: HealthState::Healthy,
            properties: BTreeMap::new(),
            provenance: BTreeMap::new(),
        };
        let lease = provider.observe_lease(&allocation).unwrap().unwrap();
        assert_eq!(lease.material_ref, "sbx_lease");
        let renewed = provider
            .renew_lease(
                &allocation,
                &LeaseRenewalRequest {
                    expires_at: "2026-09-01T14:00:00Z".into(),
                },
            )
            .unwrap();
        assert_eq!(renewed.expires_at, "2026-09-01T14:00:00Z");
    }

    #[test]
    fn snapshot_returns_reusable_material_checkpoint() {
        let transport = FixtureTransport::with_responses(vec![response(
            202,
            json!({"id":"snap_42","status":{"state":"Creating"}}),
        )]);
        let mut provider = OpenSandboxExecutionProvider::new(config(), transport).unwrap();
        let allocation = ProviderAllocation {
            provider_ref: provider.provider_ref().clone(),
            port: ProviderPortKind::Execution,
            material_ref: "sbx_snapshot".into(),
            health: HealthState::Healthy,
            properties: BTreeMap::new(),
            provenance: BTreeMap::new(),
        };
        let checkpoint = provider
            .checkpoint(
                &allocation,
                &CheckpointRequest {
                    name: Some("project-world".into()),
                },
            )
            .unwrap();
        assert_eq!(checkpoint.checkpoint_ref, "snap_42");
        assert_eq!(checkpoint.state, MaterialCheckpointState::Creating);
        assert!(checkpoint.reusable);
    }

    #[test]
    fn execd_command_uses_native_data_plane_and_does_not_return_secret_headers() {
        let sse = concat!(
            "data: {\"type\":\"init\",\"text\":\"exec_1\"}\n\n",
            "data: {\"type\":\"stdout\",\"text\":\"hello\\n\"}\n\n",
            "data: {\"type\":\"execution_complete\",\"execution_time\":1}\n\n"
        );
        let transport = FixtureTransport::with_responses(vec![
            response(
                200,
                json!({
                    "endpoint":"127.0.0.1:44772",
                    "headers":{"OpenSandbox-Secure-Access":"secret-route-token"}
                }),
            ),
            OpenSandboxHttpResponse {
                status: 200,
                headers: BTreeMap::new(),
                body: sse.as_bytes().to_vec(),
            },
        ]);
        let inspect = transport.clone();
        let mut provider = OpenSandboxExecutionProvider::new(config(), transport).unwrap();
        let allocation = ProviderAllocation {
            provider_ref: provider.provider_ref().clone(),
            port: ProviderPortKind::Execution,
            material_ref: "sbx_exec".into(),
            health: HealthState::Healthy,
            properties: BTreeMap::new(),
            provenance: BTreeMap::new(),
        };
        let result = provider
            .execute_operation(
                &allocation,
                &ProviderOperation {
                    key: "command".into(),
                    parameters: BTreeMap::from([("command".into(), "printf hello".into())]),
                },
            )
            .unwrap();
        assert_eq!(
            result.output.get("stdout").map(String::as_str),
            Some("hello\n")
        );
        assert!(!format!("{result:?}").contains("secret-route-token"));
        let requests = inspect.requests();
        assert_eq!(
            requests[1]
                .headers
                .get("OpenSandbox-Secure-Access")
                .map(String::as_str),
            Some("secret-route-token")
        );
    }

    #[test]
    fn endpoint_reading_discloses_header_names_not_values() {
        let transport = FixtureTransport::with_responses(vec![response(
            200,
            json!({
                "endpoint":"endpoint.example/sandboxes/sbx/port/8080",
                "headers":{"OpenSandbox-Secure-Access":"do-not-disclose"}
            }),
        )]);
        let provider = OpenSandboxExecutionProvider::new(config(), transport).unwrap();
        let allocation = ProviderAllocation {
            provider_ref: provider.provider_ref().clone(),
            port: ProviderPortKind::Execution,
            material_ref: "sbx".into(),
            health: HealthState::Healthy,
            properties: BTreeMap::new(),
            provenance: BTreeMap::new(),
        };
        let reading = provider.endpoint_reading(&allocation, 8080).unwrap();
        assert_eq!(
            reading.required_header_names,
            vec!["OpenSandbox-Secure-Access"]
        );
        assert!(!format!("{reading:?}").contains("do-not-disclose"));
    }

    #[test]
    fn provider_vocabulary_does_not_enter_workcell_demand() {
        let source = include_str!("../../workcell-core/src/demand.rs").to_ascii_lowercase();
        assert!(!source.contains("opensandbox"));
        assert!(!source.contains("sandbox_id"));
        assert!(!source.contains("poolref"));
    }

    #[test]
    fn chunked_http_body_is_decoded_for_streaming_data_plane() {
        let body = b"5\r\nhello\r\n6\r\n world\r\n0\r\n\r\n";
        assert_eq!(decode_chunked(body).unwrap(), b"hello world");
    }
}
