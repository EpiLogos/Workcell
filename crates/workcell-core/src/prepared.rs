use std::collections::{BTreeMap, BTreeSet};

use crate::{
    plan, ArtifactStorageProvider, Binding, BindingPresence, CollectedOutput, CollectionBundle,
    Degradation, DesiredMaterialState, Discovery, ExecutionDemand, ExecutionProvider, Exposure,
    ExposureBundle, ExposureRequirement, HealthState, MaterialExposureProvider,
    MaterialObservation, MaterialisationPlan, MaterialisedExecutionWorld, ObservationBundle,
    OperationalOffer, PersistenceScope, PlanOmission, ProjectRuntimeProvider, ProviderAllocation,
    ProviderCollectedMaterial, ProviderExposedSurface, ProviderExposureRequest,
    ProviderObservation, ProviderPort, ProviderPortKind, ProviderRef, ProviderReleaseResult,
    ReconciliationDelta, ReconciliationResult, ReleaseDisposition, ReleaseResult,
    RequirementNecessity, Result, RetentionExpectation, ServiceProvider, WorkcellControlPlane,
    WorkcellError, WorkcellRef, WorkspaceProvider, WorldRef,
};

pub trait RuntimeExposureProvider: ProjectRuntimeProvider + MaterialExposureProvider {}

impl<T> RuntimeExposureProvider for T where T: ProjectRuntimeProvider + MaterialExposureProvider {}

trait ErasedWorkspaceProvider {
    fn offers(&self) -> Result<Vec<OperationalOffer>>;
    fn observe(&self, allocation: &ProviderAllocation) -> Result<ProviderObservation>;
    fn release(
        &mut self,
        allocation: &ProviderAllocation,
        retention: &RetentionExpectation,
    ) -> Result<ProviderReleaseResult>;
}

impl<P> ErasedWorkspaceProvider for P
where
    P: WorkspaceProvider,
{
    fn offers(&self) -> Result<Vec<OperationalOffer>> {
        ProviderPort::offers(self)
    }

    fn observe(&self, allocation: &ProviderAllocation) -> Result<ProviderObservation> {
        WorkspaceProvider::observe_workspace(self, allocation)
    }

    fn release(
        &mut self,
        allocation: &ProviderAllocation,
        retention: &RetentionExpectation,
    ) -> Result<ProviderReleaseResult> {
        WorkspaceProvider::release_workspace(self, allocation, retention)
    }
}

trait ErasedExecutionProvider {
    fn offers(&self) -> Result<Vec<OperationalOffer>>;
    fn observe(&self, allocation: &ProviderAllocation) -> Result<ProviderObservation>;
    fn release(
        &mut self,
        allocation: &ProviderAllocation,
        retention: &RetentionExpectation,
    ) -> Result<ProviderReleaseResult>;
}

impl<P> ErasedExecutionProvider for P
where
    P: ExecutionProvider,
{
    fn offers(&self) -> Result<Vec<OperationalOffer>> {
        ProviderPort::offers(self)
    }

    fn observe(&self, allocation: &ProviderAllocation) -> Result<ProviderObservation> {
        ExecutionProvider::observe_execution(self, allocation)
    }

    fn release(
        &mut self,
        allocation: &ProviderAllocation,
        retention: &RetentionExpectation,
    ) -> Result<ProviderReleaseResult> {
        ExecutionProvider::release_execution(self, allocation, retention)
    }
}

trait ErasedRuntimeProvider {
    fn offers(&self) -> Result<Vec<OperationalOffer>>;
    fn observe(&self, allocation: &ProviderAllocation) -> Result<ProviderObservation>;
    fn expose_material(
        &self,
        allocation: &ProviderAllocation,
        request: &ProviderExposureRequest,
    ) -> Result<ProviderExposedSurface>;
    fn release(
        &mut self,
        allocation: &ProviderAllocation,
        retention: &RetentionExpectation,
    ) -> Result<ProviderReleaseResult>;
}

