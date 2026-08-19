use std::collections::{BTreeMap, BTreeSet};

use epilogos_workcell_core::{
    Degradation, HealthState, LogicalConnectionRequirement, PlanOmission, PlanStatus, ProviderRef,
    RequirementNecessity, Result, WorkcellError, WorkcellRef,
};

/// Logical reachability requested by a semantic/material caller.
///
/// The values describe the relationship, never a provider network name or
/// address. `Any` exists for compatibility with the original string-only
/// `ExecutionDemand.connectivity` floor.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub enum ReachabilityScope {
    Any,
    Local,
    Private,
    Public,
}

impl ReachabilityScope {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Any => "any",
            Self::Local => "local",
            Self::Private => "private",
            Self::Public => "public",
        }
    }
}

/// Material security property required of the path.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub enum NetworkSecurity {
    Any,
    Encrypted,
    Authenticated,
    AuthenticatedEncrypted,
}

impl NetworkSecurity {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Any => "any",
            Self::Encrypted => "encrypted",
            Self::Authenticated => "authenticated",
            Self::AuthenticatedEncrypted => "authenticated+encrypted",
        }
    }
}

/// Stable logical relationship. Material provider/path changes do not change
/// this identity.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct NetworkRelationship {
    pub relationship_ref: String,
    pub source_role: String,
    pub destination_role: String,
    pub transport: Option<String>,
    pub scope: ReachabilityScope,
    pub security: NetworkSecurity,
}

impl NetworkRelationship {
    pub fn new(
        relationship_ref: impl Into<String>,
        source_role: impl Into<String>,
        destination_role: impl Into<String>,
    ) -> Result<Self> {
        let relationship = Self {
            relationship_ref: relationship_ref.into(),
            source_role: source_role.into(),
            destination_role: destination_role.into(),
            transport: None,
            scope: ReachabilityScope::Any,
            security: NetworkSecurity::Any,
        };
        relationship.validate()?;
        Ok(relationship)
    }

    /// Lift the original string connectivity requirement into the structured
    /// relationship seam without changing its caller-owned destination token.
    pub fn from_legacy(requirement: &LogicalConnectionRequirement) -> Result<Self> {
        Self::new(
            format!("connection:{}", requirement.as_str()),
            "execution:world",
            requirement.as_str(),
        )
    }

    pub fn with_transport(mut self, transport: impl Into<String>) -> Result<Self> {
        self.transport = Some(transport.into());
        self.validate()?;
        Ok(self)
    }

    pub fn with_scope(mut self, scope: ReachabilityScope) -> Self {
        self.scope = scope;
        self
    }

    pub fn with_security(mut self, security: NetworkSecurity) -> Self {
        self.security = security;
        self
    }

