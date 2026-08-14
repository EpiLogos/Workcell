use std::{
    collections::BTreeMap,
    path::PathBuf,
    sync::Arc,
};

use epilogos_workcell_core::{
    validate_allocation, Availability, HealthState, MaterialExposureProvider, OfferRef,
    OperationalOffer, PersistenceScope, ProjectRuntimeMaterialRequest, ProjectRuntimeProvider,
    ProviderAllocation, ProviderExposedSurface, ProviderExposureRequest, ProviderObservation,
    ProviderPort, ProviderPortKind, ProviderRef, ProviderReleaseResult, ReleaseDisposition, Result,
    RetentionExpectation, WorkcellError,
};

use crate::{
    path_string, probe_compose, probe_engine, provider_metadata, short_lived_persistence, stable_key,
    DockerCommand, DockerCommandRunner, SystemDockerCommandRunner,
};

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DockerExposureTarget {
    pub service: String,
    pub container_port: u16,
    pub scheme: String,
    pub host: String,
    pub path: String,
}

impl DockerExposureTarget {
    pub fn http(service: impl Into<String>, container_port: u16) -> Result<Self> {
        let service = service.into();
        if service.trim().is_empty() || container_port == 0 {
            return Err(WorkcellError::InvalidDemand(
                "Docker exposure requires a service and non-zero container port".into(),
            ));
        }
        Ok(Self {
            service,
            container_port,
            scheme: "http".into(),
            host: "127.0.0.1".into(),
            path: "/".into(),
        })
    }

    pub fn with_scheme(mut self, scheme: impl Into<String>) -> Result<Self> {
        let scheme = scheme.into();
        if scheme.trim().is_empty() {
            return Err(WorkcellError::InvalidDemand(
                "Docker exposure scheme must not be empty".into(),
            ));
        }
        self.scheme = scheme;
        Ok(self)
    }

    pub fn with_host(mut self, host: impl Into<String>) -> Result<Self> {
        let host = host.into();
        if host.trim().is_empty() {
            return Err(WorkcellError::InvalidDemand(
                "Docker exposure host must not be empty".into(),
            ));
        }
        self.host = host;
        Ok(self)
    }

