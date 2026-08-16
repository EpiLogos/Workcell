use std::{
    cell::RefCell,
    collections::BTreeMap,
    env,
    net::{TcpStream, ToSocketAddrs},
    path::{Path, PathBuf},
    process::{Child, Command, Stdio},
    thread,
    time::{Duration, Instant},
};

use epilogos_workcell_core::{
    Availability, HealthState, OfferRef, OperationalOffer, ProviderAllocation, ProviderObservation,
    ProviderPort, ProviderPortKind, ProviderRef, ProviderReleaseResult, ReleaseDisposition, Result,
    RetentionExpectation, ServiceMaterialRequest, ServiceProvider, WorkcellError,
};

use crate::support::stable_key;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct StaticService {
    pub logical_ref: String,
    pub endpoint: String,
    pub health: HealthState,
    pub metadata: BTreeMap<String, String>,
}

impl StaticService {
    pub fn new(logical_ref: impl Into<String>, endpoint: impl Into<String>) -> Result<Self> {
        let logical_ref = logical_ref.into();
        let endpoint = endpoint.into();
        if logical_ref.trim().is_empty() {
            return Err(WorkcellError::InvalidDemand(
                "static service logical ref must not be empty".into(),
            ));
        }
        if endpoint.trim().is_empty() {
            return Err(WorkcellError::InvalidDemand(
                "static service endpoint must not be empty".into(),
            ));
        }
        Ok(Self {
            logical_ref,
            endpoint,
            health: HealthState::Healthy,
            metadata: BTreeMap::new(),
        })
    }

    pub fn with_health(mut self, health: HealthState) -> Self {
        self.health = health;
        self
    }
}

pub struct StaticServiceProvider {
    provider_ref: ProviderRef,
    services: BTreeMap<String, StaticService>,
    records: BTreeMap<String, StaticService>,
}

impl StaticServiceProvider {
    pub fn new(
        provider_ref: ProviderRef,
        services: impl IntoIterator<Item = StaticService>,
    ) -> Result<Self> {
        let mut by_ref = BTreeMap::new();
        for service in services {
            if by_ref
                .insert(service.logical_ref.clone(), service)
                .is_some()
            {
                return Err(WorkcellError::InvalidDemand(
                    "static service provider contains duplicate logical refs".into(),
                ));
            }
        }
        Ok(Self {
            provider_ref,
            services: by_ref,
            records: BTreeMap::new(),
        })
    }

    fn record(&self, allocation: &ProviderAllocation) -> Result<&StaticService> {
        self.records.get(&allocation.material_ref).ok_or_else(|| {
            WorkcellError::NotFound(format!(
                "service binding `{}` is not known by this provider",
                allocation.material_ref
            ))
        })
    }
}

impl ProviderPort for StaticServiceProvider {
    fn provider_ref(&self) -> &ProviderRef {
        &self.provider_ref
    }

    fn port_kind(&self) -> ProviderPortKind {
        ProviderPortKind::Service
    }

