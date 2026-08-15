use std::{
    cell::RefCell,
    collections::BTreeMap,
    fs,
    path::PathBuf,
    rc::Rc,
};

use epilogos_workcell_artifact::DirectoryArtifactStorageProvider;
use epilogos_workcell_core::{
    compose_world, ArtifactChannelRequest, ArtifactStorageProvider, CollectionBundle,
    DesiredMaterialState, Discovery, ExecutionDemand, ExecutionMaterialRequest, ExecutionProvider,
    ExposureBundle, LogicalConnectionRequirement, MaterialisationPlan, MaterialisedExecutionWorld,
    ObservationBundle, PlanStatus, PlannedAllocation, PreparedWorldControlPlane,
    ProviderAllocation, ProviderObservation, ProviderPort, ProviderPortKind, ProviderRef,
    ProviderReleaseResult, ReconciliationResult, ReleaseResult, Result, RetentionExpectation,
    WorkcellControlPlane, WorkcellError, WorkcellRef, WorkspaceAccess, WorkspaceMaterialRequest,
    WorkspaceMaterialSource, WorkspaceProvider, WorldRef,
};
use epilogos_workcell_workspace::DirectoryWorkspaceProvider;

use crate::HostProcessExecutionProvider;

#[derive(Clone, Debug)]
pub struct CollapsedLocalConfig {
    pub workcell_ref: WorkcellRef,
    pub state_root: PathBuf,
    pub workspace_source: Option<PathBuf>,
    pub artifact_channels: Vec<String>,
}

impl CollapsedLocalConfig {
    pub fn new(workcell_ref: WorkcellRef, state_root: impl Into<PathBuf>) -> Self {
        Self {
            workcell_ref,
            state_root: state_root.into(),
            workspace_source: None,
            artifact_channels: vec!["logs:run".into(), "artifacts:run".into()],
        }
    }

    pub fn with_workspace_source(mut self, source: impl Into<PathBuf>) -> Self {
        self.workspace_source = Some(source.into());
        self
    }

    pub fn with_artifact_channel(mut self, channel: impl Into<String>) -> Self {
        self.artifact_channels.push(channel.into());
        self
    }
}

struct SharedProvider<P> {
    provider_ref: ProviderRef,
    inner: Rc<RefCell<P>>,
}

impl<P> SharedProvider<P>
where
    P: ProviderPort,
{
    fn new(provider: P) -> Self {
        Self {
            provider_ref: provider.provider_ref().clone(),
            inner: Rc::new(RefCell::new(provider)),
        }
    }
}

impl<P> Clone for SharedProvider<P> {
    fn clone(&self) -> Self {
        Self {
            provider_ref: self.provider_ref.clone(),
            inner: Rc::clone(&self.inner),
        }
    }
}

impl<P> ProviderPort for SharedProvider<P>
where
    P: ProviderPort,
{
    fn provider_ref(&self) -> &ProviderRef {
        &self.provider_ref
    }

    fn port_kind(&self) -> ProviderPortKind {
        self.inner.borrow().port_kind()
    }

    fn offers(&self) -> Result<Vec<epilogos_workcell_core::OperationalOffer>> {
        self.inner.borrow().offers()
    }
}

impl<P> WorkspaceProvider for SharedProvider<P>
where
    P: WorkspaceProvider,
{
    fn prepare_workspace(
        &mut self,
        request: &WorkspaceMaterialRequest,
    ) -> Result<ProviderAllocation> {
        self.inner.borrow_mut().prepare_workspace(request)
    }

    fn observe_workspace(&self, allocation: &ProviderAllocation) -> Result<ProviderObservation> {
        self.inner.borrow().observe_workspace(allocation)
    }

    fn release_workspace(
        &mut self,
        allocation: &ProviderAllocation,
        retention: &RetentionExpectation,
    ) -> Result<ProviderReleaseResult> {
        self.inner
            .borrow_mut()
            .release_workspace(allocation, retention)
    }
}

impl<P> ArtifactStorageProvider for SharedProvider<P>
where
    P: ArtifactStorageProvider,
{
    fn prepare_artifact_channel(
        &mut self,
        request: &ArtifactChannelRequest,
    ) -> Result<ProviderAllocation> {
        self.inner.borrow_mut().prepare_artifact_channel(request)
    }

    fn collect_material(
        &self,
        allocation: &ProviderAllocation,
    ) -> Result<Vec<epilogos_workcell_core::ProviderCollectedMaterial>> {
        self.inner.borrow().collect_material(allocation)
    }

    fn observe_artifact_channel(
        &self,
        allocation: &ProviderAllocation,
    ) -> Result<ProviderObservation> {
        self.inner.borrow().observe_artifact_channel(allocation)
    }

    fn release_artifact_channel(
        &mut self,
        allocation: &ProviderAllocation,
        retention: &RetentionExpectation,
    ) -> Result<ProviderReleaseResult> {
        self.inner
            .borrow_mut()
            .release_artifact_channel(allocation, retention)
    }
}

