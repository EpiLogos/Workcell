use std::{cell::RefCell, collections::BTreeMap, fs, path::PathBuf, rc::Rc};

use epilogos_workcell_artifact::DirectoryArtifactStorageProvider;
use epilogos_workcell_core::{
    compose_world, ArtifactChannelRequest, ArtifactStorageProvider, CollectionBundle,
    DesiredMaterialState, Discovery, ExecutionDemand, ExecutionMaterialRequest, ExecutionProvider,
    ExposureBundle, LogicalConnectionRequirement, MaterialisationPlan, MaterialisedExecutionWorld,
    ObservationBundle, PlanStatus, PlannedAllocation, PreparedWorldControlPlane,
    ProviderAllocation, ProviderObservation, ProviderPort, ProviderPortKind, ProviderRef,
    ProviderReleaseResult, ReconciliationResult, ReleaseResult, Result, RetentionExpectation,
    ServiceMaterialRequest, ServiceProvider, WorkcellControlPlane, WorkcellError, WorkcellRef,
    WorkspaceAccess, WorkspaceMaterialRequest, WorkspaceMaterialSource, WorkspaceProvider,
    WorldRef,
};
use epilogos_workcell_workspace::DirectoryWorkspaceProvider;

use crate::{
    service_declaration::{read_service_declarations, read_state_root_service_declarations},
    DeclaredServices, ExternalManagedService, ExternalManagedServiceProvider,
    HostProcessExecutionProvider, ManagedHostService, ManagedHostServiceProvider,
};

/// Provider identity for operator-declared services whose material process is a
/// child of this Workcell process.
pub const MANAGED_SERVICE_PROVIDER_REF: &str = "provider:collapsed-local-managed-services";
/// Provider identity for operator-declared services that something outside
/// Workcell starts and supervises.
pub const TARGET_SERVICE_PROVIDER_REF: &str = "provider:collapsed-local-target-services";

/// Where a collapsed-local Workcell reads its operator-declared services.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub enum ServiceDeclarationSource {
    /// `<state-root>/services.json` when that file exists, nothing otherwise.
    ///
    /// This is the default so that every host of a collapsed-local Workcell —
    /// the zero-daemon CLI, the Control Service, anything else that composes it
    /// — reads the same declaration without separate wiring.
    #[default]
    StateRoot,
    /// A named file, which must exist.
    File(PathBuf),
    /// Read nothing. Only services passed programmatically are declared.
    None,
}

#[derive(Clone, Debug)]
pub struct CollapsedLocalConfig {
    pub workcell_ref: WorkcellRef,
    pub state_root: PathBuf,
    pub workspace_source: Option<PathBuf>,
    pub artifact_channels: Vec<String>,
    /// Services declared programmatically by the host composing this Workcell.
    ///
    /// Empty by default: a Workcell that has been told about no service offers
    /// none, and a demand for one is honestly unsatisfiable.
    pub services: DeclaredServices,
    /// Where further declarations are read from at composition time.
    pub service_declaration: ServiceDeclarationSource,
}

