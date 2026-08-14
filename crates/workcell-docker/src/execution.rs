use std::{
    collections::{BTreeMap, BTreeSet},
    sync::Arc,
};

use epilogos_workcell_core::{
    validate_allocation, Availability, ExecutionMaterialRequest, ExecutionProvider, HealthState,
    OfferRef, OperationalOffer, ProviderAllocation, ProviderObservation, ProviderOperation,
    ProviderOperationResult, ProviderPort, ProviderPortKind, ProviderRef, ProviderReleaseResult,
    ReleaseDisposition, Result, RetentionExpectation, WorkcellError,
};

use crate::{
    docker_memory_bytes, probe_engine, provider_metadata, stable_key, DockerCommand,
    DockerCommandRunner, SystemDockerCommandRunner,
};

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DockerExecutionConfig {
    pub image: String,
    pub affordances: Vec<String>,
    pub logical_networks: BTreeMap<String, String>,
    pub isolation_trust: Vec<String>,
    pub hold_command: Vec<String>,
    pub shell_program: String,
}

impl DockerExecutionConfig {
    pub fn new(image: impl Into<String>) -> Result<Self> {
        let image = image.into();
        if image.trim().is_empty() {
            return Err(WorkcellError::InvalidDemand(
                "Docker execution image must not be empty".into(),
            ));
        }
        Ok(Self {
            image,
            affordances: vec!["shell".into()],
            logical_networks: BTreeMap::new(),
            isolation_trust: vec!["process-isolation".into()],
            hold_command: vec![
                "sh".into(),
                "-lc".into(),
                "while :; do sleep 3600; done".into(),
            ],
            shell_program: "sh".into(),
        })
    }

    pub fn with_affordance(mut self, affordance: impl Into<String>) -> Result<Self> {
        let affordance = affordance.into();
        if affordance.trim().is_empty() {
            return Err(WorkcellError::InvalidDemand(
                "Docker affordance must not be empty".into(),
            ));
        }
        if !self.affordances.iter().any(|item| item == &affordance) {
            self.affordances.push(affordance);
        }
        Ok(self)
    }

    pub fn with_logical_network(
        mut self,
        logical: impl Into<String>,
        docker_network: impl Into<String>,
    ) -> Result<Self> {
        let logical = logical.into();
        let docker_network = docker_network.into();
        if logical.trim().is_empty() || docker_network.trim().is_empty() {
            return Err(WorkcellError::InvalidDemand(
                "logical and Docker network names must not be empty".into(),
            ));
        }
        self.logical_networks.insert(logical, docker_network);
        Ok(self)
    }

    pub fn with_isolation_trust(mut self, requirement: impl Into<String>) -> Result<Self> {
        let requirement = requirement.into();
        if requirement.trim().is_empty() {
            return Err(WorkcellError::InvalidDemand(
                "Docker isolation/trust offer must not be empty".into(),
            ));
        }
        if !self
            .isolation_trust
            .iter()
            .any(|item| item == &requirement)
        {
            self.isolation_trust.push(requirement);
        }
        Ok(self)
    }

    pub fn with_hold_command(mut self, command: Vec<String>) -> Result<Self> {
        if command.is_empty() || command.iter().any(|part| part.trim().is_empty()) {
            return Err(WorkcellError::InvalidDemand(
                "Docker hold command must contain non-empty arguments".into(),
            ));
        }
        self.hold_command = command;
        Ok(self)
    }
}

#[derive(Clone, Debug)]
struct ExecutionRecord {
    container_id: String,
    container_name: String,
    engine_version: String,
    logical_networks: Vec<String>,
}

pub struct DockerExecutionProvider {
    provider_ref: ProviderRef,
    config: DockerExecutionConfig,
    runner: Arc<dyn DockerCommandRunner>,
    records: BTreeMap<String, ExecutionRecord>,
}

