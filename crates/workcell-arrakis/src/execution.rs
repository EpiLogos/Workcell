use std::{
    collections::{BTreeMap, BTreeSet},
    path::PathBuf,
    sync::Arc,
};

use epilogos_workcell_core::{
    validate_allocation, Availability, ExecutionMaterialRequest, ExecutionProvider, HealthState,
    OfferRef, OperationalOffer, ProviderAllocation, ProviderObservation, ProviderOperation,
    ProviderOperationResult, ProviderPort, ProviderPortKind, ProviderRef, ProviderReleaseResult,
    ReleaseDisposition, Result, RetentionExpectation, WorkcellError,
};

use crate::{
    source_metadata, stable_key, status_health, ArrakisCommand, ArrakisCommandRunner,
    ArrakisHostProbe, SystemArrakisCommandRunner, SystemArrakisHostProbe,
};

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ArrakisExecutionConfig {
    pub client_path: PathBuf,
    pub client_config: Option<PathBuf>,
    pub kernel: Option<PathBuf>,
    pub rootfs: Option<PathBuf>,
    pub entry_point: Option<String>,
    pub require_local_kvm: bool,
    pub affordances: Vec<String>,
    pub connections: Vec<String>,
    pub isolation_trust: Vec<String>,
}

impl ArrakisExecutionConfig {
    pub fn new(client_path: impl Into<PathBuf>) -> Result<Self> {
        let client_path = client_path.into();
        if client_path.as_os_str().is_empty() {
            return Err(WorkcellError::InvalidDemand(
                "Arrakis client path must not be empty".into(),
            ));
        }
        Ok(Self {
            client_path,
            client_config: None,
            kernel: None,
            rootfs: None,
            entry_point: None,
            require_local_kvm: false,
            affordances: vec!["shell".into(), "snapshot".into(), "restore".into()],
            connections: vec!["internet".into()],
            isolation_trust: vec!["microvm-isolation".into(), "strong-isolation".into()],
        })
    }

    pub fn with_client_config(mut self, path: impl Into<PathBuf>) -> Result<Self> {
        let path = path.into();
        if path.as_os_str().is_empty() {
            return Err(WorkcellError::InvalidDemand(
                "Arrakis client config path must not be empty".into(),
            ));
        }
        self.client_config = Some(path);
        Ok(self)
    }

    pub fn with_kernel(mut self, path: impl Into<PathBuf>) -> Result<Self> {
        let path = path.into();
        if path.as_os_str().is_empty() {
            return Err(WorkcellError::InvalidDemand(
                "Arrakis kernel path must not be empty".into(),
            ));
        }
        self.kernel = Some(path);
        Ok(self)
    }

    pub fn with_rootfs(mut self, path: impl Into<PathBuf>) -> Result<Self> {
        let path = path.into();
        if path.as_os_str().is_empty() {
            return Err(WorkcellError::InvalidDemand(
                "Arrakis rootfs path must not be empty".into(),
            ));
        }
        self.rootfs = Some(path);
        Ok(self)
    }

    pub fn with_entry_point(mut self, entry_point: impl Into<String>) -> Result<Self> {
        let entry_point = entry_point.into();
        if entry_point.trim().is_empty() {
            return Err(WorkcellError::InvalidDemand(
                "Arrakis entry point must not be empty".into(),
            ));
        }
        self.entry_point = Some(entry_point);
        Ok(self)
    }

    pub fn require_local_kvm(mut self, required: bool) -> Self {
        self.require_local_kvm = required;
        self
    }

    pub fn with_affordance(mut self, affordance: impl Into<String>) -> Result<Self> {
        let affordance = affordance.into();
        if affordance.trim().is_empty() {
            return Err(WorkcellError::InvalidDemand(
                "Arrakis affordance must not be empty".into(),
            ));
        }
        if !self.affordances.iter().any(|item| item == &affordance) {
            self.affordances.push(affordance);
        }
        Ok(self)
    }

    pub fn with_connection(mut self, connection: impl Into<String>) -> Result<Self> {
        let connection = connection.into();
        if connection.trim().is_empty() {
            return Err(WorkcellError::InvalidDemand(
                "Arrakis logical connection must not be empty".into(),
            ));
        }
        if !self.connections.iter().any(|item| item == &connection) {
            self.connections.push(connection);
        }
        Ok(self)
    }
}

pub struct ArrakisExecutionProvider {
    provider_ref: ProviderRef,
    config: ArrakisExecutionConfig,
    runner: Arc<dyn ArrakisCommandRunner>,
    host_probe: Arc<dyn ArrakisHostProbe>,
}

