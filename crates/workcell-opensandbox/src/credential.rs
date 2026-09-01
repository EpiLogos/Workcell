use std::collections::BTreeMap;

use epilogos_workcell_core::{
    authorise_broker_boundary, BrokerHandle, BrokerPolicy, BrokerRoute, ProviderAllocation, Result,
    SecretMaterialReceipt, SecretMaterialisationRequest, SecretProvider, WorkcellError,
};
use serde_json::{json, Value};

use super::{
    client::{data_request, require_success_safe, resolve_data_endpoint},
    protocol::{
        OpenSandboxConfig, OpenSandboxTransport, OPENSANDBOX_SOURCE_REVISION,
    },
};

pub const OPENSANDBOX_EGRESS_PORT: u16 = 18080;

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum OpenSandboxCredentialAuth {
    Bearer,
    Basic,
    ApiKey { header_name: String },
}

impl OpenSandboxCredentialAuth {
    fn validate(&self) -> Result<()> {
        if let Self::ApiKey { header_name } = self {
            if header_name.trim().is_empty() {
                return Err(WorkcellError::InvalidDemand(
                    "OpenSandbox Credential Vault API-key header must not be empty".into(),
                ));
            }
        }
        Ok(())
    }

    fn value(&self, credential_name: &str) -> Value {
        match self {
            Self::Bearer => json!({
                "type": "bearer",
                "credential": credential_name,
            }),
            Self::Basic => json!({
                "type": "basic",
                "credential": credential_name,
            }),
            Self::ApiKey { header_name } => json!({
                "type": "apiKey",
                "name": header_name,
                "credential": credential_name,
            }),
        }
    }
}

/// Provider-local rendering of one already-authorised Workcell broker grant.
/// Host and method are taken from the Workcell BrokerRoute rather than repeated
/// here, so this value cannot silently widen the authorised destination.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct OpenSandboxCredentialBindingSpec {
    pub credential_name: String,
    pub binding_name: String,
    pub schemes: Vec<String>,
    pub paths: Vec<String>,
    pub auth: OpenSandboxCredentialAuth,
}

impl OpenSandboxCredentialBindingSpec {
    pub fn https(
        credential_name: impl Into<String>,
        binding_name: impl Into<String>,
        paths: Vec<String>,
        auth: OpenSandboxCredentialAuth,
    ) -> Result<Self> {
        let spec = Self {
            credential_name: credential_name.into(),
            binding_name: binding_name.into(),
            schemes: vec!["https".into()],
            paths,
            auth,
        };
        spec.validate()?;
        Ok(spec)
    }

    pub fn validate(&self) -> Result<()> {
        for (label, value) in [
            ("credential_name", self.credential_name.as_str()),
            ("binding_name", self.binding_name.as_str()),
        ] {
            if value.trim().is_empty() {
                return Err(WorkcellError::InvalidDemand(format!(
                    "OpenSandbox Credential Vault {label} must not be empty"
                )));
            }
        }
        if self.schemes.is_empty()
            || self
                .schemes
                .iter()
                .any(|value| !matches!(value.as_str(), "http" | "https"))
        {
            return Err(WorkcellError::InvalidDemand(
                "OpenSandbox Credential Vault schemes must contain only http/https".into(),
            ));
        }
        if self.paths.is_empty() || self.paths.iter().any(|path| !path.starts_with('/')) {
            return Err(WorkcellError::InvalidDemand(
                "OpenSandbox Credential Vault requires at least one absolute request path".into(),
            ));
        }
        self.auth.validate()
    }
}

/// Portable evidence that Workcell authorised material use and the provider
/// accepted a sandbox-local Vault revision. It deliberately contains no secret
/// value and no provider endpoint authentication value.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct OpenSandboxCredentialMaterialReceipt {
    pub workcell_receipt: SecretMaterialReceipt,
    pub sandbox_material_ref: String,
    pub vault_revision: Option<u64>,
    pub credential_name: String,
    pub binding_name: String,
    pub reinjection_required_after_sidecar_recreation: bool,
    pub provenance: BTreeMap<String, String>,
}

pub struct OpenSandboxCredentialBroker<T> {
    config: OpenSandboxConfig,
    transport: T,
}

