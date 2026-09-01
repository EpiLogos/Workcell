use std::collections::BTreeMap;

use epilogos_workcell_sdk::{
    contract::{ExternalRef, WorkcellRef},
    fabric::{
        FabricPolicyOffer, FabricPolicyProvider, FabricPolicyState, NetworkEndpoint,
    },
    provider::{ProviderAllocation, ProviderRef, Result, WorkcellError},
};
use serde_json::{json, Value};

use super::{
    client::{data_request, require_success_safe, resolve_data_endpoint},
    protocol::{OpenSandboxConfig, OpenSandboxTransport, OPENSANDBOX_SOURCE_REVISION},
    OPENSANDBOX_EGRESS_PORT,
};

/// Exact OpenSandbox egress OpenAPI blob inspected for this provider cut.
pub const OPENSANDBOX_EGRESS_SPEC_BLOB: &str = "08e4885176998e854df62b999914c5eb01855308";
pub const OPENSANDBOX_EGRESS_API_VERSION: &str = "0.1.0";

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct OpenSandboxEgressTarget {
    /// Caller-owned logical external endpoint, e.g. `service:github-api`.
    pub endpoint_ref: ExternalRef,
    /// Provider-local policy target, e.g. `api.github.com`.
    pub target: String,
    /// Optional logical relationship identity when one policy entry is intended
    /// for a specific relation rather than every relation to this endpoint.
    pub relationship_ref: Option<String>,
}

impl OpenSandboxEgressTarget {
    pub fn new(endpoint_ref: ExternalRef, target: impl Into<String>) -> Result<Self> {
        let value = Self {
            endpoint_ref,
            target: target.into(),
            relationship_ref: None,
        };
        value.validate()?;
        Ok(value)
    }

    pub fn for_relationship(mut self, relationship_ref: impl Into<String>) -> Result<Self> {
        self.relationship_ref = Some(relationship_ref.into());
        self.validate()?;
        Ok(self)
    }