/// Direct, zero-daemon Workcell composition for an ordinary local machine.
///
/// This type is an application layer over the existing planner, provider ports,
/// world composer, and prepared-world lifecycle control plane. It deliberately
/// does not change the F.11 recovery-only `PreparedWorldControlPlane::prepare`
/// boundary.
pub struct CollapsedLocalWorkcell {
    workcell_ref: WorkcellRef,
    workspace_source: Option<PathBuf>,
    control: PreparedWorldControlPlane,
    workspace: SharedProvider<DirectoryWorkspaceProvider>,
    execution: HostProcessExecutionProvider,
    artifacts: SharedProvider<DirectoryArtifactStorageProvider>,
}

impl CollapsedLocalWorkcell {
    pub fn new(config: CollapsedLocalConfig) -> Result<Self> {
        if config.state_root.as_os_str().is_empty() {
            return Err(WorkcellError::InvalidDemand(
                "collapsed-local state root must not be empty".into(),
            ));
        }
        fs::create_dir_all(&config.state_root).map_err(|error| {
            WorkcellError::OperationFailed(format!("create collapsed-local state root: {error}"))
        })?;

        let workspace = SharedProvider::new(DirectoryWorkspaceProvider::new(
            ProviderRef::new("provider:collapsed-local-workspace").unwrap(),
            config.state_root.join("workspaces"),
        ));
        let execution = HostProcessExecutionProvider::new(
            ProviderRef::new("provider:collapsed-local-host-process").unwrap(),
        );
        let artifacts = SharedProvider::new(DirectoryArtifactStorageProvider::new(
            ProviderRef::new("provider:collapsed-local-artifacts").unwrap(),
            config.state_root.join("artifacts"),
            config.artifact_channels,
        )?);

        let mut control = PreparedWorldControlPlane::new(config.workcell_ref.clone());
        control.register_workspace_provider(workspace.clone())?;
        control.register_execution_provider(execution.clone())?;
        control.register_artifact_provider(artifacts.clone())?;

        Ok(Self {
            workcell_ref: config.workcell_ref,
            workspace_source: config.workspace_source,
            control,
            workspace,
            execution,
            artifacts,
        })
    }

    pub fn world(&self, world_ref: &WorldRef) -> Option<&MaterialisedExecutionWorld> {
        self.control.world(world_ref)
    }

    fn workspace_request(
        &self,
        demand: &ExecutionDemand,
        fallback_requirement: &str,
    ) -> Result<WorkspaceMaterialRequest> {
        let (source, revision, access) = if let Some(workspace) = &demand.workspace {
            (
                workspace.source.clone(),
                workspace.revision.clone(),
                workspace.access.clone(),
            )
        } else {
            let access = match fallback_requirement {
                "workspace:read-only" => WorkspaceAccess::ReadOnly,
                "workspace:writable" => WorkspaceAccess::Writable,
                other => {
                    return Err(WorkcellError::InvalidDemand(format!(
                        "cannot infer workspace access from `{other}`"
                    )))
                }
            };
            (None, None, access)
        };

        if source.is_some() && self.workspace_source.is_none() {
            return Err(WorkcellError::UnsatisfiedDemand(
                "workspace has a semantic source ref but collapsed-local has no material source binding"
                    .into(),
            ));
        }
        let material_source = self.workspace_source.as_ref().map(|path| {
            let mut provenance = BTreeMap::new();
            provenance.insert("binding_source".into(), "collapsed-local-config".into());
            WorkspaceMaterialSource {
                locator: path.display().to_string(),
                provenance,
            }
        });

        Ok(WorkspaceMaterialRequest {
            demand_ref: demand.demand_ref.clone(),
            source,
            material_source,
            revision,
            access,
            persistence: demand.persistence.clone(),
            retention: demand.retention.clone(),
        })
    }