impl DockerExecutionProvider {
    pub fn new(provider_ref: ProviderRef, config: DockerExecutionConfig) -> Self {
        Self::with_runner(provider_ref, config, Arc::new(SystemDockerCommandRunner))
    }

    pub fn with_runner(
        provider_ref: ProviderRef,
        config: DockerExecutionConfig,
        runner: Arc<dyn DockerCommandRunner>,
    ) -> Self {
        Self {
            provider_ref,
            config,
            runner,
            records: BTreeMap::new(),
        }
    }

    fn record(&self, allocation: &ProviderAllocation) -> Result<&ExecutionRecord> {
        validate_allocation(self, allocation)?;
        self.records.get(&allocation.material_ref).ok_or_else(|| {
            WorkcellError::NotFound(format!(
                "Docker execution `{}` is not known by this provider",
                allocation.material_ref
            ))
        })
    }

    fn require_engine(&self) -> Result<String> {
        probe_engine(self.runner.as_ref()).map_err(|error| {
            WorkcellError::Unavailable(format!("Docker Engine is unavailable: {error}"))
        })
    }

    fn validate_request(&self, request: &ExecutionMaterialRequest) -> Result<Vec<String>> {
        for affordance in &request.affordances {
            if !self.config.affordances.iter().any(|item| item == affordance) {
                return Err(WorkcellError::UnsatisfiedDemand(format!(
                    "Docker execution provider does not offer affordance `{affordance}`"
                )));
            }
        }

        if let Some(isolation) = &request.isolation_trust {
            if !self
                .config
                .isolation_trust
                .iter()
                .any(|item| item == isolation.as_str())
            {
                return Err(WorkcellError::UnsatisfiedDemand(format!(
                    "Docker execution provider does not satisfy isolation/trust `{}`",
                    isolation.as_str()
                )));
            }
        }

        let mut networks = Vec::new();
        for connection in &request.connectivity {
            if connection.as_str() == "internet" {
                continue;
            }
            let network = self
                .config
                .logical_networks
                .get(connection.as_str())
                .ok_or_else(|| {
                    WorkcellError::UnsatisfiedDemand(format!(
                        "Docker execution provider has no material binding for logical connection `{}`",
                        connection.as_str()
                    ))
                })?;
            networks.push(network.clone());
        }
        Ok(networks)
    }

    fn resource_args(&self, request: &ExecutionMaterialRequest) -> Result<Vec<String>> {
        let mut args = Vec::new();
        for resource in &request.resources {
            match resource.key.as_str() {
                "memory" => {
                    let minimum = resource.minimum.ok_or_else(|| {
                        WorkcellError::InvalidDemand(
                            "memory resource requires a minimum value".into(),
                        )
                    })?;
                    args.push("--memory".into());
                    args.push(docker_memory_bytes(minimum, resource.unit.as_deref())?);
                }
                "cpu" | "cpus" => {
                    let minimum = resource.minimum.ok_or_else(|| {
                        WorkcellError::InvalidDemand(
                            "CPU resource requires a minimum value".into(),
                        )
                    })?;
                    args.push("--cpus".into());
                    args.push(minimum.to_string());
                }
                other => {
                    return Err(WorkcellError::UnsatisfiedDemand(format!(
                        "Docker execution provider cannot map resource `{other}`"
                    )))
                }
            }
        }
        Ok(args)
    }

    fn cleanup_container(&self, container_id: &str) {
        let _ = self.runner.run(&DockerCommand::new([
            "container",
            "rm",
            "--force",
            container_id,
        ]));
    }

    pub fn restart_execution(
        &self,
        allocation: &ProviderAllocation,
    ) -> Result<ProviderOperationResult> {
        let record = self.record(allocation)?;
        let output = self.runner.run(&DockerCommand::new([
            "container",
            "restart",
            record.container_id.as_str(),
        ]))?;
        let mut result = BTreeMap::new();
        if !output.stdout.trim().is_empty() {
            result.insert("stdout".into(), output.stdout.trim().into());
        }
        let mut provenance = allocation.provenance.clone();
        provenance.insert("container_id".into(), record.container_id.clone());
        Ok(ProviderOperationResult {
            provider_ref: self.provider_ref.clone(),
            material_ref: allocation.material_ref.clone(),
            operation: "restart".into(),
            output: result,
            provenance,
        })
    }
}