impl ArrakisExecutionProvider {
    pub fn new(provider_ref: ProviderRef, config: ArrakisExecutionConfig) -> Self {
        Self::with_adapters(
            provider_ref,
            config,
            Arc::new(SystemArrakisCommandRunner),
            Arc::new(SystemArrakisHostProbe),
        )
    }

    pub fn with_adapters(
        provider_ref: ProviderRef,
        config: ArrakisExecutionConfig,
        runner: Arc<dyn ArrakisCommandRunner>,
        host_probe: Arc<dyn ArrakisHostProbe>,
    ) -> Self {
        Self {
            provider_ref,
            config,
            runner,
            host_probe,
        }
    }

    fn command(&self, args: impl IntoIterator<Item = impl Into<String>>) -> ArrakisCommand {
        let mut command_args = Vec::new();
        if let Some(config) = &self.config.client_config {
            command_args.push("--config".into());
            command_args.push(config.to_string_lossy().into_owned());
        }
        command_args.extend(args.into_iter().map(Into::into));
        ArrakisCommand::new(self.config.client_path.clone(), command_args)
    }

    fn require_available(&self) -> Result<()> {
        if self.config.require_local_kvm && !self.host_probe.local_kvm_available() {
            return Err(WorkcellError::Unavailable(
                "Arrakis local MicroVM execution requires Linux KVM at `/dev/kvm`".into(),
            ));
        }
        self.runner
            .run(&self.command(["list-all"]))
            .map(|_| ())
            .map_err(|error| {
                WorkcellError::Unavailable(format!(
                    "Arrakis control service/client is unavailable: {error}"
                ))
            })
    }

    fn validate_request(&self, request: &ExecutionMaterialRequest) -> Result<()> {
        for affordance in &request.affordances {
            if !self
                .config
                .affordances
                .iter()
                .any(|item| item == affordance)
            {
                return Err(WorkcellError::UnsatisfiedDemand(format!(
                    "Arrakis execution provider does not offer affordance `{affordance}`"
                )));
            }
        }
        for connection in &request.connectivity {
            if !self
                .config
                .connections
                .iter()
                .any(|item| item == connection.as_str())
            {
                return Err(WorkcellError::UnsatisfiedDemand(format!(
                    "Arrakis execution provider does not materialise logical connection `{}`",
                    connection.as_str()
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
                    "Arrakis execution provider does not satisfy isolation/trust `{}`",
                    isolation.as_str()
                )));
            }
        }
        if !request.resources.is_empty() {
            return Err(WorkcellError::UnsatisfiedDemand(
                "pinned Arrakis API does not expose per-VM resource sizing through the inspected start contract"
                    .into(),
            ));
        }
        Ok(())
    }

    fn vm_name(&self, allocation: &ProviderAllocation) -> Result<String> {
        validate_allocation(self, allocation)?;
        allocation
            .properties
            .get("vm_name")
            .or_else(|| allocation.provenance.get("arrakis_vm_name"))
            .cloned()
            .ok_or_else(|| {
                WorkcellError::OperationFailed(format!(
                    "Arrakis allocation `{}` cannot be recovered: vm_name provenance is missing",
                    allocation.material_ref
                ))
            })
    }

    fn snapshot_id(&self, allocation: &ProviderAllocation, purpose: &str) -> String {
        let key = stable_key(&[
            allocation.material_ref.as_str(),
            self.provider_ref.as_str(),
            purpose,
        ]);
        format!("epilogos-wc-{purpose}-{key}")
    }

    pub fn snapshot_execution(
        &self,
        allocation: &ProviderAllocation,
        snapshot_id: Option<&str>,
    ) -> Result<ProviderOperationResult> {
        let vm_name = self.vm_name(allocation)?;
        let snapshot_id = snapshot_id
            .map(str::to_owned)
            .unwrap_or_else(|| self.snapshot_id(allocation, "snapshot"));
        if snapshot_id.trim().is_empty() {
            return Err(WorkcellError::InvalidDemand(
                "Arrakis snapshot id must not be empty".into(),
            ));
        }
        let output = self.runner.run(&self.command([
            "snapshot",
            "--name",
            vm_name.as_str(),
            "--id",
            snapshot_id.as_str(),
        ]))?;
        let mut result = BTreeMap::new();
        result.insert("snapshot_id".into(), snapshot_id.clone());
        if !output.stdout.trim().is_empty() {
            result.insert("stdout".into(), output.stdout.trim().into());
        }
        if !output.stderr.trim().is_empty() {
            result.insert("stderr".into(), output.stderr.trim().into());
        }
        let mut provenance = allocation.provenance.clone();
        provenance.extend(source_metadata());
        provenance.insert("arrakis_vm_name".into(), vm_name);
        provenance.insert("arrakis_snapshot_id".into(), snapshot_id);
        Ok(ProviderOperationResult {
            provider_ref: self.provider_ref.clone(),
            material_ref: allocation.material_ref.clone(),
            operation: "snapshot".into(),
            output: result,
            provenance,
        })
    }