    pub fn validate(&self) -> Result<()> {
        for (label, value) in [
            ("relationship_ref", self.relationship_ref.as_str()),
            ("source_role", self.source_role.as_str()),
            ("destination_role", self.destination_role.as_str()),
        ] {
            if value.trim().is_empty() {
                return Err(WorkcellError::InvalidDemand(format!(
                    "network relationship {label} must not be empty"
                )));
            }
        }
        if self
            .transport
            .as_deref()
            .is_some_and(|transport| transport.trim().is_empty())
        {
            return Err(WorkcellError::InvalidDemand(
                "network relationship transport must not be empty when supplied".into(),
            ));
        }
        Ok(())
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RequiredNetworkRelationship {
    pub relationship: NetworkRelationship,
    pub necessity: RequirementNecessity,
    pub source_workcell: WorkcellRef,
    pub destination_workcell: WorkcellRef,
}

impl RequiredNetworkRelationship {
    pub fn validate(&self) -> Result<()> {
        self.relationship.validate()
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum FabricPathState {
    Reachable,
    Degraded,
    Denied,
    Unavailable,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct FabricPathOffer {
    pub provider_ref: ProviderRef,
    pub path_ref: String,
    pub source_workcell: WorkcellRef,
    pub destination_workcell: WorkcellRef,
    pub transport: Option<String>,
    pub scope: ReachabilityScope,
    pub security: NetworkSecurity,
    pub state: FabricPathState,
    /// Provider-native endpoint/path locator. This is material provenance only.
    pub endpoint: Option<String>,
    /// Provider-native quality/path class, for example local, direct or relay.
    pub path_class: Option<String>,
    pub provenance: BTreeMap<String, String>,
}

impl FabricPathOffer {
    pub fn validate(&self) -> Result<()> {
        if self.path_ref.trim().is_empty() {
            return Err(WorkcellError::OperationFailed(
                "fabric path_ref must not be empty".into(),
            ));
        }
        if self
            .transport
            .as_deref()
            .is_some_and(|transport| transport.trim().is_empty())
        {
            return Err(WorkcellError::OperationFailed(
                "fabric transport must not be empty when supplied".into(),
            ));
        }
        Ok(())
    }
}

/// Public authoring seam for material connectivity providers.
///
/// This is deliberately not a generic VPN/mesh abstraction and does not add a
/// core ProviderPortKind until a real provider lifecycle proves that necessary.
pub trait FabricPathProvider {
    fn provider_ref(&self) -> &ProviderRef;
    fn paths(&self) -> Result<Vec<FabricPathOffer>>;
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum FabricPolicyResult {
    Allowed,
    Denied,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct MaterialFabricBinding {
    pub relationship_ref: String,
    pub provider_ref: ProviderRef,
    pub path_ref: String,
    pub source_workcell: WorkcellRef,
    pub destination_workcell: WorkcellRef,
    pub transport: Option<String>,
    pub scope: ReachabilityScope,
    pub security: NetworkSecurity,
    pub endpoint: Option<String>,
    pub path_class: Option<String>,
    pub policy: FabricPolicyResult,
    pub health: HealthState,
    pub provenance: BTreeMap<String, String>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum FabricDiagnosticKind {
    ProviderUnavailable,
    PolicyDenied,
    DestinationUnavailable,
    ScopeMismatch,
    SecurityMismatch,
    TransportMismatch,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct FabricDiagnostic {
    pub relationship_ref: String,
    pub provider_ref: Option<ProviderRef>,
    pub kind: FabricDiagnosticKind,
    pub detail: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct FabricPlan {
    pub status: PlanStatus,
    pub bindings: Vec<MaterialFabricBinding>,
    pub degradations: Vec<Degradation>,
    pub omissions: Vec<PlanOmission>,
    pub diagnostics: Vec<FabricDiagnostic>,
}

pub fn evaluate_fabric(
    relationships: &[RequiredNetworkRelationship],
    providers: &[&dyn FabricPathProvider],
) -> Result<FabricPlan> {
    let mut relationship_refs = BTreeSet::new();
    for required in relationships {
        required.validate()?;
        if !relationship_refs.insert(required.relationship.relationship_ref.clone()) {
            return Err(WorkcellError::InvalidDemand(format!(
                "network relationship `{}` appears more than once",
                required.relationship.relationship_ref
            )));
        }
    }

    let mut paths = Vec::new();
    for provider in providers {
        for path in provider.paths()? {
            path.validate()?;
            if path.provider_ref != *provider.provider_ref() {
                return Err(WorkcellError::OperationFailed(format!(
                    "fabric provider `{}` returned path `{}` with provider identity `{}`",
                    provider.provider_ref(),
                    path.path_ref,
                    path.provider_ref
                )));
            }
            paths.push(path);
        }
    }
    paths.sort_by(|left, right| {
        path_rank(left.state)
            .cmp(&path_rank(right.state))
            .reverse()
            .then_with(|| left.provider_ref.cmp(&right.provider_ref))
            .then_with(|| left.path_ref.cmp(&right.path_ref))
    });

    let mut plan = FabricPlan {
        status: PlanStatus::Satisfiable,
        bindings: Vec::new(),
        degradations: Vec::new(),
        omissions: Vec::new(),
        diagnostics: Vec::new(),
    };

    for required in relationships {
        let relation = &required.relationship;
        let mut relation_diagnostics = Vec::new();
        let mut selected = None;

        for path in &paths {
            if path.source_workcell != required.source_workcell
                || path.destination_workcell != required.destination_workcell
            {
                continue;
            }
            if !transport_matches(relation, path) {
                relation_diagnostics.push(diagnostic(
                    relation,
                    Some(path.provider_ref.clone()),
                    FabricDiagnosticKind::TransportMismatch,
                    format!(
                        "path `{}` transport {:?} does not satisfy {:?}",
                        path.path_ref, path.transport, relation.transport
                    ),
                ));
                continue;
            }
            if !scope_matches(relation.scope, path.scope) {
                relation_diagnostics.push(diagnostic(
                    relation,
                    Some(path.provider_ref.clone()),
                    FabricDiagnosticKind::ScopeMismatch,
                    format!(
                        "path `{}` scope `{}` does not satisfy `{}`",
                        path.path_ref,
                        path.scope.as_str(),
                        relation.scope.as_str()
                    ),
                ));
                continue;
            }
            if !security_matches(relation.security, path.security) {
                relation_diagnostics.push(diagnostic(
                    relation,
                    Some(path.provider_ref.clone()),
                    FabricDiagnosticKind::SecurityMismatch,
                    format!(
                        "path `{}` security `{}` does not satisfy `{}`",
                        path.path_ref,
                        path.security.as_str(),
                        relation.security.as_str()
                    ),
                ));
                continue;
            }
            match path.state {
                FabricPathState::Denied => {
                    relation_diagnostics.push(diagnostic(
                        relation,
                        Some(path.provider_ref.clone()),
                        FabricDiagnosticKind::PolicyDenied,
                        format!("path `{}` is denied by provider policy", path.path_ref),
                    ));
                }
                FabricPathState::Unavailable => {
                    relation_diagnostics.push(diagnostic(
                        relation,
                        Some(path.provider_ref.clone()),
                        FabricDiagnosticKind::ProviderUnavailable,
                        format!("path `{}` is unavailable", path.path_ref),
                    ));
                }
                FabricPathState::Reachable | FabricPathState::Degraded => {
                    selected = Some(path);
                    break;
                }
            }
        }

        if let Some(path) = selected {
            let health = match path.state {
                FabricPathState::Reachable => HealthState::Healthy,
                FabricPathState::Degraded => HealthState::Degraded,
                FabricPathState::Denied | FabricPathState::Unavailable => unreachable!(),
            };
            plan.bindings.push(MaterialFabricBinding {
                relationship_ref: relation.relationship_ref.clone(),
                provider_ref: path.provider_ref.clone(),
                path_ref: path.path_ref.clone(),
                source_workcell: path.source_workcell.clone(),
                destination_workcell: path.destination_workcell.clone(),
                transport: path.transport.clone(),
                scope: path.scope,
                security: path.security,
                endpoint: path.endpoint.clone(),
                path_class: path.path_class.clone(),
                policy: FabricPolicyResult::Allowed,
                health: health.clone(),
                provenance: path.provenance.clone(),
            });
            if health == HealthState::Degraded {
                plan.status = PlanStatus::Degraded;
                plan.degradations.push(Degradation {
                    requirement: relation.relationship_ref.clone(),
                    necessity: required.necessity,
                    reason: format!(
                        "material fabric provider `{}` reports degraded path `{}`",
                        path.provider_ref, path.path_ref
                    ),
                });
            }
            plan.diagnostics.extend(relation_diagnostics);
            continue;
        }

        if relation_diagnostics.is_empty() {
            relation_diagnostics.push(diagnostic(
                relation,
                None,
                FabricDiagnosticKind::DestinationUnavailable,
                format!(
                    "no material path connects `{}` to `{}`",
                    required.source_workcell, required.destination_workcell
                ),
            ));
        }
        let reason = relation_diagnostics
            .iter()
            .map(|item| item.detail.as_str())
            .collect::<Vec<_>>()
            .join("; ");
        plan.diagnostics.extend(relation_diagnostics);
        match required.necessity {
            RequirementNecessity::Required => {
                plan.status = PlanStatus::Unsatisfiable;
                plan.degradations.push(Degradation {
                    requirement: relation.relationship_ref.clone(),
                    necessity: required.necessity,
                    reason,
                });
            }
            RequirementNecessity::Preferred => {
                if plan.status != PlanStatus::Unsatisfiable {
                    plan.status = PlanStatus::Degraded;
                }
                plan.degradations.push(Degradation {
                    requirement: relation.relationship_ref.clone(),
                    necessity: required.necessity,
                    reason,
                });
            }
            RequirementNecessity::Optional => {
                plan.omissions.push(PlanOmission {
                    requirement: relation.relationship_ref.clone(),
                    necessity: required.necessity,
                    reason,
                });
            }
        }
    }

    Ok(plan)
}

/// Required network relationships must be satisfiable before material
/// preparation can begin.
pub fn require_fabric_plan(plan: FabricPlan) -> Result<FabricPlan> {
    if plan.status == PlanStatus::Unsatisfiable {
        let detail = plan
            .degradations
            .iter()
            .filter(|item| item.necessity == RequirementNecessity::Required)
            .map(|item| format!("{}: {}", item.requirement, item.reason))
            .collect::<Vec<_>>()
            .join("; ");
        return Err(WorkcellError::UnsatisfiedDemand(format!(
            "required material connectivity is not realisable before prepare: {detail}"
        )));
    }
    Ok(plan)
}

fn diagnostic(
    relation: &NetworkRelationship,
    provider_ref: Option<ProviderRef>,
    kind: FabricDiagnosticKind,
    detail: String,
) -> FabricDiagnostic {
    FabricDiagnostic {
        relationship_ref: relation.relationship_ref.clone(),
        provider_ref,
        kind,
        detail,
    }
}

fn path_rank(state: FabricPathState) -> u8 {
    match state {
        FabricPathState::Reachable => 3,
        FabricPathState::Degraded => 2,
        FabricPathState::Denied => 1,
        FabricPathState::Unavailable => 0,
    }
}

fn transport_matches(relation: &NetworkRelationship, path: &FabricPathOffer) -> bool {
    match (&relation.transport, &path.transport) {
        (None, _) => true,
        (Some(required), Some(available)) => required.eq_ignore_ascii_case(available),
        (Some(_), None) => false,
    }
}

fn scope_matches(required: ReachabilityScope, available: ReachabilityScope) -> bool {
    required == ReachabilityScope::Any || required == available
}

fn security_matches(required: NetworkSecurity, available: NetworkSecurity) -> bool {
    match required {
        NetworkSecurity::Any => true,
        NetworkSecurity::Encrypted => matches!(
            available,
            NetworkSecurity::Encrypted | NetworkSecurity::AuthenticatedEncrypted
        ),
        NetworkSecurity::Authenticated => matches!(
            available,
            NetworkSecurity::Authenticated | NetworkSecurity::AuthenticatedEncrypted
        ),
        NetworkSecurity::AuthenticatedEncrypted => {
            available == NetworkSecurity::AuthenticatedEncrypted
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct FixtureProvider {
        provider_ref: ProviderRef,
        paths: Vec<FabricPathOffer>,
    }

    impl FixtureProvider {
        fn new(name: &str, paths: Vec<FabricPathOffer>) -> Self {
            Self {
                provider_ref: ProviderRef::new(name).unwrap(),
                paths,
            }
        }
    }

    impl FabricPathProvider for FixtureProvider {
        fn provider_ref(&self) -> &ProviderRef {
            &self.provider_ref
        }

        fn paths(&self) -> Result<Vec<FabricPathOffer>> {
            Ok(self.paths.clone())
        }
    }

    fn workcell(name: &str) -> WorkcellRef {
        WorkcellRef::new(name).unwrap()
    }

    fn provider(name: &str) -> ProviderRef {
        ProviderRef::new(name).unwrap()
    }

    fn path(
        provider_ref: &str,
        path_ref: &str,
        source: &str,
        destination: &str,
        scope: ReachabilityScope,
        state: FabricPathState,
    ) -> FabricPathOffer {
        FabricPathOffer {
            provider_ref: provider(provider_ref),
            path_ref: path_ref.into(),
            source_workcell: workcell(source),
            destination_workcell: workcell(destination),
            transport: Some("tcp".into()),
            scope,
            security: NetworkSecurity::AuthenticatedEncrypted,
            state,
            endpoint: Some(format!("material://{path_ref}")),
            path_class: Some("fixture".into()),
            provenance: BTreeMap::from([("fixture".into(), "true".into())]),
        }
    }

    fn relationship(
        relationship_ref: &str,
        source: &str,
        destination: &str,
        necessity: RequirementNecessity,
        scope: ReachabilityScope,
    ) -> RequiredNetworkRelationship {
        RequiredNetworkRelationship {
            relationship: NetworkRelationship::new(
                relationship_ref,
                "execution:world",
                "service:target",
            )
            .unwrap()
            .with_transport("tcp")
            .unwrap()
            .with_scope(scope)
            .with_security(NetworkSecurity::AuthenticatedEncrypted),
            necessity,
            source_workcell: workcell(source),
            destination_workcell: workcell(destination),
        }
    }

    #[test]
    fn local_and_private_overlay_realise_the_same_logical_relationship() {
        let relation = relationship(
            "relationship:service",
            "workcell:a",
            "workcell:b",
            RequirementNecessity::Required,
            ReachabilityScope::Private,
        );
        let overlay = FixtureProvider::new(
            "provider:overlay",
            vec![path(
                "provider:overlay",
                "path:overlay-1",
                "workcell:a",
                "workcell:b",
                ReachabilityScope::Private,
                FabricPathState::Reachable,
            )],
        );
        let first = require_fabric_plan(evaluate_fabric(&[relation.clone()], &[&overlay]).unwrap())
            .unwrap();
        assert_eq!(first.bindings[0].relationship_ref, "relationship:service");

        let replacement = FixtureProvider::new(
            "provider:replacement-overlay",
            vec![path(
                "provider:replacement-overlay",
                "path:overlay-2",
                "workcell:a",
                "workcell:b",
                ReachabilityScope::Private,
                FabricPathState::Reachable,
            )],
        );
        let second =
            require_fabric_plan(evaluate_fabric(&[relation], &[&replacement]).unwrap()).unwrap();
        assert_eq!(second.bindings[0].relationship_ref, "relationship:service");
        assert_ne!(
            first.bindings[0].provider_ref,
            second.bindings[0].provider_ref
        );
        assert_ne!(first.bindings[0].path_ref, second.bindings[0].path_ref);
    }

    #[test]
    fn required_unavailable_path_fails_before_prepare() {
        let relation = relationship(
            "relationship:required",
            "workcell:a",
            "workcell:b",
            RequirementNecessity::Required,
            ReachabilityScope::Private,
        );
        let unavailable = FixtureProvider::new(
            "provider:overlay",
            vec![path(
                "provider:overlay",
                "path:down",
                "workcell:a",
                "workcell:b",
                ReachabilityScope::Private,
                FabricPathState::Unavailable,
            )],
        );
        let plan = evaluate_fabric(&[relation], &[&unavailable]).unwrap();
        assert_eq!(plan.status, PlanStatus::Unsatisfiable);
        assert!(matches!(
            require_fabric_plan(plan),
            Err(WorkcellError::UnsatisfiedDemand(_))
        ));
    }

    #[test]
    fn policy_denial_is_distinct_from_provider_absence() {
        let relation = relationship(
            "relationship:denied",
            "workcell:a",
            "workcell:b",
            RequirementNecessity::Required,
            ReachabilityScope::Private,
        );
        let denied = FixtureProvider::new(
            "provider:overlay",
            vec![path(
                "provider:overlay",
                "path:denied",
                "workcell:a",
                "workcell:b",
                ReachabilityScope::Private,
                FabricPathState::Denied,
            )],
        );
        let denied_plan = evaluate_fabric(&[relation.clone()], &[&denied]).unwrap();
        assert!(denied_plan
            .diagnostics
            .iter()
            .any(|item| item.kind == FabricDiagnosticKind::PolicyDenied));

        let absent = evaluate_fabric(&[relation], &[]).unwrap();
        assert!(absent
            .diagnostics
            .iter()
            .any(|item| item.kind == FabricDiagnosticKind::DestinationUnavailable));
    }

    #[test]
    fn public_exposure_cannot_silently_satisfy_private_reachability() {
        let relation = relationship(
            "relationship:private",
            "workcell:a",
            "workcell:b",
            RequirementNecessity::Required,
            ReachabilityScope::Private,
        );
        let public_only = FixtureProvider::new(
            "provider:public",
            vec![path(
                "provider:public",
                "path:public",
                "workcell:a",
                "workcell:b",
                ReachabilityScope::Public,
                FabricPathState::Reachable,
            )],
        );
        let plan = evaluate_fabric(&[relation], &[&public_only]).unwrap();
        assert_eq!(plan.status, PlanStatus::Unsatisfiable);
        assert!(plan
            .diagnostics
            .iter()
            .any(|item| item.kind == FabricDiagnosticKind::ScopeMismatch));
    }

    #[test]
    fn preferred_loss_degrades_and_optional_loss_is_omitted() {
        let preferred = relationship(
            "relationship:preferred",
            "workcell:a",
            "workcell:b",
            RequirementNecessity::Preferred,
            ReachabilityScope::Private,
        );
        let optional = relationship(
            "relationship:optional",
            "workcell:a",
            "workcell:c",
            RequirementNecessity::Optional,
            ReachabilityScope::Private,
        );
        let plan = evaluate_fabric(&[preferred, optional], &[]).unwrap();
        assert_eq!(plan.status, PlanStatus::Degraded);
        assert_eq!(plan.degradations.len(), 1);
        assert_eq!(plan.omissions.len(), 1);
    }

    #[test]
    fn tunnel_style_transport_is_material_path_not_relationship_identity() {
        let relation = relationship(
            "relationship:control",
            "workcell:client",
            "workcell:server",
            RequirementNecessity::Required,
            ReachabilityScope::Private,
        );
        let mut tunnel_path = path(
            "provider:ssh-tunnel-fixture",
            "path:tunnel-1",
            "workcell:client",
            "workcell:server",
            ReachabilityScope::Private,
            FabricPathState::Reachable,
        );
        tunnel_path.path_class = Some("ssh-tunnel-style".into());
        tunnel_path.endpoint = Some("127.0.0.1:49152".into());
        let tunnel = FixtureProvider::new("provider:ssh-tunnel-fixture", vec![tunnel_path]);
        let plan = require_fabric_plan(evaluate_fabric(&[relation], &[&tunnel]).unwrap()).unwrap();
        assert_eq!(plan.bindings[0].relationship_ref, "relationship:control");
        assert_eq!(
            plan.bindings[0].path_class.as_deref(),
            Some("ssh-tunnel-style")
        );
        assert_eq!(
            plan.bindings[0].endpoint.as_deref(),
            Some("127.0.0.1:49152")
        );
    }
}
