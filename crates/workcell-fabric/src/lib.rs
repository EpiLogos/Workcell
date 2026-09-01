use std::collections::{BTreeMap, BTreeSet};

use epilogos_workcell_core::{
    Degradation, ExternalRef, HealthState, LogicalConnectionRequirement, PlanOmission, PlanStatus,
    ProviderRef, RequirementNecessity, Result, WorkcellError, WorkcellRef,
};

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

/// Logical end of a requested network relationship.
///
/// An External endpoint is caller-owned identity such as `service:github-api`;
/// it is not a provider IP, DNS route, tailnet peer or sandbox endpoint.
#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub enum NetworkEndpoint {
    Workcell(WorkcellRef),
    External(ExternalRef),
}

impl NetworkEndpoint {
    pub fn workcell(workcell_ref: WorkcellRef) -> Self {
        Self::Workcell(workcell_ref)
    }

    pub fn external(endpoint_ref: impl Into<String>) -> Result<Self> {
        Ok(Self::External(ExternalRef::new(endpoint_ref.into())?))
    }

    pub fn label(&self) -> String {
        match self {
            Self::Workcell(value) => value.to_string(),
            Self::External(value) => value.to_string(),
        }
    }
}

/// Stable logical relationship. Material path and policy providers may change
/// without changing this identity.
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
    pub source: NetworkEndpoint,
    pub destination: NetworkEndpoint,
    /// True when materialisation must include an explicit policy-enforcement
    /// decision in addition to a reachable path.
    pub requires_policy: bool,
}

impl RequiredNetworkRelationship {
    pub fn between_workcells(
        relationship: NetworkRelationship,
        necessity: RequirementNecessity,
        source: WorkcellRef,
        destination: WorkcellRef,
    ) -> Self {
        Self {
            relationship,
            necessity,
            source: NetworkEndpoint::Workcell(source),
            destination: NetworkEndpoint::Workcell(destination),
            requires_policy: false,
        }
    }

    pub fn to_external(
        relationship: NetworkRelationship,
        necessity: RequirementNecessity,
        source: WorkcellRef,
        destination: ExternalRef,
    ) -> Self {
        Self {
            relationship,
            necessity,
            source: NetworkEndpoint::Workcell(source),
            destination: NetworkEndpoint::External(destination),
            requires_policy: true,
        }
    }

    pub fn with_required_policy(mut self) -> Self {
        self.requires_policy = true;
        self
    }

    pub fn validate(&self) -> Result<()> {
        self.relationship.validate()?;
        if self.source == self.destination {
            return Err(WorkcellError::InvalidDemand(
                "network relationship source and destination must be distinct".into(),
            ));
        }
        Ok(())
    }
}