    pub fn validate(&self) -> Result<()> {
        if self.target.trim().is_empty() {
            return Err(WorkcellError::InvalidDemand(
                "OpenSandbox egress target must not be empty".into(),
            ));
        }
        if self
            .relationship_ref
            .as_deref()
            .is_some_and(|value| value.trim().is_empty())
        {
            return Err(WorkcellError::InvalidDemand(
                "OpenSandbox egress relationship_ref must not be empty when supplied".into(),
            ));
        }
        Ok(())
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum OpenSandboxEgressAction {
    Allow,
    Deny,
}

impl OpenSandboxEgressAction {
    fn as_str(self) -> &'static str {
        match self {
            Self::Allow => "allow",
            Self::Deny => "deny",
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct OpenSandboxEgressRule {
    pub action: OpenSandboxEgressAction,
    pub target: String,
}

impl OpenSandboxEgressRule {
    pub fn validate(&self) -> Result<()> {
        if self.target.trim().is_empty() {
            return Err(WorkcellError::InvalidDemand(
                "OpenSandbox egress rule target must not be empty".into(),
            ));
        }
        Ok(())
    }
}

/// OpenSandbox's egress sidecar supplies policy enforcement over a path owned
/// by Docker/Kubernetes/the host network. It intentionally implements only
/// FabricPolicyProvider; route existence is observed by a separate path provider.
pub struct OpenSandboxEgressPolicyProvider<T> {
    config: OpenSandboxConfig,
    transport: T,
    allocation: ProviderAllocation,
    source_workcell: WorkcellRef,
    targets: Vec<OpenSandboxEgressTarget>,
}

impl<T> OpenSandboxEgressPolicyProvider<T>
where
    T: OpenSandboxTransport,
{
    pub fn new(
        config: OpenSandboxConfig,
        transport: T,
        allocation: ProviderAllocation,
        source_workcell: WorkcellRef,
        targets: Vec<OpenSandboxEgressTarget>,
    ) -> Result<Self> {
        config.validate()?;
        if allocation.provider_ref != config.provider_ref || allocation.material_ref.trim().is_empty() {
            return Err(WorkcellError::InvalidDemand(
                "OpenSandbox egress provider requires an allocation from the same provider instance"
                    .into(),
            ));
        }
        if targets.is_empty() {
            return Err(WorkcellError::InvalidDemand(
                "OpenSandbox egress policy provider requires at least one logical external target"
                    .into(),
            ));
        }
        for target in &targets {
            target.validate()?;
        }
        Ok(Self {
            config,
            transport,
            allocation,
            source_workcell,
            targets,
        })
    }

    /// Merge provider-native egress rules. This mutates enforcement only; it
    /// does not claim or create a network route.
    pub fn patch_policy(&self, rules: &[OpenSandboxEgressRule]) -> Result<()> {
        if rules.is_empty() {
            return Err(WorkcellError::InvalidDemand(
                "OpenSandbox egress patch requires at least one rule".into(),
            ));
        }
        for rule in rules {
            rule.validate()?;
        }
        let endpoint = resolve_data_endpoint(
            &self.config,
            &self.transport,
            &self.allocation,
            OPENSANDBOX_EGRESS_PORT,
        )?;
        let body = serde_json::to_vec(
            &rules
                .iter()
                .map(|rule| {
                    json!({
                        "action": rule.action.as_str(),
                        "target": rule.target,
                    })
                })
                .collect::<Vec<_>>(),
        )
        .map_err(|error| {
            WorkcellError::OperationFailed(format!(
                "encode OpenSandbox egress policy patch: {error}"
            ))
        })?;
        let response = data_request(
            &self.transport,
            &endpoint,
            "PATCH",
            "/policy",
            BTreeMap::new(),
            body,
        )?;
        require_success_safe(&response, "egress policy patch")
    }

    fn observe_policy(&self) -> Result<ObservedPolicy> {
        let endpoint = resolve_data_endpoint(
            &self.config,
            &self.transport,
            &self.allocation,
            OPENSANDBOX_EGRESS_PORT,
        )?;
        let response = data_request(
            &self.transport,
            &endpoint,
            "GET",
            "/policy",
            BTreeMap::new(),
            Vec::new(),
        )?;
        require_success_safe(&response, "egress policy observation")?;
        let value: Value = serde_json::from_slice(&response.body).map_err(|error| {
            WorkcellError::OperationFailed(format!(
                "decode OpenSandbox egress policy response: {error}"
            ))
        })?;
        ObservedPolicy::from_value(&value)
    }

    fn unavailable_offers(&self, reason: &str) -> Vec<FabricPolicyOffer> {
        self.targets
            .iter()
            .map(|target| FabricPolicyOffer {
                provider_ref: self.config.provider_ref.clone(),
                policy_ref: format!(
                    "policy:opensandbox:{}:{}",
                    self.allocation.material_ref, target.endpoint_ref
                ),
                relationship_ref: target.relationship_ref.clone(),
                source: NetworkEndpoint::Workcell(self.source_workcell.clone()),
                destination: NetworkEndpoint::External(target.endpoint_ref.clone()),
                state: FabricPolicyState::Unavailable,
                provenance: policy_provenance(
                    &self.allocation,
                    target,
                    None,
                    None,
                    Some(reason),
                ),
            })
            .collect()
    }
}

impl<T> FabricPolicyProvider for OpenSandboxEgressPolicyProvider<T>
where
    T: OpenSandboxTransport,
{
    fn provider_ref(&self) -> &ProviderRef {
        &self.config.provider_ref
    }

    fn policies(&self) -> Result<Vec<FabricPolicyOffer>> {
        let observed = match self.observe_policy() {
            Ok(observed) => observed,
            Err(error) => return Ok(self.unavailable_offers(&error.to_string())),
        };
        Ok(self
            .targets
            .iter()
            .map(|target| {
                let action = observed.action_for(&target.target);
                let state = match action {
                    OpenSandboxEgressAction::Allow => FabricPolicyState::Allowed,
                    OpenSandboxEgressAction::Deny => FabricPolicyState::Denied,
                };
                FabricPolicyOffer {
                    provider_ref: self.config.provider_ref.clone(),
                    policy_ref: format!(
                        "policy:opensandbox:{}:{}",
                        self.allocation.material_ref, target.endpoint_ref
                    ),
                    relationship_ref: target.relationship_ref.clone(),
                    source: NetworkEndpoint::Workcell(self.source_workcell.clone()),
                    destination: NetworkEndpoint::External(target.endpoint_ref.clone()),
                    state,
                    provenance: policy_provenance(
                        &self.allocation,
                        target,
                        Some(&observed.mode),
                        Some(&observed.enforcement_mode),
                        None,
                    ),
                }
            })
            .collect())
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct ObservedPolicy {
    default_action: OpenSandboxEgressAction,
    rules: Vec<OpenSandboxEgressRule>,
    mode: String,
    enforcement_mode: String,
}

impl ObservedPolicy {
    fn from_value(value: &Value) -> Result<Self> {
        let object = value.as_object().ok_or_else(|| {
            WorkcellError::OperationFailed("OpenSandbox egress response must be an object".into())
        })?;
        let policy = object
            .get("policy")
            .and_then(Value::as_object)
            .ok_or_else(|| {
                WorkcellError::OperationFailed(
                    "OpenSandbox egress response requires a policy object".into(),
                )
            })?;
        let default_action = parse_action(
            policy
                .get("defaultAction")
                .and_then(Value::as_str)
                .unwrap_or("deny"),
        )?;
        let rules = policy
            .get("egress")
            .and_then(Value::as_array)
            .map(|rules| {
                rules
                    .iter()
                    .map(|rule| {
                        let rule = rule.as_object().ok_or_else(|| {
                            WorkcellError::OperationFailed(
                                "OpenSandbox egress rule must be an object".into(),
                            )
                        })?;
                        let action = parse_action(
                            rule.get("action").and_then(Value::as_str).ok_or_else(|| {
                                WorkcellError::OperationFailed(
                                    "OpenSandbox egress rule requires action".into(),
                                )
                            })?,
                        )?;
                        let target = rule
                            .get("target")
                            .and_then(Value::as_str)
                            .filter(|value| !value.trim().is_empty())
                            .ok_or_else(|| {
                                WorkcellError::OperationFailed(
                                    "OpenSandbox egress rule requires target".into(),
                                )
                            })?
                            .to_owned();
                        Ok(OpenSandboxEgressRule { action, target })
                    })
                    .collect::<Result<Vec<_>>>()
            })
            .transpose()?
            .unwrap_or_default();
        Ok(Self {
            default_action,
            rules,
            mode: object
                .get("mode")
                .and_then(Value::as_str)
                .unwrap_or("unknown")
                .to_owned(),
            enforcement_mode: object
                .get("enforcementMode")
                .and_then(Value::as_str)
                .unwrap_or("unknown")
                .to_owned(),
        })
    }

    fn action_for(&self, target: &str) -> OpenSandboxEgressAction {
        self.rules
            .iter()
            .find(|rule| rule.target.eq_ignore_ascii_case(target))
            .map(|rule| rule.action)
            .unwrap_or(self.default_action)
    }
}

fn parse_action(value: &str) -> Result<OpenSandboxEgressAction> {
    match value.to_ascii_lowercase().as_str() {
        "allow" => Ok(OpenSandboxEgressAction::Allow),
        "deny" => Ok(OpenSandboxEgressAction::Deny),
        other => Err(WorkcellError::OperationFailed(format!(
            "OpenSandbox egress action `{other}` is not supported by the pinned API"
        ))),
    }
}

fn policy_provenance(
    allocation: &ProviderAllocation,
    target: &OpenSandboxEgressTarget,
    mode: Option<&str>,
    enforcement_mode: Option<&str>,
    observation_error: Option<&str>,
) -> BTreeMap<String, String> {
    let mut provenance = BTreeMap::from([
        ("provider".into(), "opensandbox:egress-policy".into()),
        ("upstream.revision".into(), OPENSANDBOX_SOURCE_REVISION.into()),
        ("upstream.egress_spec_blob".into(), OPENSANDBOX_EGRESS_SPEC_BLOB.into()),
        ("upstream.egress_api".into(), OPENSANDBOX_EGRESS_API_VERSION.into()),
        ("sandbox.material_ref".into(), allocation.material_ref.clone()),
        ("provider.target".into(), target.target.clone()),
    ]);
    if let Some(mode) = mode {
        provenance.insert("egress.mode".into(), mode.into());
    }
    if let Some(enforcement_mode) = enforcement_mode {
        provenance.insert("egress.enforcement_mode".into(), enforcement_mode.into());
    }
    if let Some(error) = observation_error {
        provenance.insert("observation_error".into(), error.into());
    }
    provenance
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Mutex};

    use epilogos_workcell_sdk::{
        fabric::{
            evaluate_fabric_with_policies, require_fabric_plan, FabricDiagnosticKind,
            FabricPathOffer, FabricPathProvider, FabricPathState, NetworkRelationship,
            NetworkSecurity, ReachabilityScope, RequiredNetworkRelationship,
        },
        provider::{HealthState, ProviderPortKind},
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
                .ok_or_else(|| WorkcellError::Unavailable("fixture exhausted".into()))
        }
    }

    struct PathFixture(FabricPathOffer);

    impl FabricPathProvider for PathFixture {
        fn provider_ref(&self) -> &ProviderRef {
            &self.0.provider_ref
        }

        fn paths(&self) -> Result<Vec<FabricPathOffer>> {
            Ok(vec![self.0.clone()])
        }
    }

    fn response(status: u16, value: Value) -> OpenSandboxHttpResponse {
        OpenSandboxHttpResponse {
            status,
            headers: BTreeMap::new(),
            body: serde_json::to_vec(&value).unwrap(),
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
            material_ref: "sbx_egress".into(),
            health: HealthState::Healthy,
            properties: BTreeMap::new(),
            provenance: BTreeMap::new(),
        }
    }

    fn target(endpoint_ref: &str, host: &str) -> OpenSandboxEgressTarget {
        OpenSandboxEgressTarget::new(ExternalRef::new(endpoint_ref).unwrap(), host).unwrap()
    }

    #[test]
    fn observed_allow_and_default_deny_become_independent_policy_offers() {
        let transport = FixtureTransport::with_responses(vec![
            response(
                200,
                json!({
                    "endpoint": "http://egress.fixture:18080",
                    "headers": {"OPENSANDBOX-EGRESS-AUTH": "provider-auth"}
                }),
            ),
            response(
                200,
                json!({
                    "status": "ok",
                    "mode": "deny_all",
                    "enforcementMode": "dns+nft",
                    "policy": {
                        "defaultAction": "deny",
                        "egress": [{"action": "allow", "target": "api.github.com"}]
                    }
                }),
            ),
        ]);
        let provider = OpenSandboxEgressPolicyProvider::new(
            config(),
            transport,
            allocation(),
            WorkcellRef::new("workcell:sandbox").unwrap(),
            vec![
                target("service:github-api", "api.github.com"),
                target("service:model-api", "api.example.ai"),
            ],
        )
        .unwrap();
        let policies = provider.policies().unwrap();
        assert_eq!(policies[0].state, FabricPolicyState::Allowed);
        assert_eq!(policies[1].state, FabricPolicyState::Denied);
        assert_eq!(
            policies[0].provenance["egress.enforcement_mode"],
            "dns+nft"
        );
    }

    #[test]
    fn reachable_external_path_plus_denied_egress_reports_policy_denial_not_path_absence() {
        let transport = FixtureTransport::with_responses(vec![
            response(
                200,
                json!({"endpoint": "http://egress.fixture:18080", "headers": {}}),
            ),
            response(
                200,
                json!({
                    "status": "ok",
                    "mode": "deny_all",
                    "enforcementMode": "dns+nft",
                    "policy": {"defaultAction": "deny", "egress": []}
                }),
            ),
        ]);
        let policy = OpenSandboxEgressPolicyProvider::new(
            config(),
            transport,
            allocation(),
            WorkcellRef::new("workcell:sandbox").unwrap(),
            vec![target("service:github-api", "api.github.com")],
        )
        .unwrap();
        let source = NetworkEndpoint::Workcell(WorkcellRef::new("workcell:sandbox").unwrap());
        let destination = NetworkEndpoint::External(ExternalRef::new("service:github-api").unwrap());
        let path = PathFixture(FabricPathOffer {
            provider_ref: ProviderRef::new("provider:kubernetes-network").unwrap(),
            path_ref: "path:public-egress".into(),
            source: source.clone(),
            destination: destination.clone(),
            transport: Some("tcp".into()),
            scope: ReachabilityScope::Public,
            security: NetworkSecurity::Encrypted,
            state: FabricPathState::Reachable,
            endpoint: None,
            path_class: Some("host-egress".into()),
            provenance: BTreeMap::new(),
        });
        let relationship = RequiredNetworkRelationship {
            relationship: NetworkRelationship::new(
                "relationship:github-api",
                "execution:world",
                "service:github-api",
            )
            .unwrap()
            .with_transport("tcp")
            .unwrap()
            .with_scope(ReachabilityScope::Public)
            .with_security(NetworkSecurity::Encrypted),
            necessity: epilogos_workcell_sdk::contract::RequirementNecessity::Required,
            source,
            destination,
            requires_policy: true,
        };
        let plan = evaluate_fabric_with_policies(&[relationship], &[&path], &[&policy]).unwrap();
        assert!(plan
            .diagnostics
            .iter()
            .any(|item| item.kind == FabricDiagnosticKind::PolicyDenied));
        assert!(!plan
            .diagnostics
            .iter()
            .any(|item| item.kind == FabricDiagnosticKind::DestinationUnavailable));
        assert!(require_fabric_plan(plan).is_err());
    }

    #[test]
    fn policy_patch_uses_egress_sidecar_auth_but_does_not_materialise_a_path() {
        let transport = FixtureTransport::with_responses(vec![
            response(
                200,
                json!({
                    "endpoint": "http://egress.fixture:18080",
                    "headers": {"OPENSANDBOX-EGRESS-AUTH": "provider-auth"}
                }),
            ),
            response(200, json!({"status":"ok", "mode":"deny_all"})),
        ]);
        let inspect = transport.clone();
        let provider = OpenSandboxEgressPolicyProvider::new(
            config(),
            transport,
            allocation(),
            WorkcellRef::new("workcell:sandbox").unwrap(),
            vec![target("service:github-api", "api.github.com")],
        )
        .unwrap();
        provider
            .patch_policy(&[OpenSandboxEgressRule {
                action: OpenSandboxEgressAction::Allow,
                target: "api.github.com".into(),
            }])
            .unwrap();
        let requests = inspect.requests();
        assert_eq!(requests[1].method, "PATCH");
        assert!(requests[1].url.ends_with("/policy"));
        assert_eq!(
            requests[1].headers.get("OPENSANDBOX-EGRESS-AUTH"),
            Some(&"provider-auth".into())
        );
    }
}