impl<P> ErasedRuntimeProvider for P
where
    P: RuntimeExposureProvider,
{
    fn offers(&self) -> Result<Vec<OperationalOffer>> {
        ProviderPort::offers(self)
    }

    fn observe(&self, allocation: &ProviderAllocation) -> Result<ProviderObservation> {
        ProjectRuntimeProvider::observe_runtime(self, allocation)
    }

    fn expose_material(
        &self,
        allocation: &ProviderAllocation,
        request: &ProviderExposureRequest,
    ) -> Result<ProviderExposedSurface> {
        MaterialExposureProvider::expose_material(self, allocation, request)
    }

    fn release(
        &mut self,
        allocation: &ProviderAllocation,
        retention: &RetentionExpectation,
    ) -> Result<ProviderReleaseResult> {
        ProjectRuntimeProvider::release_runtime(self, allocation, retention)
    }
}

trait ErasedServiceProvider {
    fn offers(&self) -> Result<Vec<OperationalOffer>>;
    fn observe(&self, allocation: &ProviderAllocation) -> Result<ProviderObservation>;
    fn release(
        &mut self,
        allocation: &ProviderAllocation,
        retention: &RetentionExpectation,
    ) -> Result<ProviderReleaseResult>;
}

impl<P> ErasedServiceProvider for P
where
    P: ServiceProvider,
{
    fn offers(&self) -> Result<Vec<OperationalOffer>> {
        ProviderPort::offers(self)
    }

    fn observe(&self, allocation: &ProviderAllocation) -> Result<ProviderObservation> {
        ServiceProvider::observe_service(self, allocation)
    }

    fn release(
        &mut self,
        allocation: &ProviderAllocation,
        retention: &RetentionExpectation,
    ) -> Result<ProviderReleaseResult> {
        ServiceProvider::release_service(self, allocation, retention)
    }
}

trait ErasedArtifactProvider {
    fn offers(&self) -> Result<Vec<OperationalOffer>>;
    fn collect_material(
        &self,
        allocation: &ProviderAllocation,
    ) -> Result<Vec<ProviderCollectedMaterial>>;
    fn observe(&self, allocation: &ProviderAllocation) -> Result<ProviderObservation>;
    fn release(
        &mut self,
        allocation: &ProviderAllocation,
        retention: &RetentionExpectation,
    ) -> Result<ProviderReleaseResult>;
}

impl<P> ErasedArtifactProvider for P
where
    P: ArtifactStorageProvider,
{
    fn offers(&self) -> Result<Vec<OperationalOffer>> {
        ProviderPort::offers(self)
    }

    fn collect_material(
        &self,
        allocation: &ProviderAllocation,
    ) -> Result<Vec<ProviderCollectedMaterial>> {
        ArtifactStorageProvider::collect_material(self, allocation)
    }

    fn observe(&self, allocation: &ProviderAllocation) -> Result<ProviderObservation> {
        ArtifactStorageProvider::observe_artifact_channel(self, allocation)
    }

    fn release(
        &mut self,
        allocation: &ProviderAllocation,
        retention: &RetentionExpectation,
    ) -> Result<ProviderReleaseResult> {
        ArtifactStorageProvider::release_artifact_channel(self, allocation, retention)
    }
}

/// Operational control-plane surface for material worlds that have already
/// been prepared and composed.
///
/// World records can be re-registered after a process restart. Providers then
/// re-observe the persisted material handles rather than silently allocating a
/// replacement with the old identity. Generic preparation remains a separate
/// orchestration concern.
pub struct PreparedWorldControlPlane {
    workcell_ref: WorkcellRef,
    worlds: BTreeMap<String, MaterialisedExecutionWorld>,
    workspace_providers: BTreeMap<String, Box<dyn ErasedWorkspaceProvider>>,
    execution_providers: BTreeMap<String, Box<dyn ErasedExecutionProvider>>,
    runtime_providers: BTreeMap<String, Box<dyn ErasedRuntimeProvider>>,
    service_providers: BTreeMap<String, Box<dyn ErasedServiceProvider>>,
    artifact_providers: BTreeMap<String, Box<dyn ErasedArtifactProvider>>,
}