    fn offers(&self) -> Result<Vec<OperationalOffer>> {
        let mut offers = Vec::with_capacity(self.services.len());
        for service in self.services.values() {
            let availability = match service.health {
                HealthState::Healthy => Availability::Available,
                HealthState::Degraded | HealthState::Unknown => Availability::Degraded,
                HealthState::Unavailable => Availability::Unavailable,
            };
            let mut metadata = service.metadata.clone();
            metadata.insert("implementation".into(), "static-service".into());
            metadata.insert("logical_ref".into(), service.logical_ref.clone());
            offers.push(OperationalOffer {
                offer_ref: OfferRef::new(format!(
                    "offer:{}:service:{}",
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
                health: service.health.clone(),
                capacity: BTreeMap::new(),
                metadata,
            });
        }
        Ok(offers)
    }
}

impl ServiceProvider for StaticServiceProvider {
    fn resolve_service(&mut self, request: &ServiceMaterialRequest) -> Result<ProviderAllocation> {
        let logical_ref = request.connection.as_str();
        let service = self.services.get(logical_ref).cloned().ok_or_else(|| {
            WorkcellError::UnsatisfiedDemand(format!(
                "logical service `{logical_ref}` is not offered"
            ))
        })?;
        if service.health == HealthState::Unavailable {
            return Err(WorkcellError::Unavailable(format!(
                "logical service `{logical_ref}` is unavailable"
            )));
        }

        let key = stable_key(&[request.demand_ref.as_str(), logical_ref, &service.endpoint]);
        let material_ref = format!("service:static:{key}");
        let mut properties = BTreeMap::new();
        properties.insert("logical_ref".into(), logical_ref.into());
        properties.insert("endpoint".into(), service.endpoint.clone());
        let mut provenance = BTreeMap::new();
        provenance.insert("implementation".into(), "static-service".into());
        provenance.insert("logical_ref".into(), logical_ref.into());

        self.records.insert(material_ref.clone(), service.clone());
        Ok(ProviderAllocation {
            provider_ref: self.provider_ref.clone(),
            port: ProviderPortKind::Service,
            material_ref,
            health: service.health,
            properties,
            provenance,
        })
    }

    fn observe_service(&self, allocation: &ProviderAllocation) -> Result<ProviderObservation> {
        let service = self.record(allocation)?;
        let mut detail = BTreeMap::new();
        detail.insert("logical_ref".into(), service.logical_ref.clone());
        detail.insert("endpoint".into(), service.endpoint.clone());
        Ok(ProviderObservation {
            provider_ref: self.provider_ref.clone(),
            material_ref: allocation.material_ref.clone(),
            health: service.health.clone(),
            detail,
        })
    }

    fn release_service(
        &mut self,
        allocation: &ProviderAllocation,
        retention: &RetentionExpectation,
    ) -> Result<ProviderReleaseResult> {
        self.record(allocation)?;
        match retention {
            RetentionExpectation::Preserve => Ok(ProviderReleaseResult {
                provider_ref: self.provider_ref.clone(),
                material_ref: allocation.material_ref.clone(),
                disposition: ReleaseDisposition::Preserved,
                changed: false,
            }),
            _ => {
                self.records.remove(&allocation.material_ref);
                Ok(ProviderReleaseResult {
                    provider_ref: self.provider_ref.clone(),
                    material_ref: allocation.material_ref.clone(),
                    disposition: ReleaseDisposition::Released,
                    changed: false,
                })
            }
        }
    }
}

/// A provider-neutral material description of a long-running host service.
///
/// The logical service identity remains caller-owned. `program`, arguments,
/// environment, process id, endpoint and upstream revision are provider/material
/// facts and are retained only as allocation properties/provenance.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ManagedHostService {
    pub logical_ref: String,
    pub endpoint: String,
    pub program: String,
    pub args: Vec<String>,
    pub env: BTreeMap<String, String>,
    pub cwd: Option<PathBuf>,
    pub metadata: BTreeMap<String, String>,
    pub readiness: Option<TcpEndpointProbe>,
}

impl ManagedHostService {
    pub fn new(
        logical_ref: impl Into<String>,
        endpoint: impl Into<String>,
        program: impl Into<String>,
    ) -> Result<Self> {
        let logical_ref = logical_ref.into();
        let endpoint = endpoint.into();
        let program = program.into();
        if logical_ref.trim().is_empty() {
            return Err(WorkcellError::InvalidDemand(
                "managed service logical ref must not be empty".into(),
            ));
        }
        if endpoint.trim().is_empty() {
            return Err(WorkcellError::InvalidDemand(
                "managed service endpoint must not be empty".into(),
            ));
        }
        if program.trim().is_empty() {
            return Err(WorkcellError::InvalidDemand(
                "managed service program must not be empty".into(),
            ));
        }
        Ok(Self {
            logical_ref,
            endpoint,
            program,
            args: Vec::new(),
            env: BTreeMap::new(),
            cwd: None,
            metadata: BTreeMap::new(),
            readiness: None,
        })
    }