impl ProviderPort for DockerExecutionProvider {
    fn provider_ref(&self) -> &ProviderRef {
        &self.provider_ref
    }

    fn port_kind(&self) -> ProviderPortKind {
        ProviderPortKind::Execution
    }

    fn offers(&self) -> Result<Vec<OperationalOffer>> {
        let engine_version = match probe_engine(self.runner.as_ref()) {
            Ok(version) => version,
            Err(_) => return Ok(Vec::new()),
        };
        let mut connections = vec!["internet".into()];
        connections.extend(self.config.logical_networks.keys().cloned());
        let connections: Vec<String> = connections
            .into_iter()
            .collect::<BTreeSet<_>>()
            .into_iter()
            .collect();
        let mut metadata = provider_metadata(&engine_version, None);
        metadata.insert("image".into(), self.config.image.clone());

        Ok(vec![OperationalOffer {
            offer_ref: OfferRef::new(format!("offer:{}:docker-execution", self.provider_ref))
                .map_err(|error| WorkcellError::OperationFailed(error.into()))?,
            provider_ref: self.provider_ref.clone(),
            port: ProviderPortKind::Execution.as_str().into(),
            affordances: self.config.affordances.clone(),
            connections,
            exposures: Vec::new(),
            isolation_trust: self.config.isolation_trust.clone(),
            availability: Availability::Available,
            health: HealthState::Healthy,
            capacity: BTreeMap::new(),
            metadata,
        }])
    }
}

impl ExecutionProvider for DockerExecutionProvider {
    fn prepare_execution(
        &mut self,
        request: &ExecutionMaterialRequest,
    ) -> Result<ProviderAllocation> {
        let engine_version = self.require_engine()?;
        let networks = self.validate_request(request)?;
        let key = stable_key(&[request.demand_ref.as_str(), self.provider_ref.as_str()]);
        let container_name = format!("epilogos-wc-exec-{key}");

        let mut args = vec![
            "container".into(),
            "create".into(),
            "--name".into(),
            container_name.clone(),
            "--label".into(),
            format!("epilogos.workcell.demand={}", request.demand_ref),
        ];
        args.extend(self.resource_args(request)?);
        if let Some(first_network) = networks.first() {
            args.push("--network".into());
            args.push(first_network.clone());
        }
        args.push(self.config.image.clone());
        args.extend(self.config.hold_command.clone());

        let create = self.runner.run(&DockerCommand::new(args))?;
        let container_id = create.stdout.trim().to_owned();
        if container_id.is_empty() {
            return Err(WorkcellError::OperationFailed(
                "Docker container create returned an empty container id".into(),
            ));
        }

        if let Err(error) = self.runner.run(&DockerCommand::new([
            "container",
            "start",
            container_id.as_str(),
        ])) {
            self.cleanup_container(&container_id);
            return Err(error);
        }

        for network in networks.iter().skip(1) {
            if let Err(error) = self.runner.run(&DockerCommand::new([
                "network",
                "connect",
                network.as_str(),
                container_id.as_str(),
            ])) {
                self.cleanup_container(&container_id);
                return Err(error);
            }
        }

        let material_ref = format!("execution:docker:{container_id}");
        let logical_networks = request
            .connectivity
            .iter()
            .map(|connection| connection.as_str().to_owned())
            .collect::<Vec<_>>();
        let record = ExecutionRecord {
            container_id: container_id.clone(),
            container_name: container_name.clone(),
            engine_version: engine_version.clone(),
            logical_networks: logical_networks.clone(),
        };
        self.records.insert(material_ref.clone(), record);

        let mut properties = BTreeMap::new();
        properties.insert("container_id".into(), container_id.clone());
        properties.insert("container_name".into(), container_name);
        properties.insert("image".into(), self.config.image.clone());
        if !logical_networks.is_empty() {
            properties.insert("logical_connections".into(), logical_networks.join(","));
        }
        let mut provenance = provider_metadata(&engine_version, None);
        provenance.insert("container_id".into(), container_id);
        provenance.insert("image".into(), self.config.image.clone());

        Ok(ProviderAllocation {
            provider_ref: self.provider_ref.clone(),
            port: ProviderPortKind::Execution,
            material_ref,
            health: HealthState::Healthy,
            properties,
            provenance,
        })
    }