    pub fn restore_execution(
        &self,
        allocation: &ProviderAllocation,
        snapshot_id: &str,
    ) -> Result<ProviderOperationResult> {
        if snapshot_id.trim().is_empty() {
            return Err(WorkcellError::InvalidDemand(
                "Arrakis restore requires a snapshot id".into(),
            ));
        }
        let vm_name = self.vm_name(allocation)?;
        let output = self.runner.run(&self.command([
            "restore",
            "--name",
            vm_name.as_str(),
            "--id",
            snapshot_id,
        ]))?;
        let mut result = BTreeMap::new();
        result.insert("snapshot_id".into(), snapshot_id.into());
        if !output.stdout.trim().is_empty() {
            result.insert("stdout".into(), output.stdout.trim().into());
        }
        if !output.stderr.trim().is_empty() {
            result.insert("stderr".into(), output.stderr.trim().into());
        }
        let mut provenance = allocation.provenance.clone();
        provenance.extend(source_metadata());
        provenance.insert("arrakis_vm_name".into(), vm_name);
        provenance.insert("arrakis_restored_snapshot_id".into(), snapshot_id.into());
        Ok(ProviderOperationResult {
            provider_ref: self.provider_ref.clone(),
            material_ref: allocation.material_ref.clone(),
            operation: "restore".into(),
            output: result,
            provenance,
        })
    }
}

impl ProviderPort for ArrakisExecutionProvider {
    fn provider_ref(&self) -> &ProviderRef {
        &self.provider_ref
    }

    fn port_kind(&self) -> ProviderPortKind {
        ProviderPortKind::Execution
    }