impl PreparedWorldControlPlane {
    pub fn new(workcell_ref: WorkcellRef) -> Self {
        Self {
            workcell_ref,
            worlds: BTreeMap::new(),
            workspace_providers: BTreeMap::new(),
            execution_providers: BTreeMap::new(),
            runtime_providers: BTreeMap::new(),
            service_providers: BTreeMap::new(),
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

    pub fn register_workspace_provider<P>(&mut self, provider: P) -> Result<()>
    where
        P: WorkspaceProvider + 'static,
    {
        let key = ProviderPort::provider_ref(&provider).to_string();
        register_provider(&self.workspace_providers, &key, "workspace")?;
        self.workspace_providers.insert(key, Box::new(provider));
        Ok(())
    }

    pub fn register_execution_provider<P>(&mut self, provider: P) -> Result<()>
    where
        P: ExecutionProvider + 'static,
    {
        let key = ProviderPort::provider_ref(&provider).to_string();
        register_provider(&self.execution_providers, &key, "execution")?;
        self.execution_providers.insert(key, Box::new(provider));
        Ok(())
    }

    pub fn register_runtime_provider<P>(&mut self, provider: P) -> Result<()>
    where
        P: RuntimeExposureProvider + 'static,
    {
        let key = ProviderPort::provider_ref(&provider).to_string();
        register_provider(&self.runtime_providers, &key, "runtime")?;
        self.runtime_providers.insert(key, Box::new(provider));
        Ok(())
    }

    pub fn register_service_provider<P>(&mut self, provider: P) -> Result<()>
    where
        P: ServiceProvider + 'static,
    {
        let key = ProviderPort::provider_ref(&provider).to_string();
        register_provider(&self.service_providers, &key, "service")?;
        self.service_providers.insert(key, Box::new(provider));
        Ok(())
    }

    pub fn register_artifact_provider<P>(&mut self, provider: P) -> Result<()>
    where
        P: ArtifactStorageProvider + 'static,
    {
        let key = ProviderPort::provider_ref(&provider).to_string();
        register_provider(&self.artifact_providers, &key, "artifact")?;
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

    fn provider_offers(&self, binding: &Binding) -> Result<Vec<OperationalOffer>> {
        match binding.port {
            ProviderPortKind::Workspace => self
                .workspace_providers
                .get(binding.provider_ref.as_str())
                .ok_or_else(|| provider_unavailable(binding))?
                .offers(),
            ProviderPortKind::Execution => self
                .execution_providers
                .get(binding.provider_ref.as_str())
                .ok_or_else(|| provider_unavailable(binding))?
                .offers(),
            ProviderPortKind::ProjectRuntime => self
                .runtime_providers
                .get(binding.provider_ref.as_str())
                .ok_or_else(|| provider_unavailable(binding))?
                .offers(),
            ProviderPortKind::Service => self
                .service_providers
                .get(binding.provider_ref.as_str())
                .ok_or_else(|| provider_unavailable(binding))?
                .offers(),
            ProviderPortKind::ArtifactStorage => self
                .artifact_providers
                .get(binding.provider_ref.as_str())
                .ok_or_else(|| provider_unavailable(binding))?
                .offers(),
        }
    }

    fn observe_binding(&self, binding: &Binding) -> Result<ProviderObservation> {
        let allocation = allocation_from_binding(binding);
        let observed = match binding.port {
            ProviderPortKind::Workspace => self
                .workspace_providers
                .get(binding.provider_ref.as_str())
                .ok_or_else(|| provider_unavailable(binding))?
                .observe(&allocation),
            ProviderPortKind::Execution => self
                .execution_providers
                .get(binding.provider_ref.as_str())
                .ok_or_else(|| provider_unavailable(binding))?
                .observe(&allocation),
            ProviderPortKind::ProjectRuntime => self
                .runtime_providers
                .get(binding.provider_ref.as_str())
                .ok_or_else(|| provider_unavailable(binding))?
                .observe(&allocation),
            ProviderPortKind::Service => self
                .service_providers
                .get(binding.provider_ref.as_str())
                .ok_or_else(|| provider_unavailable(binding))?
                .observe(&allocation),
            ProviderPortKind::ArtifactStorage => self
                .artifact_providers
                .get(binding.provider_ref.as_str())
                .ok_or_else(|| provider_unavailable(binding))?
                .observe(&allocation),
        }?;

        if observed.provider_ref != binding.provider_ref
            || observed.material_ref != binding.material_ref
        {
            return Err(WorkcellError::OperationFailed(format!(
                "provider observation escaped material binding `{}`",
                binding.logical_ref
            )));
        }
        Ok(observed)
    }

    fn release_binding(
        &mut self,
        binding: &Binding,
        retention: &RetentionExpectation,
    ) -> Result<ProviderReleaseResult> {
        let allocation = allocation_from_binding(binding);
        match binding.port {
            ProviderPortKind::Workspace => self
                .workspace_providers
                .get_mut(binding.provider_ref.as_str())
                .ok_or_else(|| provider_unavailable(binding))?
                .release(&allocation, retention),
            ProviderPortKind::Execution => self
                .execution_providers
                .get_mut(binding.provider_ref.as_str())
                .ok_or_else(|| provider_unavailable(binding))?
                .release(&allocation, retention),
            ProviderPortKind::ProjectRuntime => self
                .runtime_providers
                .get_mut(binding.provider_ref.as_str())
                .ok_or_else(|| provider_unavailable(binding))?
                .release(&allocation, retention),
            ProviderPortKind::Service => self
                .service_providers
                .get_mut(binding.provider_ref.as_str())
                .ok_or_else(|| provider_unavailable(binding))?
                .release(&allocation, retention),
            ProviderPortKind::ArtifactStorage => self
                .artifact_providers
                .get_mut(binding.provider_ref.as_str())
                .ok_or_else(|| provider_unavailable(binding))?
                .release(&allocation, retention),
        }
    }

    fn observation_for(&self, binding: &Binding) -> MaterialObservation {
        let mut detail = binding.properties.clone();
        detail.insert("provider_ref".into(), binding.provider_ref.to_string());
        detail.insert("material_ref".into(), binding.material_ref.clone());
        detail.insert("lifecycle".into(), presence_name(&binding.presence).into());

        if binding.presence == BindingPresence::Released {
            return MaterialObservation {
                logical_ref: binding.logical_ref.clone(),
                state: HealthState::Unavailable,
                detail,
            };
        }

        match self.observe_binding(binding) {
            Ok(observed) => {
                detail.extend(observed.detail);
                MaterialObservation {
                    logical_ref: binding.logical_ref.clone(),
                    state: observed.health,
                    detail,
                }
            }
            Err(error) => {
                detail.insert("observation_error".into(), error.to_string());
                MaterialObservation {
                    logical_ref: binding.logical_ref.clone(),
                    state: HealthState::Unavailable,
                    detail,
                }
            }
        }
    }

    fn binding_location(&self, logical_ref: &str) -> Result<Option<(String, usize)>> {
        let matches = self
            .worlds
            .iter()
            .flat_map(|(world_key, world)| {
                world
                    .binding_graph
                    .bindings
                    .iter()
                    .enumerate()
                    .filter(move |(_, binding)| binding.logical_ref == logical_ref)
                    .map(move |(index, _)| (world_key.clone(), index))
            })
            .collect::<Vec<_>>();
        match matches.as_slice() {
            [] => Ok(None),
            [one] => Ok(Some(one.clone())),
            _ => Err(WorkcellError::InvalidDemand(format!(
                "desired logical ref `{logical_ref}` is ambiguous across registered worlds"
            ))),
        }
    }

    fn refresh_binding(
        &mut self,
        world_key: &str,
        binding_index: usize,
    ) -> Result<(String, BindingPresence)> {
        let snapshot = self
            .worlds
            .get(world_key)
            .and_then(|world| world.binding_graph.bindings.get(binding_index))
            .cloned()
            .ok_or_else(|| {
                WorkcellError::ReconciliationFailed(
                    "binding disappeared while reconciliation was running".into(),
                )
            })?;

        if snapshot.presence == BindingPresence::Released {
            return Ok(("released".into(), BindingPresence::Released));
        }

        let offer_state = self.provider_offers(&snapshot);
        let observation = match offer_state {
            Ok(offers)
                if offers
                    .iter()
                    .any(|offer| offer.offer_ref == snapshot.offer_ref) =>
            {
                self.observe_binding(&snapshot)
            }
            Ok(_) => Err(WorkcellError::Unavailable(format!(
                "provider `{}` no longer advertises offer `{}` for `{}`",
                snapshot.provider_ref, snapshot.offer_ref, snapshot.logical_ref
            ))),
            Err(error) => Err(error),
        };

        let (observed, presence, health) = match observation {
            Ok(observation) => (
                format!("present:{}", health_name(&observation.health)),
                BindingPresence::Present,
                observation.health,
            ),
            Err(WorkcellError::NotFound(detail)) => (
                format!("missing:{detail}"),
                BindingPresence::Missing,
                HealthState::Unavailable,
            ),
            Err(error) => (
                format!("stale:{error}"),
                BindingPresence::Stale,
                HealthState::Unavailable,
            ),
        };

        let world = self.worlds.get_mut(world_key).ok_or_else(|| {
            WorkcellError::ReconciliationFailed(
                "world disappeared while reconciliation was running".into(),
            )
        })?;
        let binding = world
            .binding_graph
            .bindings
            .get_mut(binding_index)
            .ok_or_else(|| {
                WorkcellError::ReconciliationFailed(
                    "binding disappeared while reconciliation was running".into(),
                )
            })?;
        binding.presence = presence.clone();
        binding.health = health;
        world.state = world_health(&world.binding_graph.bindings);

        Ok((observed, presence))
    }
}

impl WorkcellControlPlane for PreparedWorldControlPlane {
    fn discover(&self) -> Result<Discovery> {
        let mut offers = Vec::new();
        for provider in self.workspace_providers.values() {
            offers.extend(provider.offers()?);
        }
        for provider in self.execution_providers.values() {
            offers.extend(provider.offers()?);
        }
        for provider in self.runtime_providers.values() {
            offers.extend(provider.offers()?);
        }
        for provider in self.service_providers.values() {
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
        Ok(ObservationBundle {
            world_ref: world.world_ref.clone(),
            observations: world
                .binding_graph
                .bindings
                .iter()
                .map(|binding| self.observation_for(binding))
                .collect(),
        })
    }

    fn expose(&self, world_ref: &WorldRef) -> Result<ExposureBundle> {
        let world = self.require_world(world_ref)?;
        let mut surfaces = Vec::new();
        let mut degradations = Vec::new();
        let mut omissions = Vec::new();

        for planned in &world.planned_exposures {
            let result: Result<Exposure> = (|| {
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
            let result: Result<Vec<CollectedOutput>> = (|| {
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
        if retention == RetentionExpectation::Preserve {
            return Ok(ReleaseResult {
                world_ref: snapshot.world_ref,
                disposition: ReleaseDisposition::Preserved,
                changed: false,
            });
        }

        let mut changed = false;
        for binding in snapshot.binding_graph.bindings {
            if presence_satisfies(&binding.presence, &retention) {
                continue;
            }
            if binding.presence == BindingPresence::Missing {
                continue;
            }

            let result = self.release_binding(&binding, &retention)?;
            changed |= result.changed;
            let world = self.worlds.get_mut(world_ref.as_str()).ok_or_else(|| {
                WorkcellError::NotFound(format!("material world `{world_ref}` is not registered"))
            })?;
            let current = world
                .binding_graph
                .bindings
                .iter_mut()
                .find(|candidate| candidate.binding_ref == binding.binding_ref)
                .ok_or_else(|| {
                    WorkcellError::OperationFailed(format!(
                        "binding `{}` disappeared during release",
                        binding.binding_ref
                    ))
                })?;
            current.presence = presence_for_disposition(&result.disposition);
            current.health = match result.disposition {
                ReleaseDisposition::Preserved => current.health.clone(),
                ReleaseDisposition::Suspended | ReleaseDisposition::Snapshotted => {
                    HealthState::Degraded
                }
                ReleaseDisposition::Released => HealthState::Unavailable,
            };
            world.state = world_health(&world.binding_graph.bindings);
        }

        Ok(ReleaseResult {
            world_ref: snapshot.world_ref,
            disposition: disposition_for(&retention),
            changed,
        })
    }

    fn reconcile(&mut self, desired: &[DesiredMaterialState]) -> Result<ReconciliationResult> {
        let mut seen = BTreeSet::new();
        let mut deltas = Vec::with_capacity(desired.len());

        for target in desired {
            if target.logical_ref.trim().is_empty() || target.desired.trim().is_empty() {
                return Err(WorkcellError::InvalidDemand(
                    "desired material state requires non-empty logical_ref and desired".into(),
                ));
            }
            if !seen.insert(target.logical_ref.clone()) {
                return Err(WorkcellError::InvalidDemand(format!(
                    "desired logical ref `{}` appears more than once",
                    target.logical_ref
                )));
            }
            let desired_state = target.desired.trim().to_ascii_lowercase();
            let desired_retention = match desired_state.as_str() {
                "present" => None,
                "released" => Some(RetentionExpectation::Release),
                "suspended" => Some(RetentionExpectation::SuspendIfSupported),
                "snapshotted" => Some(RetentionExpectation::SnapshotIfSupported),
                _ => {
                    return Err(WorkcellError::InvalidDemand(format!(
                        "unsupported desired material state `{}`; expected present, released, suspended or snapshotted",
                        target.desired
                    )))
                }
            };

            let Some((world_key, binding_index)) = self.binding_location(&target.logical_ref)?
            else {
                deltas.push(ReconciliationDelta {
                    logical_ref: target.logical_ref.clone(),
                    observed: None,
                    desired: desired_state,
                    action: Some("unbound".into()),
                });
                continue;
            };

            let persistence = self
                .worlds
                .get(&world_key)
                .and_then(|world| world.persistence.clone());
            let (observed, presence) = self.refresh_binding(&world_key, binding_index)?;

            let action = if desired_state == "present" {
                match presence {
                    BindingPresence::Present => None,
                    BindingPresence::Released => Some("rematerialise".into()),
                    BindingPresence::Missing | BindingPresence::Stale => {
                        if persistence == Some(PersistenceScope::Ephemeral) {
                            Some("lost".into())
                        } else {
                            Some("recover".into())
                        }
                    }
                    BindingPresence::Suspended => Some("resume".into()),
                    BindingPresence::Snapshotted => Some("restore".into()),
                }
            } else if presence_satisfies(
                &presence,
                desired_retention
                    .as_ref()
                    .expect("non-present state has retention"),
            ) {
                None
            } else if presence == BindingPresence::Missing {
                Some("already-absent".into())
            } else {
                let binding = self
                    .worlds
                    .get(&world_key)
                    .and_then(|world| world.binding_graph.bindings.get(binding_index))
                    .cloned()
                    .ok_or_else(|| {
                        WorkcellError::ReconciliationFailed(
                            "binding disappeared before lifecycle action".into(),
                        )
                    })?;
                let retention = desired_retention
                    .as_ref()
                    .expect("non-present state has retention");
                let released = self.release_binding(&binding, retention)?;
                let new_presence = presence_for_disposition(&released.disposition);
                let world = self.worlds.get_mut(&world_key).ok_or_else(|| {
                    WorkcellError::ReconciliationFailed(
                        "world disappeared before lifecycle action".into(),
                    )
                })?;
                let current = world
                    .binding_graph
                    .bindings
                    .get_mut(binding_index)
                    .ok_or_else(|| {
                        WorkcellError::ReconciliationFailed(
                            "binding disappeared before lifecycle action".into(),
                        )
                    })?;
                current.presence = new_presence;
                current.health = match released.disposition {
                    ReleaseDisposition::Released => HealthState::Unavailable,
                    ReleaseDisposition::Suspended | ReleaseDisposition::Snapshotted => {
                        HealthState::Degraded
                    }
                    ReleaseDisposition::Preserved => current.health.clone(),
                };
                world.state = world_health(&world.binding_graph.bindings);
                Some(
                    match released.disposition {
                        ReleaseDisposition::Released => "released",
                        ReleaseDisposition::Suspended => "suspended",
                        ReleaseDisposition::Snapshotted => "snapshotted",
                        ReleaseDisposition::Preserved => "preserved",
                    }
                    .into(),
                )
            };

            deltas.push(ReconciliationDelta {
                logical_ref: target.logical_ref.clone(),
                observed: Some(observed),
                desired: desired_state,
                action,
            });
        }

        Ok(ReconciliationResult { deltas })
    }
}

fn register_provider<T>(providers: &BTreeMap<String, T>, key: &str, family: &str) -> Result<()> {
    if providers.contains_key(key) {
        return Err(WorkcellError::OperationFailed(format!(
            "{family} provider `{key}` is already registered"
        )));
    }
    Ok(())
}

fn provider_unavailable(binding: &Binding) -> WorkcellError {
    WorkcellError::Unavailable(format!(
        "{} provider `{}` is not registered for `{}`",
        binding.port.as_str(),
        binding.provider_ref,
        binding.logical_ref
    ))
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

fn presence_satisfies(presence: &BindingPresence, retention: &RetentionExpectation) -> bool {
    matches!(
        (presence, retention),
        (BindingPresence::Released, RetentionExpectation::Release)
            | (
                BindingPresence::Suspended,
                RetentionExpectation::SuspendIfSupported
            )
            | (
                BindingPresence::Snapshotted,
                RetentionExpectation::SnapshotIfSupported
            )
    )
}

fn presence_for_disposition(disposition: &ReleaseDisposition) -> BindingPresence {
    match disposition {
        ReleaseDisposition::Released => BindingPresence::Released,
        ReleaseDisposition::Preserved => BindingPresence::Present,
        ReleaseDisposition::Suspended => BindingPresence::Suspended,
        ReleaseDisposition::Snapshotted => BindingPresence::Snapshotted,
    }
}

fn presence_name(presence: &BindingPresence) -> &'static str {
    match presence {
        BindingPresence::Present => "present",
        BindingPresence::Missing => "missing",
        BindingPresence::Released => "released",
        BindingPresence::Suspended => "suspended",
        BindingPresence::Snapshotted => "snapshotted",
        BindingPresence::Stale => "stale",
    }
}

fn health_name(health: &HealthState) -> &'static str {
    match health {
        HealthState::Healthy => "healthy",
        HealthState::Degraded => "degraded",
        HealthState::Unavailable => "unavailable",
        HealthState::Unknown => "unknown",
    }
}

fn world_health(bindings: &[Binding]) -> HealthState {
    let active = bindings
        .iter()
        .filter(|binding| binding.presence == BindingPresence::Present)
        .collect::<Vec<_>>();
    if active.is_empty() {
        if bindings.iter().any(|binding| {
            matches!(
                binding.presence,
                BindingPresence::Suspended | BindingPresence::Snapshotted
            )
        }) {
            return HealthState::Degraded;
        }
        return HealthState::Unavailable;
    }
    if active
        .iter()
        .any(|binding| binding.health == HealthState::Unavailable)
        || bindings.iter().any(|binding| {
            matches!(
                binding.presence,
                BindingPresence::Missing | BindingPresence::Stale
            )
        })
    {
        return HealthState::Unavailable;
    }
    if active
        .iter()
        .any(|binding| matches!(binding.health, HealthState::Degraded | HealthState::Unknown))
    {
        HealthState::Degraded
    } else {
        HealthState::Healthy
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
