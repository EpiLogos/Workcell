use epilogos_workcell_core::{
    compose_world, ExecutionDemand, MaterialisationPlan, MaterialisedExecutionWorld,
    PlannedAllocation, PlannedRelation, ProviderPortKind, Result, WorkcellError, WorkcellRef,
};

use super::OPENSANDBOX_SOURCE_REVISION;

/// Provider-local inputs for composing an already-planned Workcell world around
/// one OpenSandbox execution allocation.
///
/// This is not a second world ontology. It validates the material parent
/// relation, annotates sibling bindings with the shared sandbox provenance, and
/// delegates world identity/BindingGraph construction to Workcell core.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct OpenSandboxMaterialComposition {
    /// Logical ref of the one Execution binding whose material_ref is the
    /// provider-native OpenSandbox sandbox ID.
    pub execution_logical_ref: String,
    /// Planned provider allocations for the complete Workcell world, including
    /// the execution allocation above and any Storage/Service/etc. siblings.
    pub allocations: Vec<PlannedAllocation>,
    /// Explicit relations among the logical material bindings.
    pub relations: Vec<PlannedRelation>,
}

impl OpenSandboxMaterialComposition {
    pub fn validate(&self) -> Result<()> {
        if self.execution_logical_ref.trim().is_empty() {
            return Err(WorkcellError::InvalidDemand(
                "OpenSandbox material composition execution logical ref must not be empty".into(),
            ));
        }
        let execution = self
            .allocations
            .iter()
            .find(|allocation| allocation.logical_ref == self.execution_logical_ref)
            .ok_or_else(|| {
                WorkcellError::InvalidDemand(format!(
                    "OpenSandbox material composition has no allocation for execution logical ref `{}`",
                    self.execution_logical_ref
                ))
            })?;
        if execution.allocation.port != ProviderPortKind::Execution {
            return Err(WorkcellError::InvalidDemand(
                "OpenSandbox material parent must be an Execution allocation".into(),
            ));
        }
        if execution.allocation.material_ref.trim().is_empty() {
            return Err(WorkcellError::InvalidDemand(
                "OpenSandbox execution material ref must not be empty".into(),
            ));
        }
        if execution
            .allocation
            .provenance
            .get("upstream.revision")
            .map(String::as_str)
            != Some(OPENSANDBOX_SOURCE_REVISION)
        {
            return Err(WorkcellError::InvalidDemand(
                "OpenSandbox material parent must retain the pinned upstream revision provenance"
                    .into(),
            ));
        }
        Ok(())
    }
}

