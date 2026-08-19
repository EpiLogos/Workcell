use std::{
    cell::RefCell,
    collections::BTreeMap,
    process::{Command, Output},
};

use epilogos_workcell_core::{
    Availability, HealthState, OfferRef, OperationalOffer, ProviderAllocation, ProviderObservation,
    ProviderPort, ProviderPortKind, ProviderRef, ProviderReleaseResult, ReleaseDisposition, Result,
    RetentionExpectation, ServiceMaterialRequest, ServiceProvider, WorkcellError,
};

use crate::support::stable_key;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ExternalServiceCommand {
    pub program: String,
    pub args: Vec<String>,
    pub environment: BTreeMap<String, String>,
}

impl ExternalServiceCommand {
    pub fn new(program: impl Into<String>) -> Result<Self> {
        let program = program.into();
        if program.trim().is_empty() {
            return Err(WorkcellError::InvalidDemand(
                "external service command program must not be empty".into(),
            ));
        }
        Ok(Self {
            program,
            args: Vec::new(),
            environment: BTreeMap::new(),
        })
    }

    pub fn with_arg(mut self, arg: impl Into<String>) -> Self {
        self.args.push(arg.into());
        self
    }

    pub fn with_env(mut self, key: impl Into<String>, value: impl Into<String>) -> Result<Self> {
        let key = key.into();
        if key.trim().is_empty() {
            return Err(WorkcellError::InvalidDemand(
                "external service command environment key must not be empty".into(),
            ));
        }
        self.environment.insert(key, value.into());
        Ok(self)
    }

    fn run(&self) -> std::io::Result<Output> {
        Command::new(&self.program)
            .args(&self.args)
            .envs(&self.environment)
            .output()
    }

