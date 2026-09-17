//! Projection seam for OpenSandbox: the origin workcell holds a
//! `SecretProjectionRequest`, the sandbox receives material through the
//! existing Credential Vault broker — no new sink, no new transport.
//!
//! The law this seam enforces, in order, all before any transport call:
//!   1. the projection request itself validates (class/purpose/scope);
//!   2. the origin's recorded state for the projection is active — a
//!      revoked projection reaches the projected target as a refusal and
//!      the target receives nothing;
//!   3. the target really is this sandbox allocation, and the materialisation
//!      presented matches the projection (credential, provider, class,
//!      purpose, scope) — a route or scope outside the projection grant is
//!      denied with zero provider-side writes, the same parity the broker
//!      itself proves for its own policy;
//!   4. only then does the existing `OpenSandboxCredentialBroker::materialise`
//!      run, and its receipt is wrapped as refs-only `SecretProjectionReceipt`
//!      evidence (`use-without-read`).

use epilogos_workcell_core::{
    authorise_broker_boundary, BrokerHandle, BrokerPolicy, BrokerRoute, ProviderAllocation, Result,
    SecretMaterialisationClass, SecretMaterialisationRequest, SecretProjectionReceipt,
    SecretProjectionRequest, SecretProjectionTarget, SecretProvider, SecretRevocationState,
    WorkcellError, SECRET_PROJECTION_VERSION,
};

use super::credential::{
    OpenSandboxCredentialBindingSpec, OpenSandboxCredentialBroker,
    OpenSandboxCredentialMaterialisation,
};