    fn prepare_world(&mut self, demand: &ExecutionDemand) -> Result<MaterialisedExecutionWorld> {
        let plan = self.control.plan(demand)?;
        if plan.status == PlanStatus::Unsatisfiable {
            let missing = plan
                .omissions
                .iter()
                .filter(|item| {
                    item.necessity == epilogos_workcell_core::RequirementNecessity::Required
                })
                .map(|item| item.requirement.as_str())
                .collect::<Vec<_>>()
                .join(", ");
            return Err(WorkcellError::UnsatisfiedDemand(format!(
                "collapsed-local cannot satisfy required demand: {missing}"
            )));
        }

        let workspace_ref = self.workspace.provider_ref().clone();
        let execution_ref = self.execution.provider_ref().clone();
        let artifact_ref = self.artifacts.provider_ref().clone();

        let mut workspace_allocation = None;
        let mut execution_allocation = None;
        let mut allocations = Vec::new();

        let execution_affordances = plan
            .planned_bindings
            .iter()
            .filter(|binding| {
                binding.provider_ref == execution_ref
                    && binding.logical_ref.starts_with("affordance:")
            })
            .map(|binding| binding.requirement.clone())
            .collect::<Vec<_>>();
        let execution_connectivity = plan
            .planned_bindings
            .iter()
            .filter(|binding| {
                binding.provider_ref == execution_ref
                    && binding.logical_ref.starts_with("connectivity:")
            })
            .map(|binding| LogicalConnectionRequirement::new(binding.requirement.clone()))
            .collect::<Result<Vec<_>>>()?;

        for binding in &plan.planned_bindings {
            let allocation = if binding.provider_ref == workspace_ref {
                if workspace_allocation.is_none() {
                    let request = self.workspace_request(demand, &binding.requirement)?;
                    workspace_allocation = Some(self.workspace.prepare_workspace(&request)?);
                }
                workspace_allocation
                    .clone()
                    .expect("workspace allocation set")
            } else if binding.provider_ref == execution_ref {
                if execution_allocation.is_none() {
                    execution_allocation = Some(self.execution.prepare_execution(
                        &ExecutionMaterialRequest {
                            demand_ref: demand.demand_ref.clone(),
                            affordances: execution_affordances.clone(),
                            resources: demand.resources.clone(),
                            connectivity: execution_connectivity.clone(),
                            isolation_trust: demand.isolation_trust.clone(),
                            retention: demand.retention.clone(),
                        },
                    )?);
                }
                execution_allocation
                    .clone()
                    .expect("execution allocation set")
            } else if binding.provider_ref == artifact_ref {
                if !binding.logical_ref.starts_with("output:") {
                    return Err(WorkcellError::OperationFailed(format!(
                        "artifact provider selected for non-output binding `{}`",
                        binding.logical_ref
                    )));
                }
                self.artifacts
                    .prepare_artifact_channel(&ArtifactChannelRequest {
                        demand_ref: demand.demand_ref.clone(),
                        logical_channel: binding.requirement.clone(),
                        persistence: demand.persistence.clone(),
                        retention: demand.retention.clone(),
                    })?
            } else {
                return Err(WorkcellError::OperationFailed(format!(
                    "collapsed-local selected unregistered preparation provider `{}`",
                    binding.provider_ref
                )));
            };

            allocations.push(PlannedAllocation {
                logical_ref: binding.logical_ref.clone(),
                offer_ref: binding.offer_ref.clone(),
                allocation,
            });
        }

        let world = compose_world(
            self.workcell_ref.clone(),
            demand,
            &plan,
            allocations,
            vec![],
        )?;
        self.control.register_world(world.clone())?;
        Ok(world)
    }
}

impl WorkcellControlPlane for CollapsedLocalWorkcell {
    fn discover(&self) -> Result<Discovery> {
        self.control.discover()
    }

    fn plan(&self, demand: &ExecutionDemand) -> Result<MaterialisationPlan> {
        self.control.plan(demand)
    }

    fn prepare(&mut self, demand: &ExecutionDemand) -> Result<MaterialisedExecutionWorld> {
        self.prepare_world(demand)
    }

    fn observe(&self, world: &WorldRef) -> Result<ObservationBundle> {
        self.control.observe(world)
    }

    fn expose(&self, world: &WorldRef) -> Result<ExposureBundle> {
        self.control.expose(world)
    }

    fn collect(&self, world: &WorldRef) -> Result<CollectionBundle> {
        self.control.collect(world)
    }

    fn release(&mut self, world: &WorldRef) -> Result<ReleaseResult> {
        self.control.release(world)
    }

    fn reconcile(&mut self, desired: &[DesiredMaterialState]) -> Result<ReconciliationResult> {
        self.control.reconcile(desired)
    }
}
