use std::collections::BTreeMap;

use crate::{
    plan, ArtifactStorageProvider, Binding, BindingPresence, CollectionBundle, CollectedOutput,
    Degradation, DesiredMaterialState, Discovery, ExecutionDemand, Exposure, ExposureBundle,
    ExposureRequirement, HealthState, MaterialExposureProvider, MaterialObservation,
    MaterialisationPlan, MaterialisedExecutionWorld, ObservationBundle, PlanOmission,
    ProjectRuntimeProvider, ProviderAllocation, ProviderExposureRequest, ProviderPort,
    ProviderPortKind, ReconciliationResult, ReleaseDisposition, ReleaseResult,
    RequirementNecessity, Result, RetentionExpectation, WorkcellControlPlane, WorkcellError,
    WorkcellRef, WorldRef,
};

pub trait RuntimeExposureProvider: ProjectRuntimeProvider + MaterialExposureProvider {}

impl<T> RuntimeExposureProvider for T where T: ProjectRuntimeProvider + MaterialExposureProvider {}

/// Operational control-plane surface for material worlds that have already
/// been prepared and composed. Generic preparation/reconciliation remains a
/// later orchestration concern; expose/collect/release are real here.
pub struct PreparedWorldControlPlane {
    workcell_ref: WorkcellRef,
    worlds: BTreeMap<String, MaterialisedExecutionWorld>,
    runtime_providers: BTreeMap<String, Box<dyn RuntimeExposureProvider>>,
    artifact_providers: BTreeMap<String, Box<dyn ArtifactStorageProvider>>,
}

impl PreparedWorldControlPlane {
    pub fn new(workcell_ref: WorkcellRef) -> Self {
        Self {
            workcell_ref,
            worlds: BTreeMap::new(),
            runtime_providers: BTreeMap::new(),
            artifact_providers: BTreeMap::new(),
        }
    }

    pub fn register_world(&mut self, world: MaterialisedExecutionWorld) -> Result<()> {
        if world.workcell_ref != self.workcell_ref {
            return Err(WorkcellError::OperationFailed(format!(
                "world `{}` belongs to workcell `{}`, not `{}`",
                world.world_ref, world.workcell_ref, self.workcell_ref
            )));
        }
        self.worlds.insert(world.world_ref.to_string(), world);
        Ok(())
    }

    pub fn register_runtime_provider<P>(&mut self, provider: P) -> Result<()>
    where
        P: RuntimeExposureProvider + 'static,
    {
        let key = provider.provider_ref().to_string();
        if self.runtime_providers.contains_key(&key) {
            return Err(WorkcellError::OperationFailed(format!(
                "runtime provider `{key}` is already registered"
            )));
        }
        self.runtime_providers.insert(key, Box::new(provider));
        Ok(())
    }

    pub fn register_artifact_provider<P>(&mut self, provider: P) -> Result<()>
    where
        P: ArtifactStorageProvider + 'static,
    {
        let key = provider.provider_ref().to_string();
        if self.artifact_providers.contains_key(&key) {
            return Err(WorkcellError::OperationFailed(format!(
                "artifact provider `{key}` is already registered"
            )));
        }
        self.artifact_providers.insert(key, Box::new(provider));
        Ok(())
    }

    pub fn world(&self, world_ref: &WorldRef) -> Option<&MaterialisedExecutionWorld> {
        self.worlds.get(world_ref.as_str())
    }

    fn require_world(&self, world_ref: &WorldRef) -> Result<&MaterialisedExecutionWorld> {
        self.world(world_ref).ok_or_else(|| {
            WorkcellError::NotFound(format!("material world `{world_ref}` is not registered"))
        })
    }
}

impl WorkcellControlPlane for PreparedWorldControlPlane {
    fn discover(&self) -> Result<Discovery> {
        let mut offers = Vec::new();
        for provider in self.runtime_providers.values() {
            offers.extend(provider.offers()?);
        }
        for provider in self.artifact_providers.values() {
            offers.extend(provider.offers()?);
        }
        Ok(Discovery {
            workcell_ref: self.workcell_ref.clone(),
            health: HealthState::Healthy,
            offers,
        })
    }

    fn plan(&self, demand: &ExecutionDemand) -> Result<MaterialisationPlan> {
        plan(demand, &self.discover()?)
    }

    fn prepare(&mut self, _: &ExecutionDemand) -> Result<MaterialisedExecutionWorld> {
        Err(WorkcellError::Unsupported(
            "PreparedWorldControlPlane operates on externally prepared worlds".into(),
        ))
    }