/// Project one credential from the origin store into a sandbox allocation
/// through the existing Credential Vault broker. Every check above runs
/// before the broker is touched, so a denied route or a revoked projection
/// performs zero provider-side writes.
#[allow(clippy::too_many_arguments)]
pub fn project_credential_to_sandbox<T, P>(
    broker: &OpenSandboxCredentialBroker<T>,
    projection: &SecretProjectionRequest,
    projection_state: SecretRevocationState,
    allocation: &ProviderAllocation,
    source_provider: &P,
    request: &SecretMaterialisationRequest,
    policy: &BrokerPolicy,
    handle: &BrokerHandle,
    route: &BrokerRoute,
    binding: &OpenSandboxCredentialBindingSpec,
) -> Result<SecretProjectionReceipt>
where
    T: super::OpenSandboxTransport,
    P: SecretProvider,
{
    projection.validate()?;

    if projection_state != SecretRevocationState::Active {
        return Err(WorkcellError::UnsatisfiedDemand(format!(
            "secret projection is {} at the origin; the projected target receives nothing \
             until the origin re-authorises it",
            match projection_state {
                SecretRevocationState::Revoked => "revoked",
                SecretRevocationState::Expired => "expired",
                SecretRevocationState::Active => unreachable!(),
            }
        )));
    }

    let target = match &projection.target {
        SecretProjectionTarget::Sandbox {
            provider_ref,
            allocation_ref,
        } => (provider_ref, allocation_ref),
        SecretProjectionTarget::Workcell { .. } => {
            return Err(WorkcellError::InvalidDemand(
                "this seam projects to sandbox allocations; a workcell target presents its \
                 materialisation back to the origin over its cross-cell connection"
                    .into(),
            ))
        }
    };
    if *target.0 != allocation.provider_ref || target.1 != &allocation.material_ref {
        return Err(WorkcellError::UnsatisfiedDemand(format!(
            "projection target {}/{} does not match the presented allocation {}/{}",
            target.0.as_str(),
            target.1,
            allocation.provider_ref.as_str(),
            allocation.material_ref
        )));
    }

    if projection.credential_ref != request.credential_ref
        || projection.source_provider_ref != request.provider_ref
        || projection.class != request.class
        || projection.purpose != request.purpose
        || projection.scope != request.scope
    {
        return Err(WorkcellError::UnsatisfiedDemand(
            "presented materialisation does not match the origin's projection grant \
             (credential, provider, class, purpose and scope must all agree)"
                .into(),
        ));
    }
    if request.class != SecretMaterialisationClass::CredentialBroker {
        return Err(WorkcellError::InvalidDemand(
            "sandbox projection materialises through the credential broker".into(),
        ));
    }

    // Deny-before-write parity: the same boundary authorisation the broker
    // runs, surfaced here so a projection-level caller sees the refusal
    // before any endpoint discovery or sink write happens inside
    // `materialise`. Cheap, read-only, and it keeps the parity test honest.
    authorise_broker_boundary(source_provider, policy, handle, request, route)?;

    let materialisation = broker.materialise(OpenSandboxCredentialMaterialisation {
        allocation,
        source_provider,
        policy,
        handle,
        request,
        route,
        binding,
    })?;

    Ok(SecretProjectionReceipt {
        version: SECRET_PROJECTION_VERSION,
        credential_ref: projection.credential_ref.clone(),
        source_provider_ref: projection.source_provider_ref.clone(),
        target: projection.target.clone(),
        class: projection.class.clone(),
        purpose: projection.purpose.clone(),
        scope: projection.scope.clone(),
        requested_by: projection.requested_by.clone(),
        materialisation: Some(materialisation.workcell_receipt),
        provenance: [
            (
                "sandbox.material_ref".to_owned(),
                materialisation.sandbox_material_ref,
            ),
            (
                "sandbox.vault_revision".to_owned(),
                materialisation
                    .vault_revision
                    .map(|revision| revision.to_string())
                    .unwrap_or_default(),
            ),
            (
                "sandbox.credential_name".to_owned(),
                materialisation.credential_name,
            ),
            (
                "sandbox.binding_name".to_owned(),
                materialisation.binding_name,
            ),
            (
                "sandbox.reinjection_required_after_sidecar_recreation".to_owned(),
                materialisation
                    .reinjection_required_after_sidecar_recreation
                    .to_string(),
            ),
            (
                "secret.visibility".to_owned(),
                "use-without-read".to_owned(),
            ),
        ]
        .into_iter()
        .collect(),
    })
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
    struct ProjectionTransport {
        requests: Arc<Mutex<Vec<OpenSandboxHttpRequest>>>,
        responses: Arc<Mutex<Vec<OpenSandboxHttpResponse>>>,
    }

    impl ProjectionTransport {
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

    impl super::super::OpenSandboxTransport for ProjectionTransport {
        fn request(&self, request: OpenSandboxHttpRequest) -> Result<OpenSandboxHttpResponse> {
            self.requests.lock().unwrap().push(request);
            self.responses
                .lock()
                .unwrap()
                .pop()
                .ok_or_else(|| WorkcellError::OperationFailed("fixture response exhausted".into()))
        }
    }

    struct StatefulOriginProvider {
        provider_ref: ProviderRef,
        value: &'static str,
        revocation_state: SecretRevocationState,
    }

    impl SecretProvider for StatefulOriginProvider {
        fn provider_ref(&self) -> &ProviderRef {
            &self.provider_ref
        }

        fn resolve(&self, _credential_ref: &ExternalRef) -> Result<ProviderSecretMaterial> {
            Ok(ProviderSecretMaterial {
                value: SecretValue::new(self.value)?,
                revision_or_lease_class: Some("origin:revision-1".into()),
                expires_at: None,
                revocation_state: self.revocation_state.clone(),
            })
        }
    }

    fn response(status: u16, value: serde_json::Value) -> OpenSandboxHttpResponse {
        OpenSandboxHttpResponse {
            status,
            headers: std::collections::BTreeMap::new(),
            body: if value.is_null() {
                Vec::new()
            } else {
                serde_json::to_vec(&value).unwrap()
            },
        }
    }

    const RAW_PROJECTION_FIXTURE: &str = "RAW_PROJECTION_FIXTURE_MATERIAL";

    struct ProjectionGrant {
        provider: StatefulOriginProvider,
        broker: OpenSandboxCredentialBroker<ProjectionTransport>,
        transport: ProjectionTransport,
        projection: SecretProjectionRequest,
        request: SecretMaterialisationRequest,
        policy: BrokerPolicy,
        handle: BrokerHandle,
        route: BrokerRoute,
        binding: OpenSandboxCredentialBindingSpec,
        allocation: ProviderAllocation,
    }

    fn grant(transport: ProjectionTransport) -> ProjectionGrant {
        let provider = StatefulOriginProvider {
            provider_ref: ProviderRef::new("secret-provider:fixture").unwrap(),
            value: RAW_PROJECTION_FIXTURE,
            revocation_state: SecretRevocationState::Active,
        };
        let request = SecretMaterialisationRequest {
            credential_ref: ExternalRef::new("credential:github/operator").unwrap(),
            provider_ref: provider.provider_ref.clone(),
            binding_ref: BindingRef::new("binding:github/operator").unwrap(),
            consumer_ref: ExternalRef::new("agent-session:fixture").unwrap(),
            workload_ref: Some(ExternalRef::new("workload:fixture").unwrap()),
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
        let projection = SecretProjectionRequest {
            credential_ref: request.credential_ref.clone(),
            source_provider_ref: request.provider_ref.clone(),
            target: SecretProjectionTarget::Sandbox {
                provider_ref: ProviderRef::new("provider:opensandbox").unwrap(),
                allocation_ref: "sbx_projection_fixture".into(),
            },
            class: SecretMaterialisationClass::CredentialBroker,
            purpose: request.purpose.clone(),
            scope: request.scope.clone(),
            requested_by: ExternalRef::new("agent-session:fixture").unwrap(),
        };
        let binding = OpenSandboxCredentialBindingSpec::https(
            "github-token",
            "github-read",
            vec!["/repos/*".into()],
            crate::OpenSandboxCredentialAuth::Bearer,
        )
        .unwrap();
        let allocation = ProviderAllocation {
            provider_ref: ProviderRef::new("provider:opensandbox").unwrap(),
            port: ProviderPortKind::Execution,
            material_ref: "sbx_projection_fixture".into(),
            health: HealthState::Healthy,
            properties: std::collections::BTreeMap::new(),
            provenance: std::collections::BTreeMap::new(),
        };
        let mut config = crate::OpenSandboxConfig::local(
            ProviderRef::new("provider:opensandbox").unwrap(),
            "opensandbox/code-interpreter:v1.1.0",
            vec!["/opt/code-interpreter/code-interpreter.sh".into()],
        )
        .unwrap();
        config.api_key_env = None;
        let broker = OpenSandboxCredentialBroker::new(config, transport.clone()).unwrap();
        ProjectionGrant {
            provider,
            broker,
            transport,
            projection,
            request,
            policy,
            handle,
            route,
            binding,
            allocation,
        }
    }

    fn endpoint_and_vault_responses() -> Vec<OpenSandboxHttpResponse> {
        vec![
            response(
                200,
                serde_json::json!({
                    "endpoint": "http://egress.fixture:18080",
                    "headers": {"OPENSANDBOX-EGRESS-AUTH": "sidecar-auth"}
                }),
            ),
            response(
                200,
                serde_json::json!({
                    "revision": 9,
                    "credentials": [{"name": "github-token", "sourceType": "inline", "revision": 9}],
                    "bindings": [{"name": "github-read", "revision": 9}]
                }),
            ),
        ]
    }

    #[test]
    fn projection_denied_route_performs_zero_provider_side_writes() {
        let transport = ProjectionTransport::default();
        let grant = grant(transport.clone());
        let denied = BrokerRoute {
            destination_host: "attacker.invalid".into(),
            method: "POST".into(),
            purpose: grant.projection.purpose.clone(),
            scope: grant.projection.scope.clone(),
        };
        let result = project_credential_to_sandbox(
            &grant.broker,
            &grant.projection,
            SecretRevocationState::Active,
            &grant.allocation,
            &grant.provider,
            &grant.request,
            &grant.policy,
            &grant.handle,
            &denied,
            &grant.binding,
        );
        assert!(result.is_err());
        // Zero writes: not even endpoint discovery reached the transport.
        assert!(transport.requests().is_empty());
    }

    #[test]
    fn projection_scope_widening_is_denied_before_any_write() {
        let transport = ProjectionTransport::default();
        let grant = grant(transport.clone());
        let widened = BrokerRoute {
            destination_host: "api.github.com".into(),
            method: "GET".into(),
            purpose: grant.projection.purpose.clone(),
            scope: "repo:write".into(),
        };
        let result = project_credential_to_sandbox(
            &grant.broker,
            &grant.projection,
            SecretRevocationState::Active,
            &grant.allocation,
            &grant.provider,
            &grant.request,
            &grant.policy,
            &grant.handle,
            &widened,
            &grant.binding,
        );
        assert!(result.is_err());
        assert!(transport.requests().is_empty());
    }

    #[test]
    fn revocation_at_the_origin_reaches_the_projected_target() {
        let transport = ProjectionTransport::default();
        let mut grant = grant(transport.clone());
        grant.provider.revocation_state = SecretRevocationState::Revoked;

        let result = project_credential_to_sandbox(
            &grant.broker,
            &grant.projection,
            SecretRevocationState::Revoked,
            &grant.allocation,
            &grant.provider,
            &grant.request,
            &grant.policy,
            &grant.handle,
            &grant.route,
            &grant.binding,
        );
        let error = result.unwrap_err();
        let rendered = format!("{error:?}");
        assert!(
            rendered.contains("revoked"),
            "refusal must say revoked: {rendered}"
        );
        assert!(rendered.contains("origin"));
        assert!(transport.requests().is_empty());
    }

    #[test]
    fn mismatched_target_or_materialisation_is_refused_before_any_write() {
        let transport = ProjectionTransport::default();
        let grant = grant(transport.clone());

        let wrong_allocation = ProviderAllocation {
            material_ref: "sbx_other_allocation".into(),
            ..clone_allocation(&grant.allocation)
        };
        let result = project_credential_to_sandbox(
            &grant.broker,
            &grant.projection,
            SecretRevocationState::Active,
            &wrong_allocation,
            &grant.provider,
            &grant.request,
            &grant.policy,
            &grant.handle,
            &grant.route,
            &grant.binding,
        );
        assert!(result.is_err());
        assert!(transport.requests().is_empty());

        let mut other_request = clone_request(&grant.request);
        other_request.scope = "repo:admin".into();
        let result = project_credential_to_sandbox(
            &grant.broker,
            &grant.projection,
            SecretRevocationState::Active,
            &grant.allocation,
            &grant.provider,
            &other_request,
            &grant.policy,
            &grant.handle,
            &grant.route,
            &grant.binding,
        );
        assert!(result.is_err());
        assert!(transport.requests().is_empty());
    }

    #[test]
    fn authorised_projection_crosses_only_the_vault_boundary_and_receipts_stay_ref_only() {
        let transport = ProjectionTransport::with_responses(endpoint_and_vault_responses());
        let grant = grant(transport.clone());

        let receipt = project_credential_to_sandbox(
            &grant.broker,
            &grant.projection,
            SecretRevocationState::Active,
            &grant.allocation,
            &grant.provider,
            &grant.request,
            &grant.policy,
            &grant.handle,
            &grant.route,
            &grant.binding,
        )
        .unwrap();

        assert_eq!(
            receipt.version,
            epilogos_workcell_core::SECRET_PROJECTION_VERSION
        );
        assert!(receipt.materialisation.is_some());
        assert_eq!(
            receipt.provenance.get("secret.visibility"),
            Some(&"use-without-read".to_owned())
        );
        assert_eq!(
            receipt.provenance.get("sandbox.vault_revision"),
            Some(&"9".to_owned())
        );
        assert_eq!(
            receipt
                .provenance
                .get("sandbox.reinjection_required_after_sidecar_recreation"),
            Some(&"true".to_owned())
        );
        let rendered = format!("{receipt:?}");
        assert!(!rendered.contains(RAW_PROJECTION_FIXTURE));

        // Material crossed exactly once: to the vault endpoint, inside the
        // approved request body, and nowhere else.
        let requests = transport.requests();
        assert_eq!(requests.len(), 2);
        assert!(requests[1].url.ends_with("/credential-vault"));
        let wire = String::from_utf8(requests[1].body.clone()).unwrap();
        assert!(wire.contains(RAW_PROJECTION_FIXTURE));
    }

    fn clone_allocation(allocation: &ProviderAllocation) -> ProviderAllocation {
        ProviderAllocation {
            provider_ref: allocation.provider_ref.clone(),
            port: allocation.port.clone(),
            material_ref: allocation.material_ref.clone(),
            health: allocation.health.clone(),
            properties: allocation.properties.clone(),
            provenance: allocation.provenance.clone(),
        }
    }

    fn clone_request(request: &SecretMaterialisationRequest) -> SecretMaterialisationRequest {
        SecretMaterialisationRequest {
            credential_ref: request.credential_ref.clone(),
            provider_ref: request.provider_ref.clone(),
            binding_ref: request.binding_ref.clone(),
            consumer_ref: request.consumer_ref.clone(),
            workload_ref: request.workload_ref.clone(),
            class: request.class.clone(),
            purpose: request.purpose.clone(),
            destination: request.destination.clone(),
            scope: request.scope.clone(),
        }
    }
}