/// Observed material reachability only. Policy denial is deliberately not a
/// path state; a denied policy can coexist with a healthy route.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum FabricPathState {
    Reachable,
    Degraded,
    Unavailable,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct FabricPathOffer {
    pub provider_ref: ProviderRef,
    pub path_ref: String,
    pub source: NetworkEndpoint,
    pub destination: NetworkEndpoint,
    pub transport: Option<String>,
    pub scope: ReachabilityScope,
    pub security: NetworkSecurity,
    pub state: FabricPathState,
    /// Provider-native endpoint/path locator. Material provenance only.
    pub endpoint: Option<String>,
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
        if self.source == self.destination {
            return Err(WorkcellError::OperationFailed(
                "fabric path source and destination must be distinct".into(),
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

pub trait FabricPathProvider {
    fn provider_ref(&self) -> &ProviderRef;
    fn paths(&self) -> Result<Vec<FabricPathOffer>>;
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum FabricPolicyState {
    Allowed,
    Denied,
    Unavailable,
}

/// Independently observed material policy enforcement. This may be supplied by
/// the same underlying product as a path provider, or by a distinct sidecar,
/// firewall, mesh or credential/egress layer.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct FabricPolicyOffer {
    pub provider_ref: ProviderRef,
    pub policy_ref: String,
    pub relationship_ref: Option<String>,
    pub source: NetworkEndpoint,
    pub destination: NetworkEndpoint,
    pub state: FabricPolicyState,
    pub provenance: BTreeMap<String, String>,
}

impl FabricPolicyOffer {
    pub fn validate(&self) -> Result<()> {
        if self.policy_ref.trim().is_empty() {
            return Err(WorkcellError::OperationFailed(
                "fabric policy_ref must not be empty".into(),
            ));
        }
        if self
            .relationship_ref
            .as_deref()
            .is_some_and(|value| value.trim().is_empty())
        {
            return Err(WorkcellError::OperationFailed(
                "fabric policy relationship_ref must not be empty when supplied".into(),
            ));
        }
        Ok(())
    }
}

pub trait FabricPolicyProvider {
    fn provider_ref(&self) -> &ProviderRef;
    fn policies(&self) -> Result<Vec<FabricPolicyOffer>>;
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum FabricPolicyResult {
    NotRequired,
    Allowed,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct MaterialFabricBinding {
    pub relationship_ref: String,
    pub path_provider_ref: ProviderRef,
    pub path_ref: String,
    pub source: NetworkEndpoint,
    pub destination: NetworkEndpoint,
    pub transport: Option<String>,
    pub scope: ReachabilityScope,
    pub security: NetworkSecurity,
    pub endpoint: Option<String>,
    pub path_class: Option<String>,
    pub policy: FabricPolicyResult,
    pub policy_provider_ref: Option<ProviderRef>,
    pub policy_ref: Option<String>,
    pub health: HealthState,
    pub path_provenance: BTreeMap<String, String>,
    pub policy_provenance: BTreeMap<String, String>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum FabricDiagnosticKind {
    ProviderUnavailable,
    PolicyDenied,
    PolicyUnavailable,
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

/// Compatibility surface for relationships that do not require an independent
/// policy enforcer.
pub fn evaluate_fabric(
    relationships: &[RequiredNetworkRelationship],
    providers: &[&dyn FabricPathProvider],
) -> Result<FabricPlan> {
    evaluate_fabric_with_policies(relationships, providers, &[])
}

pub fn evaluate_fabric_with_policies(
    relationships: &[RequiredNetworkRelationship],
    path_providers: &[&dyn FabricPathProvider],
    policy_providers: &[&dyn FabricPolicyProvider],
) -> Result<FabricPlan> {
    validate_relationships(relationships)?;
    let mut paths = collect_paths(path_providers)?;
    let policies = collect_policies(policy_providers)?;
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
        evaluate_relationship(required, &paths, &policies, &mut plan)?;
    }
    Ok(plan)
}

fn validate_relationships(relationships: &[RequiredNetworkRelationship]) -> Result<()> {
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
    Ok(())
}

fn collect_paths(providers: &[&dyn FabricPathProvider]) -> Result<Vec<FabricPathOffer>> {
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
    Ok(paths)
}

fn collect_policies(providers: &[&dyn FabricPolicyProvider]) -> Result<Vec<FabricPolicyOffer>> {
    let mut policies = Vec::new();
    for provider in providers {
        for policy in provider.policies()? {
            policy.validate()?;
            if policy.provider_ref != *provider.provider_ref() {
                return Err(WorkcellError::OperationFailed(format!(
                    "fabric policy provider `{}` returned policy `{}` with provider identity `{}`",
                    provider.provider_ref(),
                    policy.policy_ref,
                    policy.provider_ref
                )));
            }
            policies.push(policy);
        }
    }
    Ok(policies)
}

fn evaluate_relationship(
    required: &RequiredNetworkRelationship,
    paths: &[FabricPathOffer],
    policies: &[FabricPolicyOffer],
    plan: &mut FabricPlan,
) -> Result<()> {
    let relation = &required.relationship;
    let mut diagnostics = Vec::new();
    let mut selected_path = None;

    for path in paths {
        if path.source != required.source || path.destination != required.destination {
            continue;
        }
        if !transport_matches(relation, path) {
            diagnostics.push(diagnostic(
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
            diagnostics.push(diagnostic(
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
            diagnostics.push(diagnostic(
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
            FabricPathState::Unavailable => diagnostics.push(diagnostic(
                relation,
                Some(path.provider_ref.clone()),
                FabricDiagnosticKind::ProviderUnavailable,
                format!("path `{}` is unavailable", path.path_ref),
            )),
            FabricPathState::Reachable | FabricPathState::Degraded => {
                selected_path = Some(path);
                break;
            }
        }
    }

    let Some(path) = selected_path else {
        if diagnostics.is_empty() {
            diagnostics.push(diagnostic(
                relation,
                None,
                FabricDiagnosticKind::DestinationUnavailable,
                format!(
                    "no material path connects `{}` to `{}`",
                    required.source.label(),
                    required.destination.label()
                ),
            ));
        }
        return record_unsatisfied(required, diagnostics, plan);
    };

    let policy = if required.requires_policy {
        match select_policy(required, policies) {
            Ok(policy) => Some(policy),
            Err(mut policy_diagnostics) => {
                diagnostics.append(&mut policy_diagnostics);
                return record_unsatisfied(required, diagnostics, plan);
            }
        }
    } else {
        None
    };

    let health = match path.state {
        FabricPathState::Reachable => HealthState::Healthy,
        FabricPathState::Degraded => HealthState::Degraded,
        FabricPathState::Unavailable => unreachable!(),
    };
    plan.bindings.push(MaterialFabricBinding {
        relationship_ref: relation.relationship_ref.clone(),
        path_provider_ref: path.provider_ref.clone(),
        path_ref: path.path_ref.clone(),
        source: path.source.clone(),
        destination: path.destination.clone(),
        transport: path.transport.clone(),
        scope: path.scope,
        security: path.security,
        endpoint: path.endpoint.clone(),
        path_class: path.path_class.clone(),
        policy: if policy.is_some() {
            FabricPolicyResult::Allowed
        } else {
            FabricPolicyResult::NotRequired
        },
        policy_provider_ref: policy.map(|value| value.provider_ref.clone()),
        policy_ref: policy.map(|value| value.policy_ref.clone()),
        health: health.clone(),
        path_provenance: path.provenance.clone(),
        policy_provenance: policy
            .map(|value| value.provenance.clone())
            .unwrap_or_default(),
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
    plan.diagnostics.extend(diagnostics);
    Ok(())
}

fn select_policy<'a>(
    required: &RequiredNetworkRelationship,
    policies: &'a [FabricPolicyOffer],
) -> std::result::Result<&'a FabricPolicyOffer, Vec<FabricDiagnostic>> {
    let relation = &required.relationship;
    let mut matching = policies
        .iter()
        .filter(|policy| {
            policy.source == required.source
                && policy.destination == required.destination
                && policy
                    .relationship_ref
                    .as_deref()
                    .is_none_or(|value| value == relation.relationship_ref)
        })
        .collect::<Vec<_>>();
    matching.sort_by(|left, right| {
        policy_rank(left.state)
            .cmp(&policy_rank(right.state))
            .reverse()
            .then_with(|| left.provider_ref.cmp(&right.provider_ref))
            .then_with(|| left.policy_ref.cmp(&right.policy_ref))
    });

    if let Some(denied) = matching
        .iter()
        .copied()
        .find(|policy| policy.state == FabricPolicyState::Denied)
    {
        return Err(vec![diagnostic(
            relation,
            Some(denied.provider_ref.clone()),
            FabricDiagnosticKind::PolicyDenied,
            format!(
                "reachable path is denied by material policy `{}` from `{}`",
                denied.policy_ref, denied.provider_ref
            ),
        )]);
    }
    if let Some(allowed) = matching
        .iter()
        .copied()
        .find(|policy| policy.state == FabricPolicyState::Allowed)
    {
        return Ok(allowed);
    }
    let detail = if let Some(unavailable) = matching.first() {
        format!(
            "material policy `{}` from `{}` is unavailable",
            unavailable.policy_ref, unavailable.provider_ref
        )
    } else {
        format!(
            "no material policy provider covers `{}` to `{}`",
            required.source.label(),
            required.destination.label()
        )
    };
    Err(vec![diagnostic(
        relation,
        matching.first().map(|value| value.provider_ref.clone()),
        FabricDiagnosticKind::PolicyUnavailable,
        detail,
    )])
}

fn record_unsatisfied(
    required: &RequiredNetworkRelationship,
    diagnostics: Vec<FabricDiagnostic>,
    plan: &mut FabricPlan,
) -> Result<()> {
    let reason = diagnostics
        .iter()
        .map(|item| item.detail.as_str())
        .collect::<Vec<_>>()
        .join("; ");
    plan.diagnostics.extend(diagnostics);
    match required.necessity {
        RequirementNecessity::Required => {
            plan.status = PlanStatus::Unsatisfiable;
            plan.degradations.push(Degradation {
                requirement: required.relationship.relationship_ref.clone(),
                necessity: required.necessity,
                reason,
            });
        }
        RequirementNecessity::Preferred => {
            if plan.status != PlanStatus::Unsatisfiable {
                plan.status = PlanStatus::Degraded;
            }
            plan.degradations.push(Degradation {
                requirement: required.relationship.relationship_ref.clone(),
                necessity: required.necessity,
                reason,
            });
        }
        RequirementNecessity::Optional => plan.omissions.push(PlanOmission {
            requirement: required.relationship.relationship_ref.clone(),
            necessity: required.necessity,
            reason,
        }),
    }
    Ok(())
}

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
        FabricPathState::Reachable => 2,
        FabricPathState::Degraded => 1,
        FabricPathState::Unavailable => 0,
    }
}

fn policy_rank(state: FabricPolicyState) -> u8 {
    match state {
        FabricPolicyState::Denied => 3,
        FabricPolicyState::Allowed => 2,
        FabricPolicyState::Unavailable => 1,
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

    struct PathFixture {
        provider_ref: ProviderRef,
        paths: Vec<FabricPathOffer>,
    }

    impl FabricPathProvider for PathFixture {
        fn provider_ref(&self) -> &ProviderRef {
            &self.provider_ref
        }

        fn paths(&self) -> Result<Vec<FabricPathOffer>> {
            Ok(self.paths.clone())
        }
    }

    struct PolicyFixture {
        provider_ref: ProviderRef,
        policies: Vec<FabricPolicyOffer>,
    }

    impl FabricPolicyProvider for PolicyFixture {
        fn provider_ref(&self) -> &ProviderRef {
            &self.provider_ref
        }

        fn policies(&self) -> Result<Vec<FabricPolicyOffer>> {
            Ok(self.policies.clone())
        }
    }

    fn workcell(name: &str) -> WorkcellRef {
        WorkcellRef::new(name).unwrap()
    }

    fn provider(name: &str) -> ProviderRef {
        ProviderRef::new(name).unwrap()
    }

    fn workcell_endpoint(name: &str) -> NetworkEndpoint {
        NetworkEndpoint::Workcell(workcell(name))
    }

    fn external_endpoint(name: &str) -> NetworkEndpoint {
        NetworkEndpoint::External(ExternalRef::new(name).unwrap())
    }

    fn path(
        provider_ref: &str,
        path_ref: &str,
        source: NetworkEndpoint,
        destination: NetworkEndpoint,
        scope: ReachabilityScope,
        state: FabricPathState,
    ) -> FabricPathOffer {
        FabricPathOffer {
            provider_ref: provider(provider_ref),
            path_ref: path_ref.into(),
            source,
            destination,
            transport: Some("tcp".into()),
            scope,
            security: NetworkSecurity::AuthenticatedEncrypted,
            state,
            endpoint: Some(format!("material://{path_ref}")),
            path_class: Some("fixture".into()),
            provenance: BTreeMap::from([("fixture".into(), "path".into())]),
        }
    }

    fn relationship(
        relationship_ref: &str,
        source: NetworkEndpoint,
        destination: NetworkEndpoint,
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
            source,
            destination,
            requires_policy: false,
        }
    }

    #[test]
    fn provider_replacement_does_not_change_logical_relationship_identity() {
        let relation = relationship(
            "relationship:service",
            workcell_endpoint("workcell:a"),
            workcell_endpoint("workcell:b"),
            RequirementNecessity::Required,
            ReachabilityScope::Private,
        );
        let first_provider = PathFixture {
            provider_ref: provider("provider:first"),
            paths: vec![path(
                "provider:first",
                "path:first",
                relation.source.clone(),
                relation.destination.clone(),
                ReachabilityScope::Private,
                FabricPathState::Reachable,
            )],
        };
        let first = require_fabric_plan(
            evaluate_fabric(std::slice::from_ref(&relation), &[&first_provider]).unwrap(),
        )
        .unwrap();

        let replacement = PathFixture {
            provider_ref: provider("provider:replacement"),
            paths: vec![path(
                "provider:replacement",
                "path:replacement",
                relation.source.clone(),
                relation.destination.clone(),
                ReachabilityScope::Private,
                FabricPathState::Reachable,
            )],
        };
        let second =
            require_fabric_plan(evaluate_fabric(&[relation], &[&replacement]).unwrap()).unwrap();
        assert_eq!(first.bindings[0].relationship_ref, second.bindings[0].relationship_ref);
        assert_ne!(
            first.bindings[0].path_provider_ref,
            second.bindings[0].path_provider_ref
        );
    }

    #[test]
    fn required_unavailable_path_fails_before_prepare() {
        let relation = relationship(
            "relationship:required",
            workcell_endpoint("workcell:a"),
            workcell_endpoint("workcell:b"),
            RequirementNecessity::Required,
            ReachabilityScope::Private,
        );
        let unavailable = PathFixture {
            provider_ref: provider("provider:path"),
            paths: vec![path(
                "provider:path",
                "path:down",
                relation.source.clone(),
                relation.destination.clone(),
                ReachabilityScope::Private,
                FabricPathState::Unavailable,
            )],
        };
        let plan = evaluate_fabric(&[relation], &[&unavailable]).unwrap();
        assert_eq!(plan.status, PlanStatus::Unsatisfiable);
        assert!(matches!(
            require_fabric_plan(plan),
            Err(WorkcellError::UnsatisfiedDemand(_))
        ));
    }

    #[test]
    fn external_policy_denial_is_distinct_from_path_absence() {
        let mut relation = relationship(
            "relationship:github",
            workcell_endpoint("workcell:sandbox"),
            external_endpoint("service:github-api"),
            RequirementNecessity::Required,
            ReachabilityScope::Public,
        );
        relation.requires_policy = true;
        let route = PathFixture {
            provider_ref: provider("provider:kubernetes-network"),
            paths: vec![path(
                "provider:kubernetes-network",
                "path:public-egress",
                relation.source.clone(),
                relation.destination.clone(),
                ReachabilityScope::Public,
                FabricPathState::Reachable,
            )],
        };
        let denied = PolicyFixture {
            provider_ref: provider("provider:opensandbox-egress"),
            policies: vec![FabricPolicyOffer {
                provider_ref: provider("provider:opensandbox-egress"),
                policy_ref: "policy:default-deny".into(),
                relationship_ref: Some("relationship:github".into()),
                source: relation.source.clone(),
                destination: relation.destination.clone(),
                state: FabricPolicyState::Denied,
                provenance: BTreeMap::from([("engine".into(), "dns+nft".into())]),
            }],
        };
        let denied_plan = evaluate_fabric_with_policies(
            std::slice::from_ref(&relation),
            &[&route],
            &[&denied],
        )
        .unwrap();
        assert!(denied_plan
            .diagnostics
            .iter()
            .any(|item| item.kind == FabricDiagnosticKind::PolicyDenied));
        assert!(!denied_plan
            .diagnostics
            .iter()
            .any(|item| item.kind == FabricDiagnosticKind::DestinationUnavailable));

        let absent = evaluate_fabric_with_policies(&[relation], &[], &[&denied]).unwrap();
        assert!(absent
            .diagnostics
            .iter()
            .any(|item| item.kind == FabricDiagnosticKind::DestinationUnavailable));
    }

    #[test]
    fn external_path_and_policy_can_be_owned_by_distinct_providers() {
        let mut relation = relationship(
            "relationship:model-api",
            workcell_endpoint("workcell:sandbox"),
            external_endpoint("service:model-api"),
            RequirementNecessity::Required,
            ReachabilityScope::Public,
        );
        relation.requires_policy = true;
        let route = PathFixture {
            provider_ref: provider("provider:docker-network"),
            paths: vec![path(
                "provider:docker-network",
                "path:default-route",
                relation.source.clone(),
                relation.destination.clone(),
                ReachabilityScope::Public,
                FabricPathState::Reachable,
            )],
        };
        let allowed = PolicyFixture {
            provider_ref: provider("provider:opensandbox-egress"),
            policies: vec![FabricPolicyOffer {
                provider_ref: provider("provider:opensandbox-egress"),
                policy_ref: "policy:model-api".into(),
                relationship_ref: Some(relation.relationship.relationship_ref.clone()),
                source: relation.source.clone(),
                destination: relation.destination.clone(),
                state: FabricPolicyState::Allowed,
                provenance: BTreeMap::from([("enforcement".into(), "sidecar".into())]),
            }],
        };
        let plan = require_fabric_plan(
            evaluate_fabric_with_policies(&[relation], &[&route], &[&allowed]).unwrap(),
        )
        .unwrap();
        let binding = &plan.bindings[0];
        assert_eq!(binding.path_provider_ref.to_string(), "provider:docker-network");
        assert_eq!(
            binding.policy_provider_ref.as_ref().map(ToString::to_string),
            Some("provider:opensandbox-egress".into())
        );
        assert_eq!(binding.policy, FabricPolicyResult::Allowed);
    }

    #[test]
    fn public_path_cannot_silently_satisfy_private_reachability() {
        let relation = relationship(
            "relationship:private",
            workcell_endpoint("workcell:a"),
            workcell_endpoint("workcell:b"),
            RequirementNecessity::Required,
            ReachabilityScope::Private,
        );
        let public_only = PathFixture {
            provider_ref: provider("provider:public"),
            paths: vec![path(
                "provider:public",
                "path:public",
                relation.source.clone(),
                relation.destination.clone(),
                ReachabilityScope::Public,
                FabricPathState::Reachable,
            )],
        };
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
            workcell_endpoint("workcell:a"),
            workcell_endpoint("workcell:b"),
            RequirementNecessity::Preferred,
            ReachabilityScope::Private,
        );
        let optional = relationship(
            "relationship:optional",
            workcell_endpoint("workcell:a"),
            workcell_endpoint("workcell:c"),
            RequirementNecessity::Optional,
            ReachabilityScope::Private,
        );
        let plan = evaluate_fabric(&[preferred, optional], &[]).unwrap();
        assert_eq!(plan.status, PlanStatus::Degraded);
        assert_eq!(plan.degradations.len(), 1);
        assert_eq!(plan.omissions.len(), 1);
    }
}