    pub fn with_arg(mut self, arg: impl Into<String>) -> Self {
        self.args.push(arg.into());
        self
    }

    pub fn with_env(mut self, key: impl Into<String>, value: impl Into<String>) -> Self {
        self.env.insert(key.into(), value.into());
        self
    }

    pub fn with_cwd(mut self, cwd: impl Into<PathBuf>) -> Self {
        self.cwd = Some(cwd.into());
        self
    }

    pub fn with_metadata(mut self, key: impl Into<String>, value: impl Into<String>) -> Self {
        self.metadata.insert(key.into(), value.into());
        self
    }

    pub fn with_tcp_readiness(mut self, readiness: TcpEndpointProbe) -> Self {
        self.readiness = Some(readiness);
        self
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TcpEndpointProbe {
    pub host: String,
    pub port: u16,
    pub timeout_ms: u64,
    pub interval_ms: u64,
}

impl TcpEndpointProbe {
    pub fn new(host: impl Into<String>, port: u16) -> Result<Self> {
        let host = host.into();
        if host.trim().is_empty() {
            return Err(WorkcellError::InvalidDemand(
                "TCP readiness host must not be empty".into(),
            ));
        }
        if port == 0 {
            return Err(WorkcellError::InvalidDemand(
                "TCP readiness port must be non-zero".into(),
            ));
        }
        Ok(Self {
            host,
            port,
            timeout_ms: 10_000,
            interval_ms: 50,
        })
    }

    pub fn with_timeout_ms(mut self, timeout_ms: u64) -> Self {
        self.timeout_ms = timeout_ms.max(1);
        self
    }

    pub fn with_interval_ms(mut self, interval_ms: u64) -> Self {
        self.interval_ms = interval_ms.max(1);
        self
    }

    fn reachable(&self) -> bool {
        let Ok(mut addresses) = (self.host.as_str(), self.port).to_socket_addrs() else {
            return false;
        };
        let Some(address) = addresses.next() else {
            return false;
        };
        TcpStream::connect_timeout(&address, Duration::from_millis(250)).is_ok()
    }
}

struct ManagedServiceRecord {
    service: ManagedHostService,
    child: RefCell<Child>,
}

pub struct ManagedHostServiceProvider {
    provider_ref: ProviderRef,
    services: BTreeMap<String, ManagedHostService>,
    records: BTreeMap<String, ManagedServiceRecord>,
    next_instance: u64,
}

impl ManagedHostServiceProvider {
    pub fn new(
        provider_ref: ProviderRef,
        services: impl IntoIterator<Item = ManagedHostService>,
    ) -> Result<Self> {
        let mut by_ref = BTreeMap::new();
        for service in services {
            if by_ref
                .insert(service.logical_ref.clone(), service)
                .is_some()
            {
                return Err(WorkcellError::InvalidDemand(
                    "managed service provider contains duplicate logical refs".into(),
                ));
            }
        }
        Ok(Self {
            provider_ref,
            services: by_ref,
            records: BTreeMap::new(),
            next_instance: 0,
        })
    }

    fn record(&self, allocation: &ProviderAllocation) -> Result<&ManagedServiceRecord> {
        self.records.get(&allocation.material_ref).ok_or_else(|| {
            WorkcellError::NotFound(format!(
                "managed service binding `{}` is not known by this provider",
                allocation.material_ref
            ))
        })
    }

    fn spawn(service: &ManagedHostService) -> Result<Child> {
        let mut command = Command::new(&service.program);
        command
            .args(&service.args)
            .envs(&service.env)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        if let Some(cwd) = &service.cwd {
            command.current_dir(cwd);
        }
        command.spawn().map_err(|error| {
            WorkcellError::OperationFailed(format!(
                "start managed host service `{}` with `{}`: {error}",
                service.logical_ref, service.program
            ))
        })
    }

    fn wait_until_ready(service: &ManagedHostService, child: &mut Child) -> Result<()> {
        let Some(probe) = &service.readiness else {
            return Ok(());
        };
        let deadline = Instant::now() + Duration::from_millis(probe.timeout_ms);
        loop {
            if let Some(status) = child.try_wait().map_err(|error| {
                WorkcellError::OperationFailed(format!(
                    "observe managed service `{}` during readiness: {error}",
                    service.logical_ref
                ))
            })? {
                return Err(WorkcellError::Unavailable(format!(
                    "managed service `{}` exited before readiness with {status}",
                    service.logical_ref
                )));
            }
            if probe.reachable() {
                return Ok(());
            }
            if Instant::now() >= deadline {
                return Err(WorkcellError::Unavailable(format!(
                    "managed service `{}` did not become reachable at {}:{} within {} ms",
                    service.logical_ref, probe.host, probe.port, probe.timeout_ms
                )));
            }
            thread::sleep(Duration::from_millis(probe.interval_ms));
        }
    }
}

impl Drop for ManagedHostServiceProvider {
    fn drop(&mut self) {
        for record in self.records.values() {
            if let Ok(mut child) = record.child.try_borrow_mut() {
                if child.try_wait().ok().flatten().is_none() {
                    let _ = child.kill();
                    let _ = child.wait();
                }
            }
        }
    }
}

impl ProviderPort for ManagedHostServiceProvider {
    fn provider_ref(&self) -> &ProviderRef {
        &self.provider_ref
    }

    fn port_kind(&self) -> ProviderPortKind {
        ProviderPortKind::Service
    }

    fn offers(&self) -> Result<Vec<OperationalOffer>> {
        let mut offers = Vec::with_capacity(self.services.len());
        for service in self.services.values() {
            let available = program_available(&service.program);
            let health = if available {
                HealthState::Healthy
            } else {
                HealthState::Unavailable
            };
            let mut metadata = service.metadata.clone();
            metadata.insert("implementation".into(), "managed-host-service".into());
            metadata.insert("logical_ref".into(), service.logical_ref.clone());
            metadata.insert("program".into(), service.program.clone());
            metadata.insert("endpoint".into(), service.endpoint.clone());
            offers.push(OperationalOffer {
                offer_ref: OfferRef::new(format!(
                    "offer:{}:managed-service:{}",
                    self.provider_ref,
                    stable_key(&[&service.logical_ref, &service.program])
                ))
                .map_err(|error| WorkcellError::OperationFailed(error.into()))?,
                provider_ref: self.provider_ref.clone(),
                port: ProviderPortKind::Service.as_str().into(),
                affordances: vec!["service:managed-host-process".into()],
                connections: vec![service.logical_ref.clone()],
                exposures: vec![],
                isolation_trust: vec!["host-process".into()],
                availability: if available {
                    Availability::Available
                } else {
                    Availability::Unavailable
                },
                health,
                capacity: BTreeMap::new(),
                metadata,
            });
        }
        Ok(offers)
    }
}

impl ServiceProvider for ManagedHostServiceProvider {
    fn resolve_service(&mut self, request: &ServiceMaterialRequest) -> Result<ProviderAllocation> {
        let logical_ref = request.connection.as_str();
        let service = self.services.get(logical_ref).cloned().ok_or_else(|| {
            WorkcellError::UnsatisfiedDemand(format!(
                "logical service `{logical_ref}` is not offered"
            ))
        })?;
        if !program_available(&service.program) {
            return Err(WorkcellError::Unavailable(format!(
                "managed service program `{}` for `{logical_ref}` is unavailable",
                service.program
            )));
        }

        let mut child = Self::spawn(&service)?;
        if let Err(error) = Self::wait_until_ready(&service, &mut child) {
            let _ = child.kill();
            let _ = child.wait();
            return Err(error);
        }

        self.next_instance = self.next_instance.saturating_add(1);
        let key = stable_key(&[
            request.demand_ref.as_str(),
            logical_ref,
            &service.endpoint,
            &self.next_instance.to_string(),
        ]);
        let material_ref = format!("service:managed-host:{key}");
        let mut properties = BTreeMap::new();
        properties.insert("logical_ref".into(), logical_ref.into());
        properties.insert("endpoint".into(), service.endpoint.clone());
        properties.insert("program".into(), service.program.clone());
        properties.insert("pid".into(), child.id().to_string());
        let mut provenance = BTreeMap::new();
        provenance.insert("implementation".into(), "managed-host-service".into());
        provenance.insert("logical_ref".into(), logical_ref.into());
        provenance.insert("provider_ref".into(), self.provider_ref.to_string());
        provenance.insert("program".into(), service.program.clone());
        for (key, value) in &service.metadata {
            provenance.insert(format!("metadata.{key}"), value.clone());
        }

        self.records.insert(
            material_ref.clone(),
            ManagedServiceRecord {
                service,
                child: RefCell::new(child),
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
        let mut child = record.child.try_borrow_mut().map_err(|_| {
            WorkcellError::OperationFailed(format!(
                "managed service `{}` is already being observed",
                allocation.material_ref
            ))
        })?;
        let status = child.try_wait().map_err(|error| {
            WorkcellError::OperationFailed(format!(
                "observe managed service `{}`: {error}",
                allocation.material_ref
            ))
        })?;
        let running = status.is_none();
        let reachable = running
            && record
                .service
                .readiness
                .as_ref()
                .is_none_or(TcpEndpointProbe::reachable);
        let health = if !running {
            HealthState::Unavailable
        } else if reachable {
            HealthState::Healthy
        } else {
            HealthState::Degraded
        };
        let mut detail = BTreeMap::new();
        detail.insert("logical_ref".into(), record.service.logical_ref.clone());
        detail.insert("endpoint".into(), record.service.endpoint.clone());
        detail.insert("program".into(), record.service.program.clone());
        detail.insert("running".into(), running.to_string());
        detail.insert("reachable".into(), reachable.to_string());
        if let Some(status) = status {
            detail.insert("exit_status".into(), status.to_string());
        }
        Ok(ProviderObservation {
            provider_ref: self.provider_ref.clone(),
            material_ref: allocation.material_ref.clone(),
            health,
            detail,
        })
    }

    fn release_service(
        &mut self,
        allocation: &ProviderAllocation,
        retention: &RetentionExpectation,
    ) -> Result<ProviderReleaseResult> {
        self.record(allocation)?;
        match retention {
            RetentionExpectation::Preserve => Ok(ProviderReleaseResult {
                provider_ref: self.provider_ref.clone(),
                material_ref: allocation.material_ref.clone(),
                disposition: ReleaseDisposition::Preserved,
                changed: false,
            }),
            RetentionExpectation::Release => {
                let record = self
                    .records
                    .remove(&allocation.material_ref)
                    .expect("managed service record checked before removal");
                let mut child = record.child.into_inner();
                if child.try_wait().map_err(|error| {
                    WorkcellError::OperationFailed(format!(
                        "observe managed service during release: {error}"
                    ))
                })?.is_none()
                {
                    child.kill().map_err(|error| {
                        WorkcellError::OperationFailed(format!(
                            "stop managed service `{}`: {error}",
                            allocation.material_ref
                        ))
                    })?;
                    child.wait().map_err(|error| {
                        WorkcellError::OperationFailed(format!(
                            "wait for managed service `{}`: {error}",
                            allocation.material_ref
                        ))
                    })?;
                }
                Ok(ProviderReleaseResult {
                    provider_ref: self.provider_ref.clone(),
                    material_ref: allocation.material_ref.clone(),
                    disposition: ReleaseDisposition::Released,
                    changed: true,
                })
            }
            RetentionExpectation::SuspendIfSupported
            | RetentionExpectation::SnapshotIfSupported => Err(WorkcellError::Unsupported(
                "managed host service does not support suspend or snapshot".into(),
            )),
        }
    }
}

fn program_available(program: &str) -> bool {
    let path = Path::new(program);
    if path.components().count() > 1 {
        return path.is_file();
    }
    env::var_os("PATH")
        .map(|path| env::split_paths(&path).any(|directory| directory.join(program).is_file()))
        .unwrap_or(false)
}
