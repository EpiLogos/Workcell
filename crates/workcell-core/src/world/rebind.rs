use super::fingerprint::{binding_ref, world_ref};
use crate::{
    BindingPresence, HealthState, MaterialisedExecutionWorld, ProviderAllocation, Result,
    WorkcellError,
};
use std::collections::BTreeMap;

/// Rebind recovered material without assigning or rewriting any caller subject.
/// This is same-provider recovery. Provider/Workcell substitution is a new
/// prepare with the caller's retained subjects and explicit prior-world link.
pub fn rebind_material_world(
    previous: &MaterialisedExecutionWorld,
    replacements: &BTreeMap<String, ProviderAllocation>,
) -> Result<MaterialisedExecutionWorld> {
    let mut world = previous.clone();
    let mut mapping = BTreeMap::new();
    for binding in &mut world.binding_graph.bindings {
        let old_ref = binding.binding_ref.clone();
        if let Some(allocation) = replacements.get(&binding.logical_ref) {
            if allocation.provider_ref != binding.provider_ref || allocation.port != binding.port {
                return Err(WorkcellError::InvalidDemand("provider substitution requires an explicit new preparation, not same-provider recovery".into()));
            }
            let old_material = binding.material_ref.clone();
            binding.material_ref = allocation.material_ref.clone();
            binding.properties = allocation.properties.clone();
            binding.provenance = allocation.provenance.clone();
            binding.health = allocation.health.clone();
            binding.presence = BindingPresence::Present;
            binding.binding_ref = binding_ref(
                &world.workcell_ref,
                &world.demand_ref,
                &binding.logical_ref,
                &binding.provider_ref,
                &binding.material_ref,
            )?;
            if old_material != binding.material_ref {
                binding
                    .provenance
                    .insert("recovered_from_material_ref".into(), old_material);
            }
        }
        mapping.insert(old_ref, binding.binding_ref.clone());
    }
    if replacements.keys().any(|k| {
        !world
            .binding_graph
            .bindings
            .iter()
            .any(|b| &b.logical_ref == k)
    }) {
        return Err(WorkcellError::InvalidDemand(
            "recovery refers to an unknown logical binding".into(),
        ));
    }
    for relation in &mut world.binding_graph.relations {
        relation.from = mapping
            .get(&relation.from)
            .cloned()
            .ok_or_else(|| WorkcellError::InvalidDemand("invalid old binding relation".into()))?;
        relation.to = mapping
            .get(&relation.to)
            .cloned()
            .ok_or_else(|| WorkcellError::InvalidDemand("invalid old binding relation".into()))?;
    }
    world.world_ref = world_ref(
        &world.workcell_ref,
        &world.demand_ref,
        world
            .binding_graph
            .bindings
            .iter()
            .map(|b| b.binding_ref.to_string()),
    )?;
    world.state = if world
        .binding_graph
        .bindings
        .iter()
        .any(|b| b.health == HealthState::Unavailable)
    {
        HealthState::Unavailable
    } else if world
        .binding_graph
        .bindings
        .iter()
        .all(|b| b.health == HealthState::Healthy)
    {
        HealthState::Healthy
    } else {
        HealthState::Degraded
    };
    let changed = world.world_ref != previous.world_ref;
    world.provenance.insert(
        "continuity".into(),
        if changed {
            "rematerialised-not-semantic-session-resume"
        } else {
            "reobserved-same-material-binding"
        }
        .into(),
    );
    if changed {
        world
            .provenance
            .insert("previous_world_ref".into(), previous.world_ref.to_string());
    }
    Ok(world)
}
