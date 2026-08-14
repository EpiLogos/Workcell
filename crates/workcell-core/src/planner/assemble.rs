use super::{
    fingerprint::make_plan_ref, policy::NeutralPlanningPolicy, policy::PlanningPolicy,
    requirements::atoms, resolve::resolve,
};
use crate::{
    Availability, Degradation, Discovery, ExecutionDemand, HealthState, MaterialisationPlan,
    PlanOmission, PlanStatus, PlannedBinding, PlannedExposure, RequirementNecessity, Result,
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
    let status = if missing_required {
        PlanStatus::Unsatisfiable
    } else if degradations.is_empty() {
        PlanStatus::Satisfiable
    } else {
        PlanStatus::Degraded
    };
    let plan_ref = make_plan_ref(demand, &bindings, &exposures, &degradations, &omissions)?;
    Ok(MaterialisationPlan {
        plan_ref,
        demand_ref: demand.demand_ref.clone(),
        status,
        planned_bindings: bindings,
        planned_exposures: exposures,
        degradations,
        omissions,
        explanation,
    })
}