    fn execute_operation(
        &mut self,
        allocation: &ProviderAllocation,
        operation: &ProviderOperation,
    ) -> Result<ProviderOperationResult> {
        if operation.key == "restart" {
            return self.restart_execution(allocation);
        }
        if operation.key != "shell" {
            return Err(WorkcellError::Unsupported(format!(
                "Docker execution provider does not support operation `{}`",
                operation.key
            )));
        }
        let command = operation.parameters.get("command").ok_or_else(|| {
            WorkcellError::InvalidDemand("shell operation requires `command` parameter".into())
        })?;
        let record = self.record(allocation)?.clone();
        let output = self.runner.run(&DockerCommand::new([
            "container",
            "exec",
            record.container_id.as_str(),
            self.config.shell_program.as_str(),
            "-lc",
            command.as_str(),
        ]))?;

        let mut result = BTreeMap::new();
        result.insert("stdout".into(), output.stdout);
        if !output.stderr.is_empty() {
            result.insert("stderr".into(), output.stderr);
        }
        let mut provenance = allocation.provenance.clone();
        provenance.insert("container_id".into(), record.container_id);
        Ok(ProviderOperationResult {
            provider_ref: self.provider_ref.clone(),
            material_ref: allocation.material_ref.clone(),
            operation: operation.key.clone(),
            output: result,
            provenance,
        })
    }

    fn observe_execution(&self, allocation: &ProviderAllocation) -> Result<ProviderObservation> {
        let record = self.record(allocation)?;
        let output = self.runner.run(&DockerCommand::new([
            "container",
            "inspect",
            "--format",
            "{{.State.Status}}",
            record.container_id.as_str(),
        ]))?;
        let status = output.stdout.trim();
        let health = match status {
            "running" => HealthState::Healthy,
            "created" | "restarting" | "paused" => HealthState::Degraded,
            "exited" | "dead" | "removing" => HealthState::Unavailable,
            _ => HealthState::Unknown,
        };
        let mut detail = BTreeMap::new();
        detail.insert("status".into(), status.into());
        detail.insert("container_id".into(), record.container_id.clone());
        detail.insert("container_name".into(), record.container_name.clone());
        detail.insert("docker_engine".into(), record.engine_version.clone());
        if !record.logical_networks.is_empty() {
            detail.insert(
                "logical_connections".into(),
                record.logical_networks.join(","),
            );
        }
        Ok(ProviderObservation {
            provider_ref: self.provider_ref.clone(),
            material_ref: allocation.material_ref.clone(),
            health,
            detail,
        })
    }

    fn release_execution(
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
                self.runner
                    .run(&DockerCommand::new([
                        "container",
                        "rm",
                        "--force",
                        record.container_id.as_str(),
                    ]))
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
                self.runner.run(&DockerCommand::new([
                    "container",
                    "stop",
                    record.container_id.as_str(),
                ]))?;
                Ok(ProviderReleaseResult {
                    provider_ref: self.provider_ref.clone(),
                    material_ref: allocation.material_ref.clone(),
                    disposition: ReleaseDisposition::Suspended,
                    changed: true,
                })
            }
            RetentionExpectation::SnapshotIfSupported => Err(WorkcellError::Unsupported(
                "Docker execution provider does not claim snapshot semantics".into(),
            )),
        }
    }
}