/// Compose the canonical Workcell `MaterialisedExecutionWorld` while retaining
/// the shared physical sandbox as material provenance only.
///
/// The resulting `world_ref` is still derived by Workcell core from the full
/// set of binding identities. The OpenSandbox sandbox ID is recorded so an
/// operator can explain the material composition, but it never replaces the
/// caller-owned World/Project/Agent identities or Workcell's own world ref.
pub fn compose_opensandbox_material_world(
    workcell_ref: WorkcellRef,
    demand: &ExecutionDemand,
    plan: &MaterialisationPlan,
    composition: OpenSandboxMaterialComposition,
) -> Result<MaterialisedExecutionWorld> {
    composition.validate()?;
    let sandbox_material_ref = composition
        .allocations
        .iter()
        .find(|allocation| allocation.logical_ref == composition.execution_logical_ref)
        .expect("validated execution allocation")
        .allocation
        .material_ref
        .clone();

    let allocations = composition
        .allocations
        .into_iter()
        .map(|mut planned| {
            planned.allocation.provenance.insert(
                "opensandbox.composition_sandbox_material_ref".into(),
                sandbox_material_ref.clone(),
            );
            planned
        })
        .collect();

    let mut world = compose_world(
        workcell_ref,
        demand,
        plan,
        allocations,
        composition.relations,
    )?;
    world.provenance.insert(
        "opensandbox.execution_material_ref".into(),
        sandbox_material_ref,
    );
    world.provenance.insert(
        "opensandbox.upstream_revision".into(),
        OPENSANDBOX_SOURCE_REVISION.into(),
    );
    Ok(world)
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use epilogos_workcell_core::{
        DemandRef, ExternalRef, HealthState, MaterialisationPlan, OfferRef, PlanRef, PlanStatus,
        PlannedBinding, ProviderAllocation, ProviderRef, RequirementNecessity, StorageAccess,
        StorageRequirement, StorageSharing,
    };

    use super::*;

    fn execution_allocation(provider_ref: &ProviderRef) -> PlannedAllocation {
        PlannedAllocation {
            logical_ref: "execution:agent".into(),
            offer_ref: OfferRef::new("offer:opensandbox:execution").unwrap(),
            allocation: ProviderAllocation {
                provider_ref: provider_ref.clone(),
                port: ProviderPortKind::Execution,
                material_ref: "sbx_123".into(),
                health: HealthState::Healthy,
                properties: BTreeMap::new(),
                provenance: BTreeMap::from([
                    ("provider".into(), "opensandbox".into()),
                    (
                        "upstream.revision".into(),
                        OPENSANDBOX_SOURCE_REVISION.into(),
                    ),
                ]),
            },
        }
    }

    fn storage_allocation(provider_ref: &ProviderRef) -> PlannedAllocation {
        PlannedAllocation {
            logical_ref: "storage:project-state".into(),
            offer_ref: OfferRef::new("offer:opensandbox:storage").unwrap(),
            allocation: ProviderAllocation {
                provider_ref: provider_ref.clone(),
                port: ProviderPortKind::Storage,
                material_ref: "project-state".into(),
                health: HealthState::Healthy,
                properties: BTreeMap::new(),
                provenance: BTreeMap::from([
                    ("provider".into(), "opensandbox:pvc-storage".into()),
                    ("opensandbox.volume_backend".into(), "pvc".into()),
                ]),
            },
        }
    }

    #[test]
    fn one_sandbox_materialises_multiple_bindings_without_becoming_world_identity() {
        let provider_ref = ProviderRef::new("provider:opensandbox").unwrap();
        let demand_ref = DemandRef::new("demand:project-world").unwrap();
        let mut demand = ExecutionDemand::new(demand_ref.clone()).with_subject(
            "project",
            ExternalRef::new("project:fixture").unwrap(),
        );
        demand.storage.required.push(StorageRequirement {
            logical_ref: "storage:project-state".into(),
            access: StorageAccess::Writable,
            sharing: StorageSharing::Shared,
            minimum_capacity: None,
            unit: None,
            persistence: None,
            retention: epilogos_workcell_core::RetentionExpectation::Preserve,
        });
        demand.validate().unwrap();

        let plan = MaterialisationPlan {
            plan_ref: PlanRef::new("plan:project-world").unwrap(),
            demand_ref,
            status: PlanStatus::Satisfiable,
            planned_bindings: vec![
                PlannedBinding {
                    logical_ref: "execution:agent".into(),
                    requirement: "shell".into(),
                    necessity: RequirementNecessity::Required,
                    provider_ref: provider_ref.clone(),
                    offer_ref: OfferRef::new("offer:opensandbox:execution").unwrap(),
                },
                PlannedBinding {
                    logical_ref: "storage:project-state".into(),
                    requirement: "storage:project-state".into(),
                    necessity: RequirementNecessity::Required,
                    provider_ref: provider_ref.clone(),
                    offer_ref: OfferRef::new("offer:opensandbox:storage").unwrap(),
                },
            ],
            planned_exposures: Vec::new(),
            planned_constraints: Vec::new(),
            degradations: Vec::new(),
            omissions: Vec::new(),
            explanation: Vec::new(),
        };

        let world = compose_opensandbox_material_world(
            WorkcellRef::new("workcell:local").unwrap(),
            &demand,
            &plan,
            OpenSandboxMaterialComposition {
                execution_logical_ref: "execution:agent".into(),
                allocations: vec![
                    execution_allocation(&provider_ref),
                    storage_allocation(&provider_ref),
                ],
                relations: vec![PlannedRelation {
                    from_logical_ref: "storage:project-state".into(),
                    to_logical_ref: "execution:agent".into(),
                    relation: "mounted-for".into(),
                }],
            },
        )
        .unwrap();

        assert_eq!(world.binding_graph.bindings.len(), 2);
        assert_eq!(world.binding_graph.relations.len(), 1);
        assert_ne!(world.world_ref.as_str(), "sbx_123");
        assert_eq!(
            world.subjects.get("project").map(ToString::to_string),
            Some("project:fixture".into())
        );
        assert_eq!(
            world
                .provenance
                .get("opensandbox.execution_material_ref")
                .map(String::as_str),
            Some("sbx_123")
        );
        for binding in &world.binding_graph.bindings {
            assert_eq!(
                binding
                    .provenance
                    .get("opensandbox.composition_sandbox_material_ref")
                    .map(String::as_str),
                Some("sbx_123")
            );
        }
    }
}
