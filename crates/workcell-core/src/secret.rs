use std::collections::BTreeMap;
use std::fmt;

use crate::{BindingRef, ExternalRef, ProviderRef, Result, WorkcellError};

pub const SECRET_MATERIALISATION_VERSION: &str = "workcell.secret-materialisation/v1";
pub const SECRET_PROJECTION_VERSION: &str = "workcell.secret-projection/v1";

#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub enum SecretMaterialisationClass {
    ProcessEnv,
    OneShotChildProcess,
    FdOrPipe,
    File,
    ProviderNativeLease,
    CredentialBroker,
    ShortLivedFederatedCredential,
}

impl SecretMaterialisationClass {
    /// Stable wire/ledger name. The ledger and CLI store this string; the
    /// enum stays the in-memory truth.
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::ProcessEnv => "process-env",
            Self::OneShotChildProcess => "one-shot-child-process",
            Self::FdOrPipe => "fd-or-pipe",
            Self::File => "file",
            Self::ProviderNativeLease => "provider-native-lease",
            Self::CredentialBroker => "credential-broker",
            Self::ShortLivedFederatedCredential => "short-lived-federated-credential",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "process-env" => Some(Self::ProcessEnv),
            "one-shot-child-process" => Some(Self::OneShotChildProcess),
            "fd-or-pipe" => Some(Self::FdOrPipe),
            "file" => Some(Self::File),
            "provider-native-lease" => Some(Self::ProviderNativeLease),
            "credential-broker" => Some(Self::CredentialBroker),
            "short-lived-federated-credential" => Some(Self::ShortLivedFederatedCredential),
            _ => None,
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SecretMaterialisationRequest {
    pub credential_ref: ExternalRef,
    pub provider_ref: ProviderRef,
    pub binding_ref: BindingRef,
    pub consumer_ref: ExternalRef,
    pub workload_ref: Option<ExternalRef>,
    pub class: SecretMaterialisationClass,
    pub purpose: String,
    pub destination: String,
    pub scope: String,
}

impl SecretMaterialisationRequest {
    pub fn validate(&self) -> Result<()> {
        for (label, value) in [
            ("purpose", self.purpose.as_str()),
            ("destination", self.destination.as_str()),
            ("scope", self.scope.as_str()),
        ] {
            if value.trim().is_empty() {
                return Err(WorkcellError::InvalidDemand(format!(
                    "secret materialisation {label} must not be empty"
                )));
            }
        }
        Ok(())
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum SecretRefreshRequirement {
    NoneAfterExit,
    RestartRequired,
    BrokerHotSwappable,
    ProviderManaged,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum SecretRevocationState {
    Active,
    Expired,
    Revoked,
}

/// Safe, portable evidence of materialisation. It deliberately cannot contain a secret value.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SecretMaterialReceipt {
    pub version: &'static str,
    pub credential_ref: ExternalRef,
    pub provider_ref: ProviderRef,
    pub binding_ref: BindingRef,
    pub consumer_ref: ExternalRef,
    pub workload_ref: Option<ExternalRef>,
    pub class: SecretMaterialisationClass,
    pub purpose: String,
    pub destination: String,
    pub scope: String,
    pub revision_or_lease_class: Option<String>,
    pub expires_at: Option<String>,
    pub revocation_state: SecretRevocationState,
    pub refresh_requirement: SecretRefreshRequirement,
}

/// Privileged material returned by a SecretProvider to Workcell materialisation code.
/// Debug output is always redacted; callers must deliberately cross the material boundary
/// through `expose_for_materialisation`.
#[derive(Clone, Eq, PartialEq)]
pub struct SecretValue(String);

impl SecretValue {
    pub fn new(value: impl Into<String>) -> Result<Self> {
        let value = value.into();
        if value.is_empty() {
            return Err(WorkcellError::Unavailable(
                "secret provider returned empty material".into(),
            ));
        }
        Ok(Self(value))
    }

    pub fn expose_for_materialisation(&self) -> &str {
        &self.0
    }
}

impl fmt::Debug for SecretValue {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("SecretValue([REDACTED])")
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ProviderSecretMaterial {
    pub value: SecretValue,
    pub revision_or_lease_class: Option<String>,
    pub expires_at: Option<String>,
    pub revocation_state: SecretRevocationState,
}

pub trait SecretProvider {
    fn provider_ref(&self) -> &ProviderRef;

    fn resolve(&self, credential_ref: &ExternalRef) -> Result<ProviderSecretMaterial>;
}

pub fn receipt_for(
    request: &SecretMaterialisationRequest,
    material: &ProviderSecretMaterial,
    refresh_requirement: SecretRefreshRequirement,
) -> SecretMaterialReceipt {
    SecretMaterialReceipt {
        version: SECRET_MATERIALISATION_VERSION,
        credential_ref: request.credential_ref.clone(),
        provider_ref: request.provider_ref.clone(),
        binding_ref: request.binding_ref.clone(),
        consumer_ref: request.consumer_ref.clone(),
        workload_ref: request.workload_ref.clone(),
        class: request.class.clone(),
        purpose: request.purpose.clone(),
        destination: request.destination.clone(),
        scope: request.scope.clone(),
        revision_or_lease_class: material.revision_or_lease_class.clone(),
        expires_at: material.expires_at.clone(),
        revocation_state: material.revocation_state.clone(),
        refresh_requirement,
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct BrokerRoute {
    pub destination_host: String,
    pub method: String,
    pub purpose: String,
    pub scope: String,
}

impl BrokerRoute {
    pub fn validate(&self) -> Result<()> {
        for (label, value) in [
            ("destination_host", self.destination_host.as_str()),
            ("method", self.method.as_str()),
            ("purpose", self.purpose.as_str()),
            ("scope", self.scope.as_str()),
        ] {
            if value.trim().is_empty() {
                return Err(WorkcellError::InvalidDemand(format!(
                    "broker route {label} must not be empty"
                )));
            }
        }
        Ok(())
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct BrokerPolicy {
    routes: Vec<BrokerRoute>,
}

impl BrokerPolicy {
    pub fn new(routes: Vec<BrokerRoute>) -> Result<Self> {
        if routes.is_empty() {
            return Err(WorkcellError::InvalidDemand(
                "broker policy requires at least one route".into(),
            ));
        }
        for route in &routes {
            route.validate()?;
        }
        Ok(Self { routes })
    }

    pub fn authorises(&self, route: &BrokerRoute) -> bool {
        self.routes.iter().any(|allowed| allowed == route)
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct BrokerHandle {
    pub credential_ref: ExternalRef,
    pub binding_ref: BindingRef,
    pub opaque_placeholder: String,
}

pub fn broker_handle(request: &SecretMaterialisationRequest) -> Result<BrokerHandle> {
    request.validate()?;
    if request.class != SecretMaterialisationClass::CredentialBroker {
        return Err(WorkcellError::InvalidDemand(
            "broker handle requires CredentialBroker materialisation class".into(),
        ));
    }
    Ok(BrokerHandle {
        credential_ref: request.credential_ref.clone(),
        binding_ref: request.binding_ref.clone(),
        opaque_placeholder: format!("workcell-broker:{}", request.binding_ref),
    })
}

pub fn authorise_broker_boundary<P: SecretProvider>(
    provider: &P,
    policy: &BrokerPolicy,
    handle: &BrokerHandle,
    request: &SecretMaterialisationRequest,
    route: &BrokerRoute,
) -> Result<(ProviderSecretMaterial, SecretMaterialReceipt)> {
    request.validate()?;
    if request.class != SecretMaterialisationClass::CredentialBroker {
        return Err(WorkcellError::InvalidDemand(
            "broker boundary requires CredentialBroker materialisation class".into(),
        ));
    }
    if handle.credential_ref != request.credential_ref || handle.binding_ref != request.binding_ref
    {
        return Err(WorkcellError::UnsatisfiedDemand(
            "broker handle does not match requested credential/binding".into(),
        ));
    }
    if route.purpose != request.purpose || route.scope != request.scope {
        return Err(WorkcellError::UnsatisfiedDemand(
            "broker route purpose/scope exceeds materialisation grant".into(),
        ));
    }
    if !policy.authorises(route) {
        return Err(WorkcellError::UnsatisfiedDemand(format!(
            "broker route denied: {} {}",
            route.method, route.destination_host
        )));
    }
    if provider.provider_ref() != &request.provider_ref {
        return Err(WorkcellError::UnsatisfiedDemand(
            "selected SecretProvider does not match materialisation request".into(),
        ));
    }

    let material = provider.resolve(&request.credential_ref)?;
    if material.revocation_state != SecretRevocationState::Active {
        return Err(WorkcellError::Unavailable(
            "credential material is expired or revoked".into(),
        ));
    }
    let receipt = receipt_for(
        request,
        &material,
        SecretRefreshRequirement::BrokerHotSwappable,
    );
    Ok((material, receipt))
}

/// Where a projection lands. One origin store stays the single origin; a
/// target receives an authorised reference/materialisation relation, never
/// a copy of the store.
#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub enum SecretProjectionTarget {
    /// Another Workcell cell, reachable through an authorised cross-cell
    /// connection (docs/CROSS-CELL-CONNECTIONS.md). The target receives a
    /// materialisation relation it may present back at use time; material
    /// crosses only an authorised sink boundary, never the control plane.
    Workcell {
        workcell_ref: crate::WorkcellRef,
        connection_label: String,
    },
    /// A sandbox allocation fed through its credential broker (the existing
    /// OpenSandbox Credential Vault seam).
    Sandbox {
        provider_ref: ProviderRef,
        allocation_ref: String,
    },
}

/// The origin-held half of the projection law: one origin, projection not
/// replication, class/purpose/scope fixed at grant time. Persisted by the
/// origin's ledger as refs only; the credential value is not representable
/// in this type.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SecretProjectionRequest {
    pub credential_ref: ExternalRef,
    pub source_provider_ref: ProviderRef,
    pub target: SecretProjectionTarget,
    /// The one materialisation class the target is authorised to receive
    /// the credential through.
    pub class: SecretMaterialisationClass,
    pub purpose: String,
    pub scope: String,
    /// Who requested the projection (agent-session/workload ref).
    pub requested_by: ExternalRef,
}

impl SecretProjectionRequest {
    pub fn validate(&self) -> Result<()> {
        for (label, value) in [
            ("purpose", self.purpose.as_str()),
            ("scope", self.scope.as_str()),
        ] {
            if value.trim().is_empty() {
                return Err(WorkcellError::InvalidDemand(format!(
                    "secret projection {label} must not be empty"
                )));
            }
        }
        match &self.target {
            SecretProjectionTarget::Sandbox { .. } => {
                if self.class != SecretMaterialisationClass::CredentialBroker {
                    return Err(WorkcellError::InvalidDemand(
                        "sandbox projection materialises through the credential broker; \
                         the class must be credential-broker"
                            .into(),
                    ));
                }
            }
            SecretProjectionTarget::Workcell {
                connection_label, ..
            } => {
                if connection_label.trim().is_empty() {
                    return Err(WorkcellError::InvalidDemand(
                        "secret projection connection label must not be empty".into(),
                    ));
                }
            }
        }
        Ok(())
    }
}

/// Portable evidence of a projection. Refs only: the embedded
/// `SecretMaterialReceipt` already cannot carry a value, and nothing here
/// adds one.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SecretProjectionReceipt {
    pub version: &'static str,
    pub credential_ref: ExternalRef,
    pub source_provider_ref: ProviderRef,
    pub target: SecretProjectionTarget,
    pub class: SecretMaterialisationClass,
    pub purpose: String,
    pub scope: String,
    pub requested_by: ExternalRef,
    /// The receipt from the underlying trusted sink, when material moved in
    /// this act. `None` when the projection is a recorded relation only.
    pub materialisation: Option<SecretMaterialReceipt>,
    pub provenance: BTreeMap<String, String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    struct FixedProvider {
        provider_ref: ProviderRef,
        value: &'static str,
    }

    impl SecretProvider for FixedProvider {
        fn provider_ref(&self) -> &ProviderRef {
            &self.provider_ref
        }

        fn resolve(&self, _credential_ref: &ExternalRef) -> Result<ProviderSecretMaterial> {
            Ok(ProviderSecretMaterial {
                value: SecretValue::new(self.value)?,
                revision_or_lease_class: Some("revision:v1".into()),
                expires_at: None,
                revocation_state: SecretRevocationState::Active,
            })
        }
    }

    fn broker_request() -> SecretMaterialisationRequest {
        SecretMaterialisationRequest {
            credential_ref: ExternalRef::new("credential:github/operator").unwrap(),
            provider_ref: ProviderRef::new("secret-provider:fixture").unwrap(),
            binding_ref: BindingRef::new("secret-binding:github/operator").unwrap(),
            consumer_ref: ExternalRef::new("agent-session:fixture").unwrap(),
            workload_ref: Some(ExternalRef::new("workload:fixture").unwrap()),
            class: SecretMaterialisationClass::CredentialBroker,
            purpose: "github-api".into(),
            destination: "broker".into(),
            scope: "repo:read".into(),
        }
    }

    #[test]
    fn secret_debug_is_redacted_and_receipt_carries_no_value() {
        let provider = FixedProvider {
            provider_ref: ProviderRef::new("secret-provider:fixture").unwrap(),
            value: "ghp_RAW_FIXTURE_SECRET",
        };
        let request = broker_request();
        let route = BrokerRoute {
            destination_host: "api.github.com".into(),
            method: "GET".into(),
            purpose: request.purpose.clone(),
            scope: request.scope.clone(),
        };
        let policy = BrokerPolicy::new(vec![route.clone()]).unwrap();
        let handle = broker_handle(&request).unwrap();
        let (material, receipt) =
            authorise_broker_boundary(&provider, &policy, &handle, &request, &route).unwrap();

        assert_eq!(format!("{:?}", material.value), "SecretValue([REDACTED])");
        assert!(!format!("{:?}", receipt).contains("ghp_RAW_FIXTURE_SECRET"));
        assert!(!format!("{:?}", handle).contains("ghp_RAW_FIXTURE_SECRET"));
    }

    #[test]
    fn unapproved_network_route_and_self_widening_are_denied() {
        let provider = FixedProvider {
            provider_ref: ProviderRef::new("secret-provider:fixture").unwrap(),
            value: "ghp_RAW_FIXTURE_SECRET",
        };
        let request = broker_request();
        let allowed = BrokerRoute {
            destination_host: "api.github.com".into(),
            method: "GET".into(),
            purpose: request.purpose.clone(),
            scope: request.scope.clone(),
        };
        let policy = BrokerPolicy::new(vec![allowed]).unwrap();
        let handle = broker_handle(&request).unwrap();

        let attacker = BrokerRoute {
            destination_host: "attacker.invalid".into(),
            method: "POST".into(),
            purpose: request.purpose.clone(),
            scope: request.scope.clone(),
        };
        assert!(
            authorise_broker_boundary(&provider, &policy, &handle, &request, &attacker).is_err()
        );

        let widened = BrokerRoute {
            destination_host: "api.github.com".into(),
            method: "GET".into(),
            purpose: request.purpose.clone(),
            scope: "repo:write".into(),
        };
        assert!(
            authorise_broker_boundary(&provider, &policy, &handle, &request, &widened).is_err()
        );
    }

    fn projection_request(
        target: SecretProjectionTarget,
        class: SecretMaterialisationClass,
    ) -> SecretProjectionRequest {
        SecretProjectionRequest {
            credential_ref: ExternalRef::new("credential:github/operator").unwrap(),
            source_provider_ref: ProviderRef::new("secret-provider:keychain/macos").unwrap(),
            target,
            class,
            purpose: "github-api".into(),
            scope: "repo:read".into(),
            requested_by: ExternalRef::new("agent-session:fixture").unwrap(),
        }
    }

    #[test]
    fn class_names_round_trip_for_the_ledger() {
        for class in [
            SecretMaterialisationClass::ProcessEnv,
            SecretMaterialisationClass::OneShotChildProcess,
            SecretMaterialisationClass::FdOrPipe,
            SecretMaterialisationClass::File,
            SecretMaterialisationClass::ProviderNativeLease,
            SecretMaterialisationClass::CredentialBroker,
            SecretMaterialisationClass::ShortLivedFederatedCredential,
        ] {
            assert_eq!(
                SecretMaterialisationClass::parse(class.as_str()),
                Some(class)
            );
        }
        assert_eq!(SecretMaterialisationClass::parse("nonsense"), None);
    }

    #[test]
    fn sandbox_projection_requires_the_credential_broker_class() {
        let target = SecretProjectionTarget::Sandbox {
            provider_ref: ProviderRef::new("provider:opensandbox").unwrap(),
            allocation_ref: "sbx_fixture".into(),
        };
        assert!(
            projection_request(target.clone(), SecretMaterialisationClass::CredentialBroker)
                .validate()
                .is_ok()
        );
        let err = projection_request(target, SecretMaterialisationClass::ProcessEnv)
            .validate()
            .unwrap_err();
        assert!(format!("{err:?}").contains("credential-broker"));
    }

    #[test]
    fn workcell_projection_requires_a_connection_label() {
        let labelled = SecretProjectionTarget::Workcell {
            workcell_ref: crate::WorkcellRef::new("workcell:remote").unwrap(),
            connection_label: "laptop".into(),
        };
        assert!(
            projection_request(labelled, SecretMaterialisationClass::File)
                .validate()
                .is_ok()
        );

        let blank = SecretProjectionTarget::Workcell {
            workcell_ref: crate::WorkcellRef::new("workcell:remote").unwrap(),
            connection_label: "  ".into(),
        };
        let err = projection_request(blank, SecretMaterialisationClass::File)
            .validate()
            .unwrap_err();
        assert!(format!("{err:?}").contains("connection label"));
    }

    #[test]
    fn projection_request_refuses_empty_purpose_and_scope() {
        let mut request = projection_request(
            SecretProjectionTarget::Sandbox {
                provider_ref: ProviderRef::new("provider:opensandbox").unwrap(),
                allocation_ref: "sbx_fixture".into(),
            },
            SecretMaterialisationClass::CredentialBroker,
        );
        request.purpose = " ".into();
        assert!(request.validate().is_err());
        request.purpose = "github-api".into();
        request.scope = String::new();
        assert!(request.validate().is_err());
    }

    #[test]
    fn projection_receipt_debug_carries_no_material_shape() {
        let request = projection_request(
            SecretProjectionTarget::Sandbox {
                provider_ref: ProviderRef::new("provider:opensandbox").unwrap(),
                allocation_ref: "sbx_fixture".into(),
            },
            SecretMaterialisationClass::CredentialBroker,
        );
        let receipt = SecretProjectionReceipt {
            version: SECRET_PROJECTION_VERSION,
            credential_ref: request.credential_ref.clone(),
            source_provider_ref: request.source_provider_ref.clone(),
            target: request.target.clone(),
            class: request.class.clone(),
            purpose: request.purpose.clone(),
            scope: request.scope.clone(),
            requested_by: request.requested_by.clone(),
            materialisation: None,
            provenance: BTreeMap::from([("secret.visibility".into(), "use-without-read".into())]),
        };
        assert_eq!(receipt.version, SECRET_PROJECTION_VERSION);
        // The receipt type has no value field to leak; the Debug render must
        // stay that way structurally.
        assert!(!format!("{receipt:?}").contains("value"));
    }
}