    pub fn with_path(mut self, path: impl Into<String>) -> Result<Self> {
        let mut path = path.into();
        if path.trim().is_empty() {
            return Err(WorkcellError::InvalidDemand(
                "Docker exposure path must not be empty".into(),
            ));
        }
        if !path.starts_with('/') {
            path.insert(0, '/');
        }
        self.path = path;
        Ok(self)
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DockerRuntimeMode {
    pub name: String,
    pub project_dir: PathBuf,
    pub compose_files: Vec<PathBuf>,
    pub connections: Vec<String>,
    pub exposures: BTreeMap<String, DockerExposureTarget>,
    pub environment: BTreeMap<String, String>,
}

impl DockerRuntimeMode {
    pub fn new(
        name: impl Into<String>,
        project_dir: impl Into<PathBuf>,
        compose_file: impl Into<PathBuf>,
    ) -> Result<Self> {
        let name = name.into();
        let project_dir = project_dir.into();
        let compose_file = compose_file.into();
        if name.trim().is_empty() {
            return Err(WorkcellError::InvalidDemand(
                "Docker runtime mode name must not be empty".into(),
            ));
        }
        if project_dir.as_os_str().is_empty() || compose_file.as_os_str().is_empty() {
            return Err(WorkcellError::InvalidDemand(
                "Docker runtime project directory and Compose file must not be empty".into(),
            ));
        }
        Ok(Self {
            name,
            project_dir,
            compose_files: vec![compose_file],
            connections: Vec::new(),
            exposures: BTreeMap::new(),
            environment: BTreeMap::new(),
        })
    }

    pub fn with_compose_file(mut self, compose_file: impl Into<PathBuf>) -> Result<Self> {
        let compose_file = compose_file.into();
        if compose_file.as_os_str().is_empty() {
            return Err(WorkcellError::InvalidDemand(
                "Compose file must not be empty".into(),
            ));
        }
        self.compose_files.push(compose_file);
        Ok(self)
    }

    pub fn with_connection(mut self, connection: impl Into<String>) -> Result<Self> {
        let connection = connection.into();
        if connection.trim().is_empty() {
            return Err(WorkcellError::InvalidDemand(
                "Docker runtime logical connection must not be empty".into(),
            ));
        }
        if !self.connections.iter().any(|item| item == &connection) {
            self.connections.push(connection);
        }
        Ok(self)
    }

    pub fn with_exposure(
        mut self,
        requirement: impl Into<String>,
        target: DockerExposureTarget,
    ) -> Result<Self> {
        let requirement = requirement.into();
        if requirement.trim().is_empty() {
            return Err(WorkcellError::InvalidDemand(
                "Docker runtime exposure requirement must not be empty".into(),
            ));
        }
        self.exposures.insert(requirement, target);
        Ok(self)
    }

    pub fn with_env(mut self, key: impl Into<String>, value: impl Into<String>) -> Result<Self> {
        let key = key.into();
        let value = value.into();
        if key.trim().is_empty() || value.trim().is_empty() {
            return Err(WorkcellError::InvalidDemand(
                "Docker runtime environment keys and values must not be empty".into(),
            ));
        }
        self.environment.insert(key, value);
        Ok(self)
    }
}

#[derive(Clone, Debug)]
struct RuntimeRecord {
    mode: DockerRuntimeMode,
    project_name: String,
    engine_version: String,
    compose_version: String,
    persistence: Option<PersistenceScope>,
}

pub struct DockerProjectRuntimeProvider {
    provider_ref: ProviderRef,
    modes: BTreeMap<String, DockerRuntimeMode>,
    runner: Arc<dyn DockerCommandRunner>,
    records: BTreeMap<String, RuntimeRecord>,
}

impl DockerProjectRuntimeProvider {
    pub fn new(
        provider_ref: ProviderRef,
        modes: impl IntoIterator<Item = DockerRuntimeMode>,
    ) -> Result<Self> {
        Self::with_runner(provider_ref, modes, Arc::new(SystemDockerCommandRunner))
    }

    pub fn with_runner(
        provider_ref: ProviderRef,
        modes: impl IntoIterator<Item = DockerRuntimeMode>,
        runner: Arc<dyn DockerCommandRunner>,
    ) -> Result<Self> {
        let mut by_name = BTreeMap::new();
        for mode in modes {
            if by_name.insert(mode.name.clone(), mode).is_some() {
                return Err(WorkcellError::InvalidDemand(
                    "Docker runtime provider contains duplicate mode names".into(),
                ));
            }
        }
        Ok(Self {
            provider_ref,
            modes: by_name,
            runner,
            records: BTreeMap::new(),
        })
    }

    fn record(&self, allocation: &ProviderAllocation) -> Result<&RuntimeRecord> {
        validate_allocation(self, allocation)?;
        self.records.get(&allocation.material_ref).ok_or_else(|| {
            WorkcellError::NotFound(format!(
                "Docker Compose runtime `{}` is not known by this provider",
                allocation.material_ref
            ))
        })
    }

    fn require_compose(&self) -> Result<(String, String)> {
        let engine = probe_engine(self.runner.as_ref()).map_err(|error| {
            WorkcellError::Unavailable(format!("Docker Engine is unavailable: {error}"))
        })?;
        let compose = probe_compose(self.runner.as_ref()).map_err(|error| {
            WorkcellError::Unavailable(format!("Docker Compose is unavailable: {error}"))
        })?;
        Ok((engine, compose))
    }

    fn command(
        &self,
        mode: &DockerRuntimeMode,
        project_name: &str,
        operation: impl IntoIterator<Item = impl Into<String>>,
    ) -> DockerCommand {
        let mut args = vec!["compose".into(), "-p".into(), project_name.into()];
        for file in &mode.compose_files {
            args.push("-f".into());
            args.push(path_string(file));
        }
        args.extend(operation.into_iter().map(Into::into));
        let mut command = DockerCommand::new(args).with_cwd(mode.project_dir.clone());
        for (key, value) in &mode.environment {
            command = command.with_env(key.clone(), value.clone());
        }
        command
    }

    fn validate_connections(
        &self,
        mode: &DockerRuntimeMode,
        request: &ProjectRuntimeMaterialRequest,
    ) -> Result<()> {
        for connection in &request.connectivity {
            if !mode
                .connections
                .iter()
                .any(|available| available == connection.as_str())
            {
                return Err(WorkcellError::UnsatisfiedDemand(format!(
                    "Docker runtime mode `{}` does not provide logical connection `{}`",
                    mode.name,
                    connection.as_str()
                )));
            }
        }
        Ok(())
    }

    pub fn restart_runtime(&self, allocation: &ProviderAllocation) -> Result<()> {
        let record = self.record(allocation)?;
        self.runner.run(&self.command(
            &record.mode,
            &record.project_name,
            ["restart"],
        ))?;
        Ok(())
    }
}

impl ProviderPort for DockerProjectRuntimeProvider {
    fn provider_ref(&self) -> &ProviderRef {
        &self.provider_ref
    }

    fn port_kind(&self) -> ProviderPortKind {
        ProviderPortKind::ProjectRuntime
    }

    fn offers(&self) -> Result<Vec<OperationalOffer>> {
        let engine_version = match probe_engine(self.runner.as_ref()) {
            Ok(version) => version,
            Err(_) => return Ok(Vec::new()),
        };
        let compose_version = match probe_compose(self.runner.as_ref()) {
            Ok(version) => version,
            Err(_) => return Ok(Vec::new()),
        };

        let mut offers = Vec::with_capacity(self.modes.len());
        for mode in self.modes.values() {
            let mut metadata = provider_metadata(&engine_version, Some(&compose_version));
            metadata.insert("runtime_mode".into(), mode.name.clone());
            offers.push(OperationalOffer {
                offer_ref: OfferRef::new(format!(
                    "offer:{}:docker-runtime:{}",
                    self.provider_ref, mode.name
                ))
                .map_err(|error| WorkcellError::OperationFailed(error.into()))?,
                provider_ref: self.provider_ref.clone(),
                port: ProviderPortKind::ProjectRuntime.as_str().into(),
                affordances: vec![format!("runtime-mode:{}", mode.name)],
                connections: mode.connections.clone(),
                exposures: mode.exposures.keys().cloned().collect(),
                isolation_trust: Vec::new(),
                availability: Availability::Available,
                health: HealthState::Healthy,
                capacity: BTreeMap::new(),
                metadata,
            });
        }
        Ok(offers)
    }
}

impl ProjectRuntimeProvider for DockerProjectRuntimeProvider {
    fn prepare_runtime(
        &mut self,
        request: &ProjectRuntimeMaterialRequest,
    ) -> Result<ProviderAllocation> {
        let (engine_version, compose_version) = self.require_compose()?;
        let mode = self.modes.get(&request.mode).cloned().ok_or_else(|| {
            WorkcellError::UnsatisfiedDemand(format!(
                "Docker runtime mode `{}` is not offered",
                request.mode
            ))
        })?;
        self.validate_connections(&mode, request)?;

        let key = stable_key(&[request.demand_ref.as_str(), &request.mode]);
        let project_name = format!("epilogos-wc-{}", &key[..12]);
        self.runner.run(&self.command(
            &mode,
            &project_name,
            ["up", "-d", "--remove-orphans"],
        ))?;

        let material_ref = format!("runtime:docker-compose:{project_name}");
        let record = RuntimeRecord {
            mode: mode.clone(),
            project_name: project_name.clone(),
            engine_version: engine_version.clone(),
            compose_version: compose_version.clone(),
            persistence: request.persistence.clone(),
        };
        self.records.insert(material_ref.clone(), record);

        let mut properties = BTreeMap::new();
        properties.insert("mode".into(), mode.name.clone());
        properties.insert("compose_project".into(), project_name.clone());
        if !request.connectivity.is_empty() {
            properties.insert(
                "logical_connections".into(),
                request
                    .connectivity
                    .iter()
                    .map(|item| item.as_str())
                    .collect::<Vec<_>>()
                    .join(","),
            );
        }
        if let Some(persistence) = &request.persistence {
            properties.insert("persistence".into(), format!("{persistence:?}"));
        }
        let mut provenance = provider_metadata(&engine_version, Some(&compose_version));
        provenance.insert("compose_project".into(), project_name);
        provenance.insert("project_dir".into(), path_string(&mode.project_dir));
        provenance.insert(
            "compose_files".into(),
            mode.compose_files
                .iter()
                .map(|path| path_string(path))
                .collect::<Vec<_>>()
                .join(","),
        );

        Ok(ProviderAllocation {
            provider_ref: self.provider_ref.clone(),
            port: ProviderPortKind::ProjectRuntime,
            material_ref,
            health: HealthState::Healthy,
            properties,
            provenance,
        })
    }

    fn observe_runtime(&self, allocation: &ProviderAllocation) -> Result<ProviderObservation> {
        let record = self.record(allocation)?;
        let all = self.runner.run(&self.command(
            &record.mode,
            &record.project_name,
            ["ps", "--all", "--quiet"],
        ))?;
        let running = self.runner.run(&self.command(
            &record.mode,
            &record.project_name,
            ["ps", "--status", "running", "--quiet"],
        ))?;
        let total_count = all.stdout.lines().filter(|line| !line.trim().is_empty()).count();
        let running_count = running
            .stdout
            .lines()
            .filter(|line| !line.trim().is_empty())
            .count();
        let health = if total_count == 0 {
            HealthState::Unavailable
        } else if running_count == total_count {
            HealthState::Healthy
        } else {
            HealthState::Degraded
        };
        let mut detail = BTreeMap::new();
        detail.insert("compose_project".into(), record.project_name.clone());
        detail.insert("services_total".into(), total_count.to_string());
        detail.insert("services_running".into(), running_count.to_string());
        detail.insert("docker_engine".into(), record.engine_version.clone());
        detail.insert("docker_compose".into(), record.compose_version.clone());
        Ok(ProviderObservation {
            provider_ref: self.provider_ref.clone(),
            material_ref: allocation.material_ref.clone(),
            health,
            detail,
        })
    }

    fn release_runtime(
        &mut self,
        allocation: &ProviderAllocation,
        retention: &RetentionExpectation,
    ) -> Result<ProviderReleaseResult> {
        let record = self.record(allocation)?.clone();
        match retention {
            RetentionExpectation::Preserve => Ok(ProviderReleaseResult {
                provider_ref: self.provider_ref.clone(),
                material_ref: allocation.material_ref.clone(),
                disposition: ReleaseDisposition::Preserved,
                changed: false,
            }),
            RetentionExpectation::Release => {
                let mut operation = vec!["down", "--remove-orphans"];
                if short_lived_persistence(record.persistence.as_ref()) {
                    operation.push("--volumes");
                }
                self.runner
                    .run(&self.command(&record.mode, &record.project_name, operation))
                    .map_err(|error| WorkcellError::CleanupFailed(error.to_string()))?;
                self.records.remove(&allocation.material_ref);
                Ok(ProviderReleaseResult {
                    provider_ref: self.provider_ref.clone(),
                    material_ref: allocation.material_ref.clone(),
                    disposition: ReleaseDisposition::Released,
                    changed: true,
                })
            }
            RetentionExpectation::SuspendIfSupported => {
                self.runner.run(&self.command(
                    &record.mode,
                    &record.project_name,
                    ["stop"],
                ))?;
                Ok(ProviderReleaseResult {
                    provider_ref: self.provider_ref.clone(),
                    material_ref: allocation.material_ref.clone(),
                    disposition: ReleaseDisposition::Suspended,
                    changed: true,
                })
            }
            RetentionExpectation::SnapshotIfSupported => Err(WorkcellError::Unsupported(
                "Docker Compose runtime provider does not claim snapshot semantics".into(),
            )),
        }
    }
}

impl MaterialExposureProvider for DockerProjectRuntimeProvider {
    fn expose_material(
        &self,
        allocation: &ProviderAllocation,
        request: &ProviderExposureRequest,
    ) -> Result<ProviderExposedSurface> {
        let record = self.record(allocation)?;
        let requirement = request.requirement.as_str();
        let target = record.mode.exposures.get(requirement).ok_or_else(|| {
            WorkcellError::UnsatisfiedDemand(format!(
                "Docker runtime mode `{}` does not expose `{requirement}`",
                record.mode.name
            ))
        })?;
        let port = target.container_port.to_string();
        let published = self.runner.run(&self.command(
            &record.mode,
            &record.project_name,
            ["port", target.service.as_str(), port.as_str()],
        ))?;
        let published_binding = published.stdout.trim();
        let published_port = published_binding
            .rsplit(':')
            .next()
            .and_then(|value| value.trim().parse::<u16>().ok())
            .ok_or_else(|| {
                WorkcellError::OperationFailed(format!(
                    "could not parse published port from Docker Compose output `{published_binding}`"
                ))
            })?;
        let endpoint = format!(
            "{}://{}:{}{}",
            target.scheme, target.host, published_port, target.path
        );

        let mut material = BTreeMap::new();
        material.insert("endpoint".into(), endpoint);
        material.insert("service".into(), target.service.clone());
        material.insert("container_port".into(), target.container_port.to_string());
        material.insert("published_binding".into(), published_binding.into());
        let mut provenance = allocation.provenance.clone();
        provenance.insert("compose_project".into(), record.project_name.clone());
        provenance.insert("provider_ref".into(), self.provider_ref.to_string());
        provenance.insert("exposure".into(), requirement.into());

        Ok(ProviderExposedSurface {
            provider_ref: self.provider_ref.clone(),
            material_ref: allocation.material_ref.clone(),
            logical_ref: format!("exposure:{requirement}"),
            interaction: requirement.into(),
            material,
            provenance,
        })
    }
}
