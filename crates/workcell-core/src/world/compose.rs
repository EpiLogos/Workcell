use std::collections::{BTreeMap, BTreeSet};

use crate::{
    Binding, BindingGraph, BindingPresence, BindingRelation, ExecutionDemand, HealthState,
    MaterialisationPlan, MaterialisedExecutionWorld, OfferRef, PlanStatus, ProviderAllocation,
    Result, WorkcellError, WorkcellRef,
};

use super::fingerprint::{binding_ref, world_ref};

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PlannedAllocation {
    pub logical_ref: String,
    pub offer_ref: OfferRef,
    pub allocation: ProviderAllocation,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PlannedRelation {
    pub from_logical_ref: String,
    pub to_logical_ref: String,
    pub relation: String,
}

pub fn compose_world(
    workcell_ref: WorkcellRef,
    demand: &ExecutionDemand,
    plan: &MaterialisationPlan,
    allocations: Vec<PlannedAllocation>,
    relations: Vec<PlannedRelation>,
) -> Result<MaterialisedExecutionWorld> {
    demand.validate()?;
    if plan.demand_ref != demand.demand_ref {
        return Err(WorkcellError::OperationFailed(
            "materialisation plan belongs to a different demand".into(),
        ));
    }
    if plan.status == PlanStatus::Unsatisfiable {
        return Err(WorkcellError::UnsatisfiedDemand(
            "cannot compose a world from an unsatisfiable plan".into(),
        ));
    }

    let planned: BTreeMap<&str, _> = plan
        .planned_bindings
        .iter()
        .map(|binding| (binding.logical_ref.as_str(), binding))
        .collect();
    if planned.len() != plan.planned_bindings.len() {
        return Err(WorkcellError::OperationFailed(
            "materialisation plan contains duplicate logical bindings".into(),
        ));
    }

    let mut seen = BTreeSet::new();
    let mut bindings = Vec::with_capacity(allocations.len());
    for item in allocations {
        if !seen.insert(item.logical_ref.clone()) {
            return Err(WorkcellError::OperationFailed(format!(
                "duplicate provider allocation for logical ref `{}`",
                item.logical_ref
            )));
        }
        let expected = planned.get(item.logical_ref.as_str()).ok_or_else(|| {
            WorkcellError::OperationFailed(format!(
                "provider allocation for unplanned logical ref `{}`",
                item.logical_ref
            ))
        })?;
        if expected.provider_ref != item.allocation.provider_ref {
            return Err(WorkcellError::OperationFailed(format!(
                "allocation provider `{}` does not match planned provider `{}` for `{}`",
                item.allocation.provider_ref, expected.provider_ref, item.logical_ref
            )));
        }
        if expected.offer_ref != item.offer_ref {
            return Err(WorkcellError::OperationFailed(format!(
                "allocation offer `{}` does not match planned offer `{}` for `{}`",
                item.offer_ref, expected.offer_ref, item.logical_ref
            )));
        }
        if item.allocation.material_ref.trim().is_empty() {
            return Err(WorkcellError::OperationFailed(format!(
                "allocation for `{}` has an empty material ref",
                item.logical_ref
            )));
        }

        let binding_ref = binding_ref(
            &workcell_ref,
            &demand.demand_ref,
            &item.logical_ref,
            &item.allocation.provider_ref,
            &item.allocation.material_ref,
        )?;
        bindings.push(Binding {
            binding_ref,
            logical_ref: item.logical_ref,
            necessity: expected.necessity,
            provider_ref: item.allocation.provider_ref,
            offer_ref: item.offer_ref,
            port: item.allocation.port,
            material_ref: item.allocation.material_ref,
            health: item.allocation.health,
            presence: BindingPresence::Present,
            properties: item.allocation.properties,
            provenance: item.allocation.provenance,
        });
    }

    for expected in &plan.planned_bindings {
        if !seen.contains(&expected.logical_ref) {
            return Err(WorkcellError::OperationFailed(format!(
                "planned logical binding `{}` has no provider allocation",
                expected.logical_ref
            )));
        }
    }

    bindings.sort_by(|left, right| left.logical_ref.cmp(&right.logical_ref));
    let by_logical: BTreeMap<&str, _> = bindings
        .iter()
        .map(|binding| (binding.logical_ref.as_str(), &binding.binding_ref))
        .collect();
    let mut graph_relations = Vec::with_capacity(relations.len());
    let mut relation_keys = BTreeSet::new();
    for relation in relations {
        if relation.relation.trim().is_empty() {
            return Err(WorkcellError::InvalidDemand(
                "binding relation name must not be empty".into(),
            ));
        }
        let from = by_logical
            .get(relation.from_logical_ref.as_str())
            .ok_or_else(|| {
                WorkcellError::OperationFailed(format!(
                    "binding relation source `{}` is not materialised",
                    relation.from_logical_ref
                ))
            })?;
        let to = by_logical
            .get(relation.to_logical_ref.as_str())
            .ok_or_else(|| {
                WorkcellError::OperationFailed(format!(
                    "binding relation target `{}` is not materialised",
                    relation.to_logical_ref
                ))
            })?;
        let key = (
            from.as_str().to_owned(),
            to.as_str().to_owned(),
            relation.relation.clone(),
        );
        if !relation_keys.insert(key) {
            return Err(WorkcellError::OperationFailed(
                "duplicate binding relation".into(),
            ));
        }
        graph_relations.push(BindingRelation {
            from: (*from).clone(),
            to: (*to).clone(),
            relation: relation.relation,
        });
    }

    let state = aggregate_health(bindings.iter().map(|binding| &binding.health));
    let world_ref = world_ref(
        &workcell_ref,
        &demand.demand_ref,
        bindings
            .iter()
            .map(|binding| binding.binding_ref.to_string()),
    )?;
    let mut provenance = BTreeMap::new();
    provenance.insert("plan_ref".into(), plan.plan_ref.to_string());
    provenance.insert("binding_count".into(), bindings.len().to_string());
    provenance.insert(
        "planned_exposure_count".into(),
        plan.planned_exposures.len().to_string(),
    );

    Ok(MaterialisedExecutionWorld {
        world_ref,
        workcell_ref,
        demand_ref: demand.demand_ref.clone(),
        subjects: demand.subjects.clone(),
        binding_graph: BindingGraph {
            bindings,
            relations: graph_relations,
        },
        planned_exposures: plan.planned_exposures.clone(),
        retention: demand.retention.clone(),
        state,
        provenance,
    })
}

fn aggregate_health<'a>(states: impl Iterator<Item = &'a HealthState>) -> HealthState {
    let mut degraded = false;
    for state in states {
        match state {
            HealthState::Unavailable => return HealthState::Unavailable,
            HealthState::Degraded | HealthState::Unknown => degraded = true,
            HealthState::Healthy => {}
        }
    }
    if degraded {
        HealthState::Degraded
    } else {
        HealthState::Healthy
    }
}