    fn observe(&self, world_ref: &WorldRef) -> Result<ObservationBundle> {
        let world = self.require_world(world_ref)?;
        let observations = world
            .binding_graph
            .bindings
            .iter()
            .map(|binding| MaterialObservation {
                logical_ref: binding.logical_ref.clone(),
                state: binding.health.clone(),
                detail: binding.properties.clone(),
            })
            .collect();
        Ok(ObservationBundle {
            world_ref: world.world_ref.clone(),
            observations,
        })
    }

    fn expose(&self, world_ref: &WorldRef) -> Result<ExposureBundle> {
        let world = self.require_world(world_ref)?;
        let mut surfaces = Vec::new();
        let mut degradations = Vec::new();
        let mut omissions = Vec::new();

        for planned in &world.planned_exposures {
            let result = (|| {
                let provider = self
                    .runtime_providers
                    .get(planned.provider_ref.as_str())
                    .ok_or_else(|| {
                        WorkcellError::Unavailable(format!(
                            "exposure provider `{}` is not registered",
                            planned.provider_ref
                        ))
                    })?;
                let binding = world
                    .binding_graph
                    .bindings
                    .iter()
                    .find(|binding| {
                        binding.provider_ref == planned.provider_ref
                            && binding.offer_ref == planned.offer_ref
                            && binding.presence == BindingPresence::Present
                    })
                    .ok_or_else(|| {
                        WorkcellError::Unavailable(format!(
                            "no present material binding exists for exposure `{}`",
                            planned.logical_ref
                        ))
                    })?;
                let allocation = allocation_from_binding(binding);
                let request = ProviderExposureRequest {
                    demand_ref: world.demand_ref.clone(),
                    requirement: ExposureRequirement::new(planned.requirement.clone())?,
                    necessity: planned.necessity,
                };
                let exposed = provider.expose_material(&allocation, &request)?;
                if exposed.provider_ref != binding.provider_ref
                    || exposed.material_ref != binding.material_ref
                {
                    return Err(WorkcellError::OperationFailed(format!(
                        "exposure provider returned material outside binding `{}`",
                        binding.logical_ref
                    )));
                }
                Ok(Exposure {
                    logical_ref: planned.logical_ref.clone(),
                    interaction: exposed.interaction,
                    material: exposed.material,
                    provenance: exposed.provenance,
                })
            })();

            match result {
                Ok(surface) => surfaces.push(surface),
                Err(error) => match planned.necessity {
                    RequirementNecessity::Required => return Err(error),
                    RequirementNecessity::Preferred => degradations.push(Degradation {
                        requirement: planned.logical_ref.clone(),
                        necessity: planned.necessity,
                        reason: error.to_string(),
                    }),
                    RequirementNecessity::Optional => omissions.push(PlanOmission {
                        requirement: planned.logical_ref.clone(),
                        necessity: planned.necessity,
                        reason: error.to_string(),
                    }),
                },
            }
        }

        Ok(ExposureBundle {
            world_ref: world.world_ref.clone(),
            surfaces,
            degradations,
            omissions,
        })
    }

    fn collect(&self, world_ref: &WorldRef) -> Result<CollectionBundle> {
        let world = self.require_world(world_ref)?;
        let mut outputs = Vec::new();
        let mut degradations = Vec::new();
        let mut omissions = Vec::new();

        for binding in world
            .binding_graph
            .bindings
            .iter()
            .filter(|binding| binding.port == ProviderPortKind::ArtifactStorage)
        {
            let result = (|| {
                if binding.presence != BindingPresence::Present {
                    return Err(WorkcellError::Unavailable(format!(
                        "artifact binding `{}` is not present",
                        binding.logical_ref
                    )));
                }
                let provider = self
                    .artifact_providers
                    .get(binding.provider_ref.as_str())
                    .ok_or_else(|| {
                        WorkcellError::Unavailable(format!(
                            "artifact provider `{}` is not registered",
                            binding.provider_ref
                        ))
                    })?;
                let allocation = allocation_from_binding(binding);
                let collected = provider.collect_material(&allocation)?;
                let mut converted = Vec::with_capacity(collected.len());
                for material in collected {
                    if material.provider_ref != binding.provider_ref
                        || material.material_ref != binding.material_ref
                    {
                        return Err(WorkcellError::OperationFailed(format!(
                            "artifact provider returned material outside binding `{}`",
                            binding.logical_ref
                        )));
                    }
                    converted.push(CollectedOutput {
                        logical_ref: material.logical_output,
                        material_locator: material.locator,
                        provenance: material.provenance,
                    });
                }
                Ok(converted)
            })();

            match result {
                Ok(mut collected) => outputs.append(&mut collected),
                Err(error) => match binding.necessity {
                    RequirementNecessity::Required => return Err(error),
                    RequirementNecessity::Preferred => degradations.push(Degradation {
                        requirement: binding.logical_ref.clone(),
                        necessity: binding.necessity,
                        reason: error.to_string(),
                    }),
                    RequirementNecessity::Optional => omissions.push(PlanOmission {
                        requirement: binding.logical_ref.clone(),
                        necessity: binding.necessity,
                        reason: error.to_string(),
                    }),
                },
            }
        }

        Ok(CollectionBundle {
            world_ref: world.world_ref.clone(),
            outputs,
            degradations,
            omissions,
        })
    }