    fn display(&self) -> String {
        std::iter::once(self.program.as_str())
            .chain(self.args.iter().map(String::as_str))
            .collect::<Vec<_>>()
            .join(" ")
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ExternalServiceAcquisition {
    /// Register/bind an existing installation but never start it implicitly.
    ObserveExisting,
    /// Start through the target-native command if the service is not healthy.
    EnsureRunning,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ExternalManagedService {
    pub logical_ref: String,
    pub endpoint: String,
    pub status: ExternalServiceCommand,
    pub readiness: Option<ExternalServiceCommand>,
    pub start: Option<ExternalServiceCommand>,
    pub stop: Option<ExternalServiceCommand>,
    pub restart: Option<ExternalServiceCommand>,
    pub acquisition: ExternalServiceAcquisition,
    pub metadata: BTreeMap<String, String>,
}

impl ExternalManagedService {
    pub fn new(
        logical_ref: impl Into<String>,
        endpoint: impl Into<String>,
        status: ExternalServiceCommand,
    ) -> Result<Self> {
        let logical_ref = logical_ref.into();
        let endpoint = endpoint.into();
        if logical_ref.trim().is_empty() {
            return Err(WorkcellError::InvalidDemand(
                "external managed service logical ref must not be empty".into(),
            ));
        }
        if endpoint.trim().is_empty() {
            return Err(WorkcellError::InvalidDemand(
                "external managed service endpoint must not be empty".into(),
            ));
        }
        Ok(Self {
            logical_ref,
            endpoint,
            status,
            readiness: None,
            start: None,
            stop: None,
            restart: None,
            acquisition: ExternalServiceAcquisition::ObserveExisting,
            metadata: BTreeMap::new(),
        })
    }

    pub fn with_readiness(mut self, command: ExternalServiceCommand) -> Self {
        self.readiness = Some(command);
        self
    }

    pub fn with_start(mut self, command: ExternalServiceCommand) -> Self {
        self.start = Some(command);
        self
    }

    pub fn with_stop(mut self, command: ExternalServiceCommand) -> Self {
        self.stop = Some(command);
        self
    }

    pub fn with_restart(mut self, command: ExternalServiceCommand) -> Self {
        self.restart = Some(command);
        self
    }

    pub fn with_acquisition(mut self, acquisition: ExternalServiceAcquisition) -> Self {
        self.acquisition = acquisition;
        self
    }

    pub fn with_metadata(mut self, key: impl Into<String>, value: impl Into<String>) -> Result<Self> {
        let key = key.into();
        if key.trim().is_empty() {
            return Err(WorkcellError::InvalidDemand(
                "external managed service metadata key must not be empty".into(),
            ));
        }
        self.metadata.insert(key, value.into());
        Ok(self)
    }
}

#[derive(Clone, Debug)]
struct ExternalServiceRecord {
    service: ExternalManagedService,
    started_by_provider: bool,
}

pub struct ExternalManagedServiceProvider {
    provider_ref: ProviderRef,
    services: BTreeMap<String, ExternalManagedService>,
    records: RefCell<BTreeMap<String, ExternalServiceRecord>>,
}

impl ExternalManagedServiceProvider {
    pub fn new(
        provider_ref: ProviderRef,
        services: impl IntoIterator<Item = ExternalManagedService>,
    ) -> Result<Self> {
        let mut by_ref = BTreeMap::new();
        for service in services {
            if by_ref
                .insert(service.logical_ref.clone(), service)
                .is_some()
            {
                return Err(WorkcellError::InvalidDemand(
                    "external managed service provider contains duplicate logical refs".into(),
                ));
            }
        }
        Ok(Self {
            provider_ref,
            services: by_ref,
            records: RefCell::new(BTreeMap::new()),
        })
    }

    pub fn restart_service(&self, allocation: &ProviderAllocation) -> Result<ProviderObservation> {
        let record = self.record(allocation)?;
        let restart = record.service.restart.as_ref().ok_or_else(|| {
            WorkcellError::Unsupported(format!(
                "logical service `{}` does not expose a target-native restart command",
                record.service.logical_ref
            ))
        })?;
        run_required(restart, "restart external service")?;
        self.observe_record(allocation, &record)
    }

    fn record(&self, allocation: &ProviderAllocation) -> Result<ExternalServiceRecord> {
        self.records
            .borrow()
            .get(&allocation.material_ref)
            .cloned()
            .ok_or_else(|| {
                WorkcellError::NotFound(format!(
                    "external service binding `{}` is not known by this provider",
                    allocation.material_ref
                ))
            })
    }

    fn observe_record(
        &self,
        allocation: &ProviderAllocation,
        record: &ExternalServiceRecord,
    ) -> Result<ProviderObservation> {
        let status = run_probe(&record.service.status);
        let readiness = record
            .service
            .readiness
            .as_ref()
            .map(run_probe)
            .unwrap_or_else(|| status.clone());
        let health = if status == HealthState::Unavailable || readiness == HealthState::Unavailable {
            HealthState::Unavailable
        } else if status == HealthState::Degraded || readiness == HealthState::Degraded {
            HealthState::Degraded
        } else {
            HealthState::Healthy
        };
        let mut detail = record.service.metadata.clone();
        detail.insert("logical_ref".into(), record.service.logical_ref.clone());
        detail.insert("endpoint".into(), record.service.endpoint.clone());
        detail.insert("status_command".into(), record.service.status.display());
        detail.insert("started_by_provider".into(), record.started_by_provider.to_string());
        if let Some(readiness) = &record.service.readiness {
            detail.insert("readiness_command".into(), readiness.display());
        }
        Ok(ProviderObservation {
            provider_ref: self.provider_ref.clone(),
            material_ref: allocation.material_ref.clone(),
            health,
            detail,
        })
    }
}

impl ProviderPort for ExternalManagedServiceProvider {
    fn provider_ref(&self) -> &ProviderRef {
        &self.provider_ref
    }

    fn port_kind(&self) -> ProviderPortKind {
        ProviderPortKind::Service
    }

    fn offers(&self) -> Result<Vec<OperationalOffer>> {
        self.services
            .values()
            .map(|service| {
                let health = run_probe(&service.status);
                let availability = match health {
                    HealthState::Healthy => Availability::Available,
                    HealthState::Degraded | HealthState::Unknown => Availability::Degraded,
                    HealthState::Unavailable => {
                        if service.acquisition == ExternalServiceAcquisition::EnsureRunning
                            && service.start.is_some()
                        {
                            Availability::Degraded
                        } else {
                            Availability::Unavailable
                        }
                    }
                };
                let mut metadata = service.metadata.clone();
                metadata.insert("implementation".into(), "external-managed-service".into());
                metadata.insert("logical_ref".into(), service.logical_ref.clone());
                metadata.insert("configuration_owner".into(), "target".into());
                Ok(OperationalOffer {
                    offer_ref: OfferRef::new(format!(
                        "offer:{}:external-service:{}",
                        self.provider_ref,
                        stable_key(&[&service.logical_ref])
                    ))
                    .map_err(|error| WorkcellError::OperationFailed(error.into()))?,
                    provider_ref: self.provider_ref.clone(),
                    port: ProviderPortKind::Service.as_str().into(),
                    affordances: vec![],
                    connections: vec![service.logical_ref.clone()],
                    exposures: vec![],
                    isolation_trust: vec![],
                    availability,
                    health,
                    capacity: BTreeMap::new(),
                    metadata,
                })
            })
            .collect()
    }
}

impl ServiceProvider for ExternalManagedServiceProvider {
    fn resolve_service(&mut self, request: &ServiceMaterialRequest) -> Result<ProviderAllocation> {
        let logical_ref = request.connection.as_str();
        let service = self.services.get(logical_ref).cloned().ok_or_else(|| {
            WorkcellError::UnsatisfiedDemand(format!(
                "logical service `{logical_ref}` is not offered"
            ))
        })?;

        let mut started_by_provider = false;
        if run_probe(&service.status) == HealthState::Unavailable {
            match service.acquisition {
                ExternalServiceAcquisition::ObserveExisting => {
                    return Err(WorkcellError::Unavailable(format!(
                        "target-owned logical service `{logical_ref}` is not running"
                    )))
                }
                ExternalServiceAcquisition::EnsureRunning => {
                    let start = service.start.as_ref().ok_or_else(|| {
                        WorkcellError::Unsupported(format!(
                            "logical service `{logical_ref}` cannot be ensured running without a target-native start command"
                        ))
                    })?;
                    run_required(start, "start external service")?;
                    started_by_provider = true;
                    if run_probe(&service.status) == HealthState::Unavailable {
                        return Err(WorkcellError::Unavailable(format!(
                            "logical service `{logical_ref}` remained unavailable after target-native start"
                        )));
                    }
                }
            }
        }

        if let Some(readiness) = &service.readiness {
            if run_probe(readiness) == HealthState::Unavailable {
                return Err(WorkcellError::Unavailable(format!(
                    "logical service `{logical_ref}` is running but not ready"
                )));
            }
        }

        let material_ref = format!(
            "service:external:{}",
            stable_key(&[request.demand_ref.as_str(), logical_ref, &service.endpoint])
        );
        let mut properties = BTreeMap::new();
        properties.insert("logical_ref".into(), logical_ref.into());
        properties.insert("endpoint".into(), service.endpoint.clone());
        properties.insert("configuration_owner".into(), "target".into());
        properties.insert("started_by_provider".into(), started_by_provider.to_string());
        let mut provenance = service.metadata.clone();
        provenance.insert("implementation".into(), "external-managed-service".into());
        provenance.insert("status_command".into(), service.status.display());

        self.records.borrow_mut().insert(
            material_ref.clone(),
            ExternalServiceRecord {
                service: service.clone(),
                started_by_provider,
            },
        );
        Ok(ProviderAllocation {
            provider_ref: self.provider_ref.clone(),
            port: ProviderPortKind::Service,
            material_ref,
            health: HealthState::Healthy,
            properties,
            provenance,
        })
    }

    fn observe_service(&self, allocation: &ProviderAllocation) -> Result<ProviderObservation> {
        let record = self.record(allocation)?;
        self.observe_record(allocation, &record)
    }

    fn release_service(
        &mut self,
        allocation: &ProviderAllocation,
        retention: &RetentionExpectation,
    ) -> Result<ProviderReleaseResult> {
        let record = self.record(allocation)?;
        match retention {
            RetentionExpectation::Preserve => Ok(ProviderReleaseResult {
                provider_ref: self.provider_ref.clone(),
                material_ref: allocation.material_ref.clone(),
                disposition: ReleaseDisposition::Preserved,
                changed: false,
            }),
            _ => {
                let mut changed = false;
                if record.started_by_provider {
                    if let Some(stop) = &record.service.stop {
                        run_required(stop, "stop external service")?;
                        changed = true;
                    }
                }
                self.records.borrow_mut().remove(&allocation.material_ref);
                Ok(ProviderReleaseResult {
                    provider_ref: self.provider_ref.clone(),
                    material_ref: allocation.material_ref.clone(),
                    disposition: ReleaseDisposition::Released,
                    changed,
                })
            }
        }
    }
}

fn run_probe(command: &ExternalServiceCommand) -> HealthState {
    match command.run() {
        Ok(output) if output.status.success() => HealthState::Healthy,
        Ok(_) => HealthState::Unavailable,
        Err(_) => HealthState::Unavailable,
    }
}

fn run_required(command: &ExternalServiceCommand, action: &str) -> Result<()> {
    let output = command.run().map_err(|error| {
        WorkcellError::Unavailable(format!(
            "{action} via `{}` failed to execute: {error}",
            command.display()
        ))
    })?;
    if output.status.success() {
        return Ok(());
    }
    let stderr = String::from_utf8_lossy(&output.stderr).trim().to_owned();
    Err(WorkcellError::OperationFailed(format!(
        "{action} via `{}` exited {}{}",
        command.display(),
        output.status,
        if stderr.is_empty() {
            String::new()
        } else {
            format!(": {stderr}")
        }
    )))
}