    fn offers(&self) -> Result<Vec<OperationalOffer>> {
        if self.config.require_local_kvm && !self.host_probe.local_kvm_available() {
            return Ok(Vec::new());
        }
        if self.runner.run(&self.command(["list-all"])).is_err() {
            return Ok(Vec::new());
        }
        let mut metadata = source_metadata();
        metadata.insert(
            "local_kvm_required".into(),
            self.config.require_local_kvm.to_string(),
        );
        let connections = self
            .config
            .connections
            .iter()
            .cloned()
            .collect::<BTreeSet<_>>()
            .into_iter()
            .collect();
        Ok(vec![OperationalOffer {
            offer_ref: OfferRef::new(format!("offer:{}:arrakis-execution", self.provider_ref))
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

impl ExecutionProvider for ArrakisExecutionProvider {
    fn prepare_execution(
        &mut self,
        request: &ExecutionMaterialRequest,
    ) -> Result<ProviderAllocation> {
        self.require_available()?;
        self.validate_request(request)?;
        let key = stable_key(&[request.demand_ref.as_str(), self.provider_ref.as_str()]);
        let vm_name = format!("epilogos-wc-{key}");
        let mut args = vec!["start".into(), "--name".into(), vm_name.clone()];
        if let Some(kernel) = &self.config.kernel {
            args.push("--kernel".into());
            args.push(kernel.to_string_lossy().into_owned());
        }
        if let Some(rootfs) = &self.config.rootfs {
            args.push("--rootfs".into());
            args.push(rootfs.to_string_lossy().into_owned());
        }
        if let Some(entry_point) = &self.config.entry_point {
            args.push("--entry-point".into());
            args.push(entry_point.clone());
        }
        let started = self.runner.run(&self.command(args))?;
        let material_ref = format!("execution:arrakis:{vm_name}");
        let mut properties = BTreeMap::new();
        properties.insert("vm_name".into(), vm_name.clone());
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
        if !started.stdout.trim().is_empty() {
            properties.insert("start_stdout".into(), started.stdout.trim().into());
        }
        let mut provenance = source_metadata();
        provenance.insert("arrakis_vm_name".into(), vm_name);
        if !started.stderr.trim().is_empty() {
            provenance.insert("start_stderr".into(), started.stderr.trim().into());
        }
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
        match operation.key.as_str() {
            "shell" => {
                let vm_name = self.vm_name(allocation)?;
                let command = operation.parameters.get("command").ok_or_else(|| {
                    WorkcellError::InvalidDemand(
                        "Arrakis shell operation requires `command` parameter".into(),
                    )
                })?;
                let output = self.runner.run(&self.command([
                    "run",
                    "--name",
                    vm_name.as_str(),
                    "--cmd",
                    command.as_str(),
                ]))?;
                let mut result = BTreeMap::new();
                result.insert("stdout".into(), output.stdout);
                if !output.stderr.is_empty() {
                    result.insert("stderr".into(), output.stderr);
                }
                let mut provenance = allocation.provenance.clone();
                provenance.extend(source_metadata());
                provenance.insert("arrakis_vm_name".into(), vm_name);
                Ok(ProviderOperationResult {
                    provider_ref: self.provider_ref.clone(),
                    material_ref: allocation.material_ref.clone(),
                    operation: "shell".into(),
                    output: result,
                    provenance,
                })
            }
            "snapshot" => self.snapshot_execution(
                allocation,
                operation.parameters.get("snapshot_id").map(String::as_str),
            ),
            "restore" => {
                let snapshot_id = operation.parameters.get("snapshot_id").ok_or_else(|| {
                    WorkcellError::InvalidDemand(
                        "Arrakis restore operation requires `snapshot_id` parameter".into(),
                    )
                })?;
                self.restore_execution(allocation, snapshot_id)
            }
            "pause" | "resume" => {
                let vm_name = self.vm_name(allocation)?;
                let output = self.runner.run(&self.command([
                    operation.key.as_str(),
                    "--name",
                    vm_name.as_str(),
                ]))?;
                let mut result = BTreeMap::new();
                if !output.stdout.trim().is_empty() {
                    result.insert("stdout".into(), output.stdout.trim().into());
                }
                if !output.stderr.trim().is_empty() {
                    result.insert("stderr".into(), output.stderr.trim().into());
                }
                let mut provenance = allocation.provenance.clone();
                provenance.extend(source_metadata());
                provenance.insert("arrakis_vm_name".into(), vm_name);
                Ok(ProviderOperationResult {
                    provider_ref: self.provider_ref.clone(),
                    material_ref: allocation.material_ref.clone(),
                    operation: operation.key.clone(),
                    output: result,
                    provenance,
                })
            }
            other => Err(WorkcellError::Unsupported(format!(
                "Arrakis execution provider does not support operation `{other}`"
            ))),
        }
    }

    fn observe_execution(&self, allocation: &ProviderAllocation) -> Result<ProviderObservation> {
        let vm_name = self.vm_name(allocation)?;
        let output = self
            .runner
            .run(&self.command(["list", "--name", vm_name.as_str()]))?;
        let health = status_health(&format!("{}\n{}", output.stdout, output.stderr))?;
        let mut detail = BTreeMap::new();
        detail.insert("vm_name".into(), vm_name);
        detail.insert("status_output".into(), output.stdout.trim().into());
        detail.extend(source_metadata());
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
        let vm_name = self.vm_name(allocation)?;
        match retention {
            RetentionExpectation::Preserve => Ok(ProviderReleaseResult {
                provider_ref: self.provider_ref.clone(),
                material_ref: allocation.material_ref.clone(),
                disposition: ReleaseDisposition::Preserved,
                changed: false,
            }),
            RetentionExpectation::Release => {
                self.runner
                    .run(&self.command(["destroy", "--name", vm_name.as_str()]))?;
                Ok(ProviderReleaseResult {
                    provider_ref: self.provider_ref.clone(),
                    material_ref: allocation.material_ref.clone(),
                    disposition: ReleaseDisposition::Released,
                    changed: true,
                })
            }
            RetentionExpectation::SuspendIfSupported => {
                self.runner
                    .run(&self.command(["pause", "--name", vm_name.as_str()]))?;
                Ok(ProviderReleaseResult {
                    provider_ref: self.provider_ref.clone(),
                    material_ref: allocation.material_ref.clone(),
                    disposition: ReleaseDisposition::Suspended,
                    changed: true,
                })
            }
            RetentionExpectation::SnapshotIfSupported => {
                let snapshot_id = self.snapshot_id(allocation, "retention");
                self.snapshot_execution(allocation, Some(&snapshot_id))?;
                self.runner
                    .run(&self.command(["destroy", "--name", vm_name.as_str()]))?;
                Ok(ProviderReleaseResult {
                    provider_ref: self.provider_ref.clone(),
                    material_ref: allocation.material_ref.clone(),
                    disposition: ReleaseDisposition::Snapshotted,
                    changed: true,
                })
            }
        }
    }
}