    fn release(&mut self, world_ref: &WorldRef) -> Result<ReleaseResult> {
        let snapshot = self.require_world(world_ref)?.clone();
        let retention = snapshot.retention.clone();
        let mut changed = false;
        let mut released_refs = Vec::new();

        for binding in &snapshot.binding_graph.bindings {
            if binding.presence != BindingPresence::Present {
                continue;
            }
            let allocation = allocation_from_binding(binding);
            let result = match binding.port {
                ProviderPortKind::ProjectRuntime => {
                    let provider = self
                        .runtime_providers
                        .get_mut(binding.provider_ref.as_str())
                        .ok_or_else(|| {
                            WorkcellError::Unavailable(format!(
                                "runtime provider `{}` is not registered",
                                binding.provider_ref
                            ))
                        })?;
                    provider.release_runtime(&allocation, &retention)?
                }
                ProviderPortKind::ArtifactStorage => {
                    let provider = self
                        .artifact_providers
                        .get_mut(binding.provider_ref.as_str())
                        .ok_or_else(|| {
                            WorkcellError::Unavailable(format!(
                                "artifact provider `{}` is not registered",
                                binding.provider_ref
                            ))
                        })?;
                    provider.release_artifact_channel(&allocation, &retention)?
                }
                _ if retention == RetentionExpectation::Preserve => continue,
                _ => {
                    return Err(WorkcellError::Unsupported(format!(
                        "PreparedWorldControlPlane cannot release provider family `{}`",
                        binding.port.as_str()
                    )))
                }
            };
            changed |= result.changed;
            if result.disposition == ReleaseDisposition::Released {
                released_refs.push(binding.binding_ref.clone());
            }
        }

        if !released_refs.is_empty() {
            let world = self.worlds.get_mut(world_ref.as_str()).ok_or_else(|| {
                WorkcellError::NotFound(format!("material world `{world_ref}` is not registered"))
            })?;
            for binding in &mut world.binding_graph.bindings {
                if released_refs.contains(&binding.binding_ref) {
                    binding.presence = BindingPresence::Released;
                    binding.health = HealthState::Unavailable;
                }
            }
            if world
                .binding_graph
                .bindings
                .iter()
                .all(|binding| binding.presence != BindingPresence::Present)
            {
                world.state = HealthState::Unavailable;
            }
        }

        Ok(ReleaseResult {
            world_ref: snapshot.world_ref,
            disposition: disposition_for(&retention),
            changed,
        })
    }

    fn reconcile(&mut self, _: &[DesiredMaterialState]) -> Result<ReconciliationResult> {
        Err(WorkcellError::Unsupported(
            "reconciliation is owned by the later lifecycle span".into(),
        ))
    }
}

fn allocation_from_binding(binding: &Binding) -> ProviderAllocation {
    ProviderAllocation {
        provider_ref: binding.provider_ref.clone(),
        port: binding.port,
        material_ref: binding.material_ref.clone(),
        health: binding.health.clone(),
        properties: binding.properties.clone(),
        provenance: binding.provenance.clone(),
    }
}

fn disposition_for(retention: &RetentionExpectation) -> ReleaseDisposition {
    match retention {
        RetentionExpectation::Release => ReleaseDisposition::Released,
        RetentionExpectation::Preserve => ReleaseDisposition::Preserved,
        RetentionExpectation::SuspendIfSupported => ReleaseDisposition::Suspended,
        RetentionExpectation::SnapshotIfSupported => ReleaseDisposition::Snapshotted,
    }
}