impl<T> OpenSandboxCredentialBroker<T>
where
    T: OpenSandboxTransport,
{
    pub fn new(config: OpenSandboxConfig, transport: T) -> Result<Self> {
        config.validate()?;
        Ok(Self { config, transport })
    }

    pub fn materialise<P: SecretProvider>(
        &self,
        allocation: &ProviderAllocation,
        source_provider: &P,
        policy: &BrokerPolicy,
        handle: &BrokerHandle,
        request: &SecretMaterialisationRequest,
        route: &BrokerRoute,
        binding: &OpenSandboxCredentialBindingSpec,
    ) -> Result<OpenSandboxCredentialMaterialReceipt> {
        binding.validate()?;

        // Authorisation happens before endpoint discovery or provider writes.
        // A denied route therefore cannot leak material into a sandbox-local sink.
        let (material, workcell_receipt) =
            authorise_broker_boundary(source_provider, policy, handle, request, route)?;

        let endpoint = resolve_data_endpoint(
            &self.config,
            &self.transport,
            allocation,
            OPENSANDBOX_EGRESS_PORT,
        )?;
        let body = serde_json::to_vec(&json!({
            "credentials": [{
                "name": binding.credential_name,
                "source": {
                    "type": "inline",
                    "value": material.value.expose_for_materialisation(),
                }
            }],
            "bindings": [{
                "name": binding.binding_name,
                "match": {
                    "schemes": binding.schemes,
                    "hosts": [route.destination_host],
                    "methods": [route.method],
                    "paths": binding.paths,
                },
                "auth": binding.auth.value(&binding.credential_name),
            }]
        }))
        .map_err(|error| {
            WorkcellError::OperationFailed(format!(
                "encode OpenSandbox Credential Vault request: {error}"
            ))
        })?;

        let response = data_request(
            &self.transport,
            &endpoint,
            "POST",
            "/credential-vault",
            BTreeMap::new(),
            body,
        )?;
        // Do not include provider response bodies in credential-boundary errors;
        // the upstream contract is sanitized, but Workcell does not depend on it.
        require_success_safe(&response, "credential-vault materialisation")?;
        let vault_revision = if response.body.is_empty() {
            None
        } else {
            let value: Value = serde_json::from_slice(&response.body).map_err(|error| {
                WorkcellError::OperationFailed(format!(
                    "decode sanitized OpenSandbox Credential Vault response: {error}"
                ))
            })?;
            value.get("revision").and_then(Value::as_u64)
        };

        Ok(OpenSandboxCredentialMaterialReceipt {
            workcell_receipt,
            sandbox_material_ref: allocation.material_ref.clone(),
            vault_revision,
            credential_name: binding.credential_name.clone(),
            binding_name: binding.binding_name.clone(),
            reinjection_required_after_sidecar_recreation: true,
            provenance: BTreeMap::from([
                ("provider".into(), "opensandbox:credential-vault".into()),
                ("upstream.revision".into(), OPENSANDBOX_SOURCE_REVISION.into()),
                ("egress.port".into(), OPENSANDBOX_EGRESS_PORT.to_string()),
                ("secret.visibility".into(), "use-without-read".into()),
            ]),
        })
    }
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Mutex};

    use epilogos_workcell_core::{
        broker_handle, BindingRef, ExternalRef, HealthState, ProviderPortKind, ProviderRef,
        ProviderSecretMaterial, SecretMaterialisationClass, SecretRevocationState, SecretValue,
    };

    use super::*;
    use crate::{OpenSandboxHttpRequest, OpenSandboxHttpResponse};

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

    struct FixedSecretProvider {
        provider_ref: ProviderRef,
        value: &'static str,
    }

    impl SecretProvider for FixedSecretProvider {
        fn provider_ref(&self) -> &ProviderRef {
            &self.provider_ref
        }

        fn resolve(&self, _credential_ref: &ExternalRef) -> Result<ProviderSecretMaterial> {
            Ok(ProviderSecretMaterial {
                value: SecretValue::new(self.value)?,
                revision_or_lease_class: Some("fixture:v1".into()),
                expires_at: None,
                revocation_state: SecretRevocationState::Active,
            })
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
        config
    }

    fn allocation() -> ProviderAllocation {
        ProviderAllocation {
            provider_ref: ProviderRef::new("provider:opensandbox").unwrap(),
            port: ProviderPortKind::Execution,
            material_ref: "sbx_credential_fixture".into(),
            health: HealthState::Healthy,
            properties: BTreeMap::new(),
            provenance: BTreeMap::new(),
        }
    }

    fn grant() -> (
        FixedSecretProvider,
        SecretMaterialisationRequest,
        BrokerRoute,
        BrokerPolicy,
        BrokerHandle,
    ) {
        let provider = FixedSecretProvider {
            provider_ref: ProviderRef::new("secret-provider:fixture").unwrap(),
            value: "RAW_FIXTURE_SECRET_DO_NOT_RETURN",
        };
        let request = SecretMaterialisationRequest {
            credential_ref: ExternalRef::new("credential:github/operator").unwrap(),
            provider_ref: provider.provider_ref.clone(),
            binding_ref: BindingRef::new("binding:github/operator").unwrap(),
            consumer_ref: ExternalRef::new("agent-session:fixture").unwrap(),
            workload_ref: Some(ExternalRef::new("world:fixture").unwrap()),
            class: SecretMaterialisationClass::CredentialBroker,
            purpose: "github-api".into(),
            destination: "opensandbox-egress".into(),
            scope: "repo:read".into(),
        };
        let route = BrokerRoute {
            destination_host: "api.github.com".into(),
            method: "GET".into(),
            purpose: request.purpose.clone(),
            scope: request.scope.clone(),
        };
        let policy = BrokerPolicy::new(vec![route.clone()]).unwrap();
        let handle = broker_handle(&request).unwrap();
        (provider, request, route, policy, handle)
    }

    #[test]
    fn authorised_secret_crosses_only_the_provider_sink_boundary() {
        let transport = FixtureTransport::with_responses(vec![
            response(
                200,
                json!({
                    "endpoint": "http://egress.fixture:18080",
                    "headers": {"OPENSANDBOX-EGRESS-AUTH": "sidecar-auth"}
                }),
            ),
            response(
                200,
                json!({
                    "revision": 7,
                    "credentials": [{"name": "github-token", "sourceType": "inline", "revision": 7}],
                    "bindings": [{"name": "github-read", "revision": 7}]
                }),
            ),
        ]);
        let broker = OpenSandboxCredentialBroker::new(config(), transport.clone()).unwrap();
        let (provider, request, route, policy, handle) = grant();
        let binding = OpenSandboxCredentialBindingSpec::https(
            "github-token",
            "github-read",
            vec!["/repos/*".into()],
            OpenSandboxCredentialAuth::Bearer,
        )
        .unwrap();

        let receipt = broker
            .materialise(
                &allocation(),
                &provider,
                &policy,
                &handle,
                &request,
                &route,
                &binding,
            )
            .unwrap();

        assert_eq!(receipt.vault_revision, Some(7));
        assert!(receipt.reinjection_required_after_sidecar_recreation);
        assert!(!format!("{receipt:?}").contains("RAW_FIXTURE_SECRET_DO_NOT_RETURN"));

        let requests = transport.requests();
        assert_eq!(requests.len(), 2);
        let vault_request = &requests[1];
        assert!(vault_request.url.ends_with("/credential-vault"));
        let wire = String::from_utf8(vault_request.body.clone()).unwrap();
        assert!(wire.contains("RAW_FIXTURE_SECRET_DO_NOT_RETURN"));
        assert!(wire.contains("api.github.com"));
        assert!(wire.contains("github-read"));
        assert_eq!(
            vault_request.headers.get("OPENSANDBOX-EGRESS-AUTH"),
            Some(&"sidecar-auth".into())
        );
    }

    #[test]
    fn denied_route_produces_no_provider_side_write() {
        let transport = FixtureTransport::default();
        let broker = OpenSandboxCredentialBroker::new(config(), transport.clone()).unwrap();
        let (provider, request, _route, policy, handle) = grant();
        let denied = BrokerRoute {
            destination_host: "attacker.invalid".into(),
            method: "POST".into(),
            purpose: request.purpose.clone(),
            scope: request.scope.clone(),
        };
        let binding = OpenSandboxCredentialBindingSpec::https(
            "github-token",
            "github-read",
            vec!["/".into()],
            OpenSandboxCredentialAuth::Bearer,
        )
        .unwrap();

        let result = broker.materialise(
            &allocation(),
            &provider,
            &policy,
            &handle,
            &request,
            &denied,
            &binding,
        );
        assert!(result.is_err());
        assert!(transport.requests().is_empty());
    }
}
