use super::{
    fingerprint::make_plan_ref, policy::NeutralPlanningPolicy, policy::PlanningPolicy,
    requirements::atoms, resolve::resolve,
};
use crate::{
    Availability, Degradation, Discovery, ExecutionDemand, HealthState, MaterialisationPlan,
    PersistenceScope, PlanOmission, PlanStatus, PlannedBinding, PlannedConstraint, PlannedExposure,
    RequirementNecessity, Result, RetentionExpectation,
};

pub fn plan(demand: &ExecutionDemand, discovery: &Discovery) -> Result<MaterialisationPlan> {
    plan_with_policy(demand, discovery, &NeutralPlanningPolicy)
}

pub fn plan_with_policy(
    demand: &ExecutionDemand,
    discovery: &Discovery,
    policy: &dyn PlanningPolicy,
) -> Result<MaterialisationPlan> {
    demand.validate()?;
    let mut bindings = Vec::new();
    let mut exposures = Vec::new();
    let mut constraints = Vec::new();
    let mut degradations = Vec::new();
    let mut omissions = Vec::new();
    let mut explanation = Vec::new();
    let mut missing_required = false;
    for requirement in atoms(demand) {
        let resolution = resolve(demand, &discovery.offers, policy, &requirement);
        if let Some(selected) = resolution.selected {
            let logical_ref = format!("{}:{}", requirement.kind, requirement.key);
            explanation.push(format!(
                "{} matched offer {} from provider {}",
                logical_ref, selected.offer_ref, selected.provider_ref
            ));
            if selected.availability == Availability::Degraded
                || matches!(
                    selected.health,
                    HealthState::Degraded | HealthState::Unknown
                )
            {
                degradations.push(Degradation {
                    requirement: logical_ref.clone(),
                    necessity: requirement.necessity,
                    reason: "selected offer is degraded or health is not fully known".into(),
                });
            }
            if requirement.kind == "exposure" {
                exposures.push(PlannedExposure {
                    logical_ref,
                    requirement: requirement.key,
                    necessity: requirement.necessity,
                    provider_ref: selected.provider_ref.clone(),
                    offer_ref: selected.offer_ref.clone(),
                });
            } else if is_constraint_kind(requirement.kind) {
                constraints.push(PlannedConstraint {
                    logical_ref,
                    requirement: requirement.key,
                    necessity: requirement.necessity,
                    provider_ref: selected.provider_ref.clone(),
                    offer_ref: selected.offer_ref.clone(),
                });
            } else {
                bindings.push(PlannedBinding {
                    logical_ref,
                    requirement: requirement.key,
                    necessity: requirement.necessity,
                    provider_ref: selected.provider_ref.clone(),
                    offer_ref: selected.offer_ref.clone(),
                });
            }
            continue;
        }
        let label = format!("{}:{}", requirement.kind, requirement.key);
        match requirement.necessity {
            RequirementNecessity::Required => {
                missing_required = true;
                omissions.push(PlanOmission {
                    requirement: label,
                    necessity: RequirementNecessity::Required,
                    reason: resolution.reason,
                });
            }
            RequirementNecessity::Preferred => degradations.push(Degradation {
                requirement: label,
                necessity: RequirementNecessity::Preferred,
                reason: resolution.reason,
            }),
            RequirementNecessity::Optional => omissions.push(PlanOmission {
                requirement: label,
                necessity: RequirementNecessity::Optional,
                reason: resolution.reason,
            }),
        }
    }
    if let Some(advisory) = hosted_services_vm_advisory(demand, &bindings, &exposures, &constraints)
    {
        degradations.push(advisory);
    }
    let status = if missing_required {
        PlanStatus::Unsatisfiable
    } else if degradations.is_empty() {
        PlanStatus::Satisfiable
    } else {
        PlanStatus::Degraded
    };
    let plan_ref = make_plan_ref(
        demand,
        &bindings,
        &exposures,
        &constraints,
        &degradations,
        &omissions,
    )?;
    Ok(MaterialisationPlan {
        plan_ref,
        demand_ref: demand.demand_ref.clone(),
        status,
        planned_bindings: bindings,
        planned_exposures: exposures,
        planned_constraints: constraints,
        degradations,
        omissions,
        explanation,
    })
}

const COLLAPSED_LOCAL_HOST_PROCESS_PROVIDER: &str = "provider:collapsed-local-host-process";
const HOSTED_SERVICES_VM_ADVISORY_REQUIREMENT: &str = "execution:hosting-advisory";

/// The hosted-services-VM consideration rides the existing degradation
/// vocabulary: it is an advisory inside `degradations`, not a new plan
/// concept. The law stays in `docs/DEPLOYMENT-PROFILES.md`; the plan only
/// points at it.
fn hosted_services_vm_advisory(
    demand: &ExecutionDemand,
    bindings: &[PlannedBinding],
    exposures: &[PlannedExposure],
    constraints: &[PlannedConstraint],
) -> Option<Degradation> {
    if !indicates_long_lived_placement(demand) {
        return None;
    }
    if !landed_on_host_process(bindings, exposures, constraints) {
        return None;
    }
    Some(Degradation {
        requirement: HOSTED_SERVICES_VM_ADVISORY_REQUIREMENT.into(),
        necessity: RequirementNecessity::Preferred,
        reason: "long-lived placement materialised on the collapsed-local host-process provider; this execution is a candidate for hosting inside a services VM - see docs/DEPLOYMENT-PROFILES.md, section 'Profile: hosted services VM (desktop or workstation host)'".into(),
    })
}

/// Conservative long-lived trigger: only the explicit durable signals count.
/// The durable persistence scopes (Project, Workcell, Factory, External) and
/// `RetentionExpectation::Preserve` indicate a long-lived placement; the
/// Ephemeral, TaskOrRun and Candidate scopes and the other retention
/// expectations do not.
fn indicates_long_lived_placement(demand: &ExecutionDemand) -> bool {
    let durable_scope = matches!(
        demand.persistence,
        Some(
            PersistenceScope::Project
                | PersistenceScope::Workcell
                | PersistenceScope::Factory
                | PersistenceScope::External
        )
    );
    durable_scope || demand.retention == RetentionExpectation::Preserve
}

fn landed_on_host_process(
    bindings: &[PlannedBinding],
    exposures: &[PlannedExposure],
    constraints: &[PlannedConstraint],
) -> bool {
    bindings
        .iter()
        .any(|binding| binding.provider_ref.as_str() == COLLAPSED_LOCAL_HOST_PROCESS_PROVIDER)
        || exposures
            .iter()
            .any(|exposure| exposure.provider_ref.as_str() == COLLAPSED_LOCAL_HOST_PROCESS_PROVIDER)
        || constraints.iter().any(|constraint| {
            constraint.provider_ref.as_str() == COLLAPSED_LOCAL_HOST_PROCESS_PROVIDER
        })
}

fn is_constraint_kind(kind: &str) -> bool {
    matches!(
        kind,
        "resource" | "persistence" | "isolation-trust" | "retention"
    )
}