impl CollapsedLocalConfig {
    pub fn new(workcell_ref: WorkcellRef, state_root: impl Into<PathBuf>) -> Self {
        Self {
            workcell_ref,
            state_root: state_root.into(),
            workspace_source: None,
            artifact_channels: vec!["logs:run".into(), "artifacts:run".into()],
            services: DeclaredServices::default(),
            service_declaration: ServiceDeclarationSource::StateRoot,
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

    /// Declare a service whose material process this Workcell process starts and
    /// owns. The child does not outlive the process that resolved it.
    pub fn with_managed_service(mut self, service: ManagedHostService) -> Self {
        self.services.managed.push(service);
        self
    }

    /// Declare a service that something outside Workcell starts and supervises.
    pub fn with_target_owned_service(mut self, service: ExternalManagedService) -> Self {
        self.services.target_owned.push(service);
        self
    }

    pub fn with_declared_services(mut self, services: DeclaredServices) -> Self {
        self.services.managed.extend(services.managed);
        self.services.target_owned.extend(services.target_owned);
        self
    }

    /// Read declarations from a named file instead of the state root.
    pub fn with_service_declaration_file(mut self, path: impl Into<PathBuf>) -> Self {
        self.service_declaration = ServiceDeclarationSource::File(path.into());
        self
    }

    /// Read no declaration file at all.
    pub fn without_service_declarations(mut self) -> Self {
        self.service_declaration = ServiceDeclarationSource::None;
        self
    }

    /// Resolve the declaration source into concrete services.
    pub fn resolve_services(&self) -> Result<DeclaredServices> {
        let mut resolved = match &self.service_declaration {
            ServiceDeclarationSource::StateRoot => {
                read_state_root_service_declarations(&self.state_root)?
            }
            ServiceDeclarationSource::File(path) => read_service_declarations(path)?,
            ServiceDeclarationSource::None => DeclaredServices::default(),
        };
        resolved
            .managed
            .extend(self.services.managed.iter().cloned());
        resolved
            .target_owned
            .extend(self.services.target_owned.iter().cloned());

        // Each provider refuses duplicates within itself. One logical ref
        // declared under both lifetimes would produce two offers for the same
        // service and let the planner silently pick one, so it is refused here.
        let mut seen = std::collections::BTreeSet::new();
        for logical_ref in resolved
            .managed
            .iter()
            .map(|service| &service.logical_ref)
            .chain(resolved.target_owned.iter().map(|s| &s.logical_ref))
        {
            if !seen.insert(logical_ref.clone()) {
                return Err(WorkcellError::InvalidDemand(format!(
                    "logical service `{logical_ref}` is declared more than once"
                )));
            }
        }
        Ok(resolved)
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

impl<P> ServiceProvider for SharedProvider<P>
where
    P: ServiceProvider,
{
    fn resolve_service(&mut self, request: &ServiceMaterialRequest) -> Result<ProviderAllocation> {
        self.inner.borrow_mut().resolve_service(request)
    }

    fn observe_service(&self, allocation: &ProviderAllocation) -> Result<ProviderObservation> {
        self.inner.borrow().observe_service(allocation)
    }

    fn release_service(
        &mut self,
        allocation: &ProviderAllocation,
        retention: &RetentionExpectation,
    ) -> Result<ProviderReleaseResult> {
        self.inner
            .borrow_mut()
            .release_service(allocation, retention)
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
    managed_services: SharedProvider<ManagedHostServiceProvider>,
    target_services: SharedProvider<ExternalManagedServiceProvider>,
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

        let declared = config.resolve_services()?;

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

        // Service ports are always registered, even with nothing declared. An
        // empty provider offers nothing, so a demand for a service nobody
        // declared stays honestly unsatisfiable; what changes is that the port
        // now exists on a real machine instead of only inside runtime tests.
        let managed_services = SharedProvider::new(ManagedHostServiceProvider::new(
            ProviderRef::new(MANAGED_SERVICE_PROVIDER_REF).unwrap(),
            declared.managed,
        )?);
        let target_services = SharedProvider::new(ExternalManagedServiceProvider::new(
            ProviderRef::new(TARGET_SERVICE_PROVIDER_REF).unwrap(),
            declared.target_owned,
        )?);

        let mut control = PreparedWorldControlPlane::new(config.workcell_ref.clone());
        control.register_workspace_provider(workspace.clone())?;
        control.register_execution_provider(execution.clone())?;
        control.register_artifact_provider(artifacts.clone())?;
        control.register_service_provider(managed_services.clone())?;
        control.register_service_provider(target_services.clone())?;

        Ok(Self {
            workcell_ref: config.workcell_ref,
            workspace_source: config.workspace_source,
            control,
            workspace,
            execution,
            artifacts,
            managed_services,
            target_services,
        })
    }

    pub fn world(&self, world_ref: &WorldRef) -> Option<&MaterialisedExecutionWorld> {
        self.control.world(world_ref)
    }

    /// Re-enter a previously prepared material world from durable allocation
    /// provenance. The world's semantic and material identities are preserved;
    /// providers reconstruct their process-local records from the bindings.
    pub fn register_world(&mut self, world: MaterialisedExecutionWorld) -> Result<()> {
        self.control.register_world(world)
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
        let managed_service_ref = self.managed_services.provider_ref().clone();
        let target_service_ref = self.target_services.provider_ref().clone();

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
            } else if binding.provider_ref == managed_service_ref
                || binding.provider_ref == target_service_ref
            {
                let connection = service_connection(binding)?;
                let request = ServiceMaterialRequest {
                    demand_ref: demand.demand_ref.clone(),
                    connection,
                    persistence: demand.persistence.clone(),
                };
                if binding.provider_ref == managed_service_ref {
                    self.managed_services.resolve_service(&request)?
                } else {
                    self.target_services.resolve_service(&request)?
                }
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

/// A service binding must have come from a `connectivity:` requirement. Anything
/// else means the planner matched a service offer to a requirement this
/// composition cannot honestly materialise, and that is a failure, not a guess.
fn service_connection(
    binding: &epilogos_workcell_core::PlannedBinding,
) -> Result<LogicalConnectionRequirement> {
    if !binding.logical_ref.starts_with("connectivity:") {
        return Err(WorkcellError::OperationFailed(format!(
            "service provider selected for non-connectivity binding `{}`",
            binding.logical_ref
        )));
    }
    LogicalConnectionRequirement::new(binding.requirement.clone())
}

#[cfg(test)]
mod tests {
    use std::{
        fs,
        time::{SystemTime, UNIX_EPOCH},
    };

    use super::*;

    fn temp_root(label: &str) -> PathBuf {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        std::env::temp_dir().join(format!(
            "epilogos-workcell-collapsed-local-{label}-{}-{nonce}",
            std::process::id()
        ))
    }

    fn config(root: &PathBuf) -> CollapsedLocalConfig {
        CollapsedLocalConfig::new(WorkcellRef::new("workcell:local-test").unwrap(), root)
    }

    #[test]
    fn the_service_port_offers_nothing_until_something_is_declared() {
        let root = temp_root("no-services");
        let workcell = CollapsedLocalWorkcell::new(config(&root)).unwrap();
        let discovery = workcell.discover().unwrap();
        assert!(
            !discovery
                .offers
                .iter()
                .any(|offer| offer.port == ProviderPortKind::Service.as_str()),
            "an undeclared service must not be offered"
        );
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn a_declared_service_reaches_discovery_through_the_registered_port() {
        let root = temp_root("declared-service");
        let workcell = CollapsedLocalWorkcell::new(
            config(&root).with_target_owned_service(
                ExternalManagedService::new(
                    "inference:caller-owned",
                    "http://127.0.0.1:1",
                    // `false` always exits non-zero, so this service is
                    // declared and materially unavailable at the same time.
                    crate::ExternalServiceCommand::new("/usr/bin/false").unwrap(),
                )
                .unwrap(),
            ),
        )
        .unwrap();

        let discovery = workcell.discover().unwrap();
        let offer = discovery
            .offers
            .iter()
            .find(|offer| offer.port == ProviderPortKind::Service.as_str())
            .expect("a declared service must be offered through the service port");
        assert_eq!(offer.provider_ref.as_str(), TARGET_SERVICE_PROVIDER_REF);
        assert_eq!(offer.connections, vec!["inference:caller-owned".to_owned()]);
        assert_eq!(
            offer.availability,
            epilogos_workcell_core::Availability::Unavailable,
            "declaring a service must not assert that it is running"
        );

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn one_logical_service_declared_under_both_lifetimes_is_refused() {
        let root = temp_root("duplicate-lifetimes");
        let configured = config(&root)
            .with_managed_service(
                ManagedHostService::new("inference:x", "http://127.0.0.1:1", "/usr/bin/true")
                    .unwrap(),
            )
            .with_target_owned_service(
                ExternalManagedService::new(
                    "inference:x",
                    "http://127.0.0.1:1",
                    crate::ExternalServiceCommand::new("/usr/bin/true").unwrap(),
                )
                .unwrap(),
            );
        let error = configured.resolve_services().unwrap_err();
        assert!(
            error.to_string().contains("declared more than once"),
            "unexpected error: {error}"
        );

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn a_named_declaration_file_that_is_missing_is_an_error_not_an_empty_set() {
        let root = temp_root("missing-declaration");
        let configured = config(&root).with_service_declaration_file(root.join("absent.json"));
        let error = configured.resolve_services().unwrap_err();
        assert!(
            matches!(error, WorkcellError::NotFound(_)),
            "unexpected error: {error}"
        );

        let _ = fs::remove_dir_all(root);
    }
}
