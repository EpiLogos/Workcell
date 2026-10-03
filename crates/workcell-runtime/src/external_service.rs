use std::{
    cell::RefCell,
    collections::BTreeMap,
    fmt,
    path::Path,
    process::{Command, ExitStatus, Stdio},
    thread,
    time::{Duration, Instant},
};

use epilogos_workcell_core::{
    Availability, HealthState, OfferRef, OperationalOffer, ProviderAllocation, ProviderObservation,
    ProviderPort, ProviderPortKind, ProviderRef, ProviderReleaseResult, ReleaseDisposition, Result,
    RetentionExpectation, ServiceMaterialRequest, ServiceProvider, WorkcellError,
};

use crate::support::stable_key;
use crate::{run_bounded_process, BoundedProcessOutput};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};

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

    fn capture(
        &self,
        basis: Option<&Value>,
        generation: Option<&str>,
    ) -> Result<BoundedProcessOutput> {
        let mut command = Command::new(&self.program);
        command
            .args(&self.args)
            .envs(&self.environment)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        // A parent process or declaration cannot substitute current identity
        // for an allocation's retained target-native identity.
        for key in INSTANCE_ENV {
            command.env_remove(key);
        }
        command.env("WORKCELL_TARGET_INSTANCE_PROTOCOL", INSTANCE_SCHEMA);
        if let Some(generation) = generation {
            command.env("WORKCELL_TARGET_GENERATION", generation);
        }
        if let Some(basis) = basis {
            command
                .env(
                    "WORKCELL_TARGET_BASIS_PATH",
                    basis["basis_path"].as_str().unwrap(),
                )
                .env(
                    "WORKCELL_TARGET_BASIS_SHA256",
                    basis["basis_sha256"].as_str().unwrap(),
                )
                .env(
                    "WORKCELL_TARGET_GENERATION",
                    basis["generation"].as_str().unwrap(),
                );
        }
        run_bounded_process(command, Duration::from_secs(10), 16_384)
    }

    fn run(&self) -> std::io::Result<ExitStatus> {
        let mut command = Command::new(&self.program);
        command
            .args(&self.args)
            .envs(&self.environment)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        crate::bounded_process::run_status_only(command, Duration::from_secs(10))
            .map_err(|failure| {
                let kind = if failure.timed_out() {
                    std::io::ErrorKind::TimedOut
                } else if let Some(cause) = failure.native_cause() {
                    cause.kind()
                } else {
                    match failure.kind() {
                        crate::BoundedCaptureFailureKind::InvalidLimits => std::io::ErrorKind::InvalidInput,
                        crate::BoundedCaptureFailureKind::UnsupportedPlatform => std::io::ErrorKind::Unsupported,
                        _ => std::io::ErrorKind::Other,
                    }
                };
                // The wrapper preserves the original typed failure/cause chain.
                // Its own raw_os_error is not a reconstruction of that cause.
                std::io::Error::new(kind, failure)
            })
    }

    // Command arguments, environment and output may contain private data. None
    // is copied into observations, errors, allocation receipts or discovery.
    fn display(&self) -> String {
        self.program.clone()
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ExternalServiceAcquisition {
    /// Register/bind an existing installation but never start it implicitly.
    ObserveExisting,
    /// Start through the target-native command if the service is not healthy.
    EnsureRunning,
}

#[derive(Clone, Eq, PartialEq)]
pub struct ExternalManagedService {
    pub logical_ref: String,
    pub endpoint: String,
    pub status: ExternalServiceCommand,
    pub readiness: Option<ExternalServiceCommand>,
    pub start: Option<ExternalServiceCommand>,
    pub stop: Option<ExternalServiceCommand>,
    pub restart: Option<ExternalServiceCommand>,
    pub acquisition: ExternalServiceAcquisition,
    pub readiness_timeout_ms: Option<u64>,
    pub readiness_interval_ms: Option<u64>,
    pub metadata: BTreeMap<String, String>,
    /// Optional command returning an immutable target-native instance receipt.
    /// Identity capture never asserts readiness or lifecycle ownership.
    pub target_instance: Option<ExternalServiceCommand>,
}

impl fmt::Debug for ExternalManagedService {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        // Preserve legacy declaration digests when the additive contract is
        // absent: the old derived Debug field order was the fingerprint basis.
        let mut value = f.debug_struct("ExternalManagedService");
        value
            .field("logical_ref", &self.logical_ref)
            .field("endpoint", &self.endpoint)
            .field("status", &self.status)
            .field("readiness", &self.readiness)
            .field("start", &self.start)
            .field("stop", &self.stop)
            .field("restart", &self.restart)
            .field("acquisition", &self.acquisition)
            .field("readiness_timeout_ms", &self.readiness_timeout_ms)
            .field("readiness_interval_ms", &self.readiness_interval_ms)
            .field("metadata", &self.metadata);
        if let Some(command) = &self.target_instance {
            value.field("target_instance", command);
        }
        value.finish()
    }
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
            readiness_timeout_ms: None,
            readiness_interval_ms: None,
            metadata: BTreeMap::new(),
            target_instance: None,
        })
    }

    fn fingerprint(&self) -> String {
        format!(
            "sha256:{:x}",
            Sha256::digest(format!("{self:?}").as_bytes())
        )
    }

    pub fn with_readiness(mut self, command: ExternalServiceCommand) -> Self {
        self.readiness = Some(command);
        self
    }

    /// How long a command probe may keep polling before the service is
    /// called not ready. A start command usually returns before the
    /// material it launches is reachable, so the window must cover the
    /// launch, not just the probe itself.
    pub fn with_readiness_timing(mut self, timeout_ms: u64, interval_ms: u64) -> Self {
        self.readiness_timeout_ms = Some(timeout_ms);
        self.readiness_interval_ms = Some(interval_ms);
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

    pub fn with_target_instance(mut self, capture: ExternalServiceCommand) -> Self {
        self.target_instance = Some(capture);
        self
    }

    pub fn with_acquisition(mut self, acquisition: ExternalServiceAcquisition) -> Self {
        self.acquisition = acquisition;
        self
    }

    pub fn with_metadata(
        mut self,
        key: impl Into<String>,
        value: impl Into<String>,
    ) -> Result<Self> {
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
    instance_basis: Option<Value>,
    receipt_properties: BTreeMap<String, String>,
}

pub(crate) struct PreparedExternalServiceRestore {
    records: BTreeMap<String, ExternalServiceRecord>,
    retired: BTreeMap<String, ExternalServiceRecord>,
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
            if service.target_instance.is_some()
                && std::iter::once(&service.status)
                    .chain(service.readiness.iter())
                    .chain(service.start.iter())
                    .chain(service.stop.iter())
                    .chain(service.restart.iter())
                    .chain(service.target_instance.iter())
                    .any(|command| {
                        INSTANCE_ENV
                            .iter()
                            .any(|key| command.environment.contains_key(*key))
                    })
            {
                return Err(WorkcellError::InvalidDemand("target-instance control environment is owned by compiled Workcell and cannot be declared".into()));
            }
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

    /// Validate the original receipt and actual bound observation before
    /// restoring process-local ownership; this never starts a target.
    pub fn restore_allocation(&self, allocation: &ProviderAllocation) -> Result<()> {
        let prepared = self.prepare_allocation_restores([allocation.clone()])?;
        self.commit_allocation_restores(prepared);
        Ok(())
    }

    /// Validate the complete receipt batch and read only original-instance
    /// observations without changing records or starting/controlling targets.
    /// A later refusal cannot leak a partial dependency census.
    pub(crate) fn prepare_allocation_restores(
        &self,
        allocations: impl IntoIterator<Item = ProviderAllocation>,
    ) -> Result<PreparedExternalServiceRestore> {
        let mut records: BTreeMap<String, ExternalServiceRecord> = BTreeMap::new();
        let mut retired: BTreeMap<String, ExternalServiceRecord> = BTreeMap::new();
        for allocation in allocations {
            let record = self.reenter(&allocation)?;
            if self
                .records
                .borrow()
                .get(&allocation.material_ref)
                .is_some_and(|existing| existing.receipt_properties != allocation.properties)
                || records
                    .get(&allocation.material_ref)
                    .is_some_and(|existing| existing.receipt_properties != allocation.properties)
                || retired
                    .get(&allocation.material_ref)
                    .is_some_and(|existing| existing.receipt_properties != allocation.properties)
            {
                return Err(WorkcellError::ReconciliationFailed("world contains an external receipt conflicting with existing or earlier binding identity; no restore committed".into()));
            }
            if record.instance_basis.is_some() {
                let observed = self.observe_record(&allocation, &record)?;
                if original_instance_retired(&observed, record.instance_basis.as_ref().unwrap()) {
                    retired.insert(allocation.material_ref, record);
                    continue;
                }
            }
            records.insert(allocation.material_ref, record);
        }
        Ok(PreparedExternalServiceRestore { records, retired })
    }

    pub(crate) fn commit_allocation_restores(&self, prepared: PreparedExternalServiceRestore) {
        let mut records = self.records.borrow_mut();
        for (reference, retired) in prepared.retired {
            if records
                .get(&reference)
                .is_some_and(|existing| existing.receipt_properties == retired.receipt_properties)
            {
                records.remove(&reference);
            }
        }
        records.extend(prepared.records);
    }

    pub fn recover_service(&self, allocation: &ProviderAllocation) -> Result<ProviderAllocation> {
        let record = self.record(allocation)?;
        if self.observe_record(allocation, &record)?.health == HealthState::Healthy {
            return Ok(allocation.clone());
        }
        if record.service.target_instance.is_some() {
            return Err(WorkcellError::Unsupported(
                "generation-bound material is unavailable; reconcile the original instance, then explicitly rematerialise a successor".into(),
            ));
        }
        if !record.started_by_provider
            || record.service.acquisition != ExternalServiceAcquisition::EnsureRunning
        {
            return Err(WorkcellError::Unsupported("observed target service is not owned for restart; re-resolve with its lifecycle owner".into()));
        }
        let status = run_probe(&record.service.status);
        let command = if status == HealthState::Healthy {
            record.service.restart.as_ref()
        } else {
            record.service.start.as_ref()
        }
        .ok_or_else(|| {
            WorkcellError::Unsupported(
                "target does not declare the required recovery command".into(),
            )
        })?;
        run_required(command, "recover external service")?;
        let observed = self.observe_record(allocation, &record)?;
        if observed.health != HealthState::Healthy {
            return Err(WorkcellError::Unavailable(
                "target-native recovery did not restore readiness".into(),
            ));
        }
        let mut recovered = allocation.clone();
        recovered.health = observed.health;
        recovered.provenance.insert(
            "recovery".into(),
            "target-native-command; process/session identity not asserted".into(),
        );
        Ok(recovered)
    }

    pub fn restart_service(&self, allocation: &ProviderAllocation) -> Result<ProviderObservation> {
        let record = self.record(allocation)?;
        if !record.started_by_provider {
            return Err(WorkcellError::Unsupported(
                "observed target service is not owned for restart".into(),
            ));
        }
        if record.service.target_instance.is_some() {
            return Err(WorkcellError::Unsupported("a target-native restart creates a successor; release the original instance and explicitly rematerialise".into()));
        }
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
        let rebuilt = self.reenter(allocation)?;
        if let Some(record) = self.records.borrow().get(&allocation.material_ref).cloned() {
            if record.receipt_properties != allocation.properties {
                return Err(WorkcellError::OperationFailed("supplied external allocation differs from its cached receipt; no target operation was performed".into()));
            }
            return Ok(record);
        }
        Ok(rebuilt)
    }

    /// Rebuild a process-local record for a binding this provider itself minted.
    ///
    /// A target-owned service is supervised outside Workcell, so a later process
    /// can honestly re-observe it: the status command is run again and answers
    /// for itself. Re-entry is refused unless the binding names a service that
    /// is still declared at the same endpoint, and `started_by_provider` is
    /// taken from the binding rather than assumed, so release never stops
    /// something Workcell did not start.
    fn reenter(&self, allocation: &ProviderAllocation) -> Result<ExternalServiceRecord> {
        if allocation.provider_ref != self.provider_ref
            || allocation.port != ProviderPortKind::Service
        {
            return Err(WorkcellError::OperationFailed(
                "external allocation provider or port differs from this provider".into(),
            ));
        }
        let logical_ref = allocation
            .properties
            .get("logical_ref")
            .ok_or_else(|| WorkcellError::NotFound("external receipt has no logical ref".into()))?;
        let service = self.services.get(logical_ref).ok_or_else(|| {
            WorkcellError::NotFound("external receipt's service is no longer declared".into())
        })?;
        if allocation.properties.get("endpoint") != Some(&service.endpoint) {
            return Err(WorkcellError::OperationFailed(
                "external receipt endpoint differs from its declaration".into(),
            ));
        }
        match allocation.properties.get("declaration_digest") {
            Some(digest) if digest != &service.fingerprint() => return Err(WorkcellError::OperationFailed("external receipt declaration changed; reconcile through the original lifecycle owner before rematerialising".into())),
            None if service.target_instance.is_some() => return Err(WorkcellError::OperationFailed("generation-bound receipt requires its declaration digest".into())),
            _ => {}
        }
        let instance_basis = allocation
            .properties
            .get("target_instance_basis")
            .map(|raw| parse_basis(raw.as_bytes(), &service.endpoint))
            .transpose()?;
        if service.target_instance.is_some() != instance_basis.is_some() {
            return Err(WorkcellError::OperationFailed(
                "external receipt target-instance contract differs from its declaration".into(),
            ));
        }
        let started_by_provider = match allocation
            .properties
            .get("started_by_provider")
            .map(String::as_str)
        {
            Some("true") => true,
            None | Some("false") => false,
            _ => {
                return Err(WorkcellError::OperationFailed(
                    "external receipt has invalid ownership standing".into(),
                ))
            }
        };
        if service.target_instance.is_some() {
            if started_by_provider && service.stop.is_none() {
                return Err(WorkcellError::OperationFailed("owned target-instance receipt has no original-instance stop declaration; reconcile with its lifecycle owner".into()));
            }
            let demand_ref = allocation.properties.get("demand_ref").ok_or_else(|| {
                WorkcellError::OperationFailed(
                    "generation-bound receipt requires its original demand ref".into(),
                )
            })?;
            let expected = format!(
                "service:external:{}",
                stable_key(&[
                    demand_ref,
                    &service.logical_ref,
                    &service.endpoint,
                    &service.fingerprint(),
                    instance_basis.as_ref().unwrap()["generation"]
                        .as_str()
                        .unwrap()
                ])
            );
            if allocation.material_ref != expected {
                return Err(WorkcellError::OperationFailed(
                    "generation-bound material ref differs from its original receipt tuple".into(),
                ));
            }
            let start_receipt = allocation
                .properties
                .get("target_start_receipt")
                .map(|raw| parse_start(raw.as_bytes()))
                .transpose()?;
            if started_by_provider && start_receipt.is_none() {
                return Err(WorkcellError::OperationFailed(
                    "owned target-instance receipt requires its original native start intent"
                        .into(),
                ));
            }
            if let Some(start) = start_receipt {
                if start["started"].as_bool() != Some(started_by_provider)
                    || start["generation"] != instance_basis.as_ref().unwrap()["generation"]
                {
                    return Err(WorkcellError::OperationFailed(
                        "target start intent and instance receipt disagree".into(),
                    ));
                }
            }
        }
        let record = ExternalServiceRecord {
            service: service.clone(),
            started_by_provider,
            instance_basis,
            receipt_properties: allocation.properties.clone(),
        };
        Ok(record)
    }

    fn observe_record(
        &self,
        allocation: &ProviderAllocation,
        record: &ExternalServiceRecord,
    ) -> Result<ProviderObservation> {
        let status = run_bound_probe(&record.service.status, record.instance_basis.as_ref())?;
        let readiness = record
            .service
            .readiness
            .as_ref()
            .map(|command| run_bound_probe(command, record.instance_basis.as_ref()))
            .transpose()?
            .unwrap_or_else(|| status.clone());
        let health = if status.health == HealthState::Unavailable
            || readiness.health == HealthState::Unavailable
        {
            HealthState::Unavailable
        } else if status.health == HealthState::Degraded
            || readiness.health == HealthState::Degraded
        {
            HealthState::Degraded
        } else {
            HealthState::Healthy
        };
        let mut detail = record.service.metadata.clone();
        if let Some(cause) = &status.cause {
            detail.insert("status_native_cause".into(), cause.to_string());
        }
        if let Some(cause) = &readiness.cause {
            detail.insert("readiness_native_cause".into(), cause.to_string());
        }
        if let Some(witness) = &status.witness {
            detail.insert("status_native_observation".into(), witness.to_string());
        }
        if let Some(witness) = &readiness.witness {
            detail.insert("readiness_native_observation".into(), witness.to_string());
        }
        detail.insert("logical_ref".into(), record.service.logical_ref.clone());
        detail.insert("endpoint".into(), record.service.endpoint.clone());
        detail.insert("status_command".into(), record.service.status.display());
        detail.insert(
            "started_by_provider".into(),
            record.started_by_provider.to_string(),
        );
        if let Some(readiness) = &record.service.readiness {
            detail.insert("readiness_command".into(), readiness.display());
        }
        if let Some(basis) = &record.instance_basis {
            detail.insert("target_instance_basis".into(), basis.to_string());
            detail.insert(
                "dependency_scope".into(),
                "provider-local census only; cross-world dependencies unqualified".into(),
            );
        }
        Ok(ProviderObservation {
            provider_ref: self.provider_ref.clone(),
            material_ref: allocation.material_ref.clone(),
            health,
            detail,
        })
    }

    fn resolve_instance_service(
        &self,
        request: &ServiceMaterialRequest,
        service: &ExternalManagedService,
    ) -> Result<ProviderAllocation> {
        // A repeated request observes its original receipt, never whichever
        // generation an endpoint currently happens to serve.
        let existing = self
            .records
            .borrow()
            .iter()
            .find_map(|(reference, record)| {
                (record.service.logical_ref == service.logical_ref
                    && record
                        .receipt_properties
                        .get("demand_ref")
                        .map(String::as_str)
                        == Some(request.demand_ref.as_str()))
                .then(|| (reference.clone(), record.clone()))
            });
        if let Some((reference, record)) = existing {
            let allocation = ProviderAllocation {
                provider_ref: self.provider_ref.clone(),
                port: ProviderPortKind::Service,
                material_ref: reference,
                health: HealthState::Unknown,
                properties: record.receipt_properties.clone(),
                provenance: service.metadata.clone(),
            };
            let observed = self.observe_service(&allocation)?;
            if observed.health != HealthState::Healthy {
                return Err(WorkcellError::ReconciliationFailed(format!("original generation is not ready; retained original receipt {}; native status cause={}; native readiness cause={}; no successor was allocated", record.receipt_properties.get("target_instance_basis").map(String::as_str).unwrap_or("none"), observed.detail.get("status_native_cause").map(String::as_str).unwrap_or("none"), observed.detail.get("readiness_native_cause").map(String::as_str).unwrap_or("none"))));
            }
            return Ok(ProviderAllocation {
                health: HealthState::Healthy,
                ..allocation
            });
        }

        let mut start_receipt = None;
        let mut started_by_provider = false;
        if run_instance_probe(&service.status, None, &service.endpoint)?.health
            == HealthState::Unavailable
        {
            if service.acquisition == ExternalServiceAcquisition::ObserveExisting {
                return Err(WorkcellError::Unavailable(
                    "observed target is not running".into(),
                ));
            }
            let start = service.start.as_ref().ok_or_else(|| {
                WorkcellError::Unsupported("ensure-running requires a native start command".into())
            })?;
            if service.stop.is_none() {
                return Err(WorkcellError::Unsupported(
                    "ensure-running requires an original-instance stop command".into(),
                ));
            }
            let output = start.capture(None, None).map_err(|error| {
                WorkcellError::ReconciliationFailed(format!("native start failed: {error}; effects uncertain; reconcile any retained target-native starting intent, no qualified generation or guessed cleanup"))
            })?;
            require_complete(&output, "native start").map_err(|error| {
                // A failed command cannot grant start authority. A syntactically
                // qualified public intent may still identify actual effects for
                // reconciliation; private output and an unqualified generation
                // remain withheld, and no current-instance capture is attempted.
                let intent = match parse_start(&output.stdout) {
                    Ok(intent) => format!("public start intent observation (not authority): {intent}"),
                    Err(cause) => format!("secondary public start intent qualification failure: {cause}"),
                };
                WorkcellError::ReconciliationFailed(format!("{error}; {intent}; effects uncertain; no qualified start authority, current-instance capture or guessed cleanup"))
            })?;
            let receipt = parse_start(&output.stdout).map_err(|error| WorkcellError::ReconciliationFailed(format!("{error}; native start result unqualified; reconcile any retained target-native starting intent")))?;
            started_by_provider = receipt["started"].as_bool().unwrap();
            start_receipt = Some(receipt);
        }
        let generation = start_receipt
            .as_ref()
            .and_then(|receipt| receipt["generation"].as_str());
        let basis = capture_instance(service, generation).map_err(|error| {
            if let Some(receipt) = &start_receipt {
                WorkcellError::ReconciliationFailed(format!("capture failed: {error}; original native start receipt {receipt}; native effects uncertain, reconcile only that generation"))
            } else { error }
        })?;
        let material_ref = format!(
            "service:external:{}",
            stable_key(&[
                request.demand_ref.as_str(),
                &service.logical_ref,
                &service.endpoint,
                &service.fingerprint(),
                basis["generation"].as_str().unwrap(),
            ])
        );
        let mut properties = BTreeMap::from([
            ("logical_ref".into(), service.logical_ref.clone()),
            ("endpoint".into(), service.endpoint.clone()),
            ("demand_ref".into(), request.demand_ref.to_string()),
            ("configuration_owner".into(), "target".into()),
            ("lifetime".into(), "target-owned".into()),
            (
                "started_by_provider".into(),
                started_by_provider.to_string(),
            ),
            ("declaration_digest".into(), service.fingerprint()),
            ("target_instance_basis".into(), basis.to_string()),
        ]);
        if let Some(receipt) = &start_receipt {
            properties.insert("target_start_receipt".into(), receipt.to_string());
        }
        let mut provenance = service.metadata.clone();
        provenance.insert("implementation".into(), "external-managed-service".into());
        provenance.insert("lifetime".into(), "target-owned".into());
        provenance.insert(
            "dependency_scope".into(),
            "provider-local census only; cross-world dependencies unqualified".into(),
        );
        let allocation = ProviderAllocation {
            provider_ref: self.provider_ref.clone(),
            port: ProviderPortKind::Service,
            material_ref,
            health: HealthState::Healthy,
            properties,
            provenance,
        };
        let record = self.reenter(&allocation)?;
        let deadline = Instant::now()
            + Duration::from_millis(
                service
                    .readiness_timeout_ms
                    .unwrap_or(DEFAULT_READINESS_WINDOW_MS),
            );
        let interval = Duration::from_millis(
            service
                .readiness_interval_ms
                .unwrap_or(DEFAULT_READINESS_INTERVAL_MS),
        );
        loop {
            let observed = match self.observe_record(&allocation, &record) {
                Ok(observed) => observed,
                Err(probe) => {
                    return Err(self.instance_readiness_failure(
                        service,
                        &basis,
                        start_receipt.as_ref(),
                        started_by_provider,
                        format!("actual native readiness probe failed: {probe}"),
                    ))
                }
            };
            if observed.health == HealthState::Healthy {
                break;
            }
            let now = Instant::now();
            if now >= deadline {
                let failure = format!("original target instance remained unavailable within its readiness window; status cause={}; readiness cause={}", observed.detail.get("status_native_cause").map(String::as_str).unwrap_or("none"), observed.detail.get("readiness_native_cause").map(String::as_str).unwrap_or("none"));
                return Err(self.instance_readiness_failure(
                    service,
                    &basis,
                    start_receipt.as_ref(),
                    started_by_provider,
                    failure,
                ));
            }
            thread::sleep(interval.min(deadline - now));
        }
        self.records
            .borrow_mut()
            .insert(allocation.material_ref.clone(), record);
        Ok(allocation)
    }

    fn instance_readiness_failure(
        &self,
        service: &ExternalManagedService,
        basis: &Value,
        start: Option<&Value>,
        owned: bool,
        failure: String,
    ) -> WorkcellError {
        let failure = format!("{failure}; retained original basis {basis}");
        if !owned {
            return WorkcellError::Unavailable(format!("{failure}; observed target retained"));
        }
        if self
            .records
            .borrow()
            .values()
            .any(|other| other.service.logical_ref == service.logical_ref)
        {
            return WorkcellError::CleanupFailed(format!("{failure}; cleanup refused: known active material dependency; target intent and original basis retained"));
        }
        match run_bound_required(service.stop.as_ref().unwrap(), Some(basis), "readiness cleanup") {
            Ok(_) => WorkcellError::Unavailable(format!("{failure}; target-native cleanup succeeded for the original instance")),
            Err(cleanup) => WorkcellError::CleanupFailed(format!("{failure}; original start receipt {}; actual cleanup failure: {cleanup}; target intent and original basis retained", start.unwrap())),
        }
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
                let health = if service.target_instance.is_some() {
                    run_instance_probe(&service.status, None, &service.endpoint)?.health
                } else {
                    run_probe(&service.status)
                };
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
                metadata.insert("lifetime".into(), "target-owned".into());
                metadata.insert("endpoint".into(), service.endpoint.clone());
                metadata.insert("status_command".into(), service.status.display());
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
        if matches!(
            request.retention,
            RetentionExpectation::SnapshotIfSupported | RetentionExpectation::SuspendIfSupported
        ) {
            return Err(WorkcellError::Unsupported(
                "external service cannot generically suspend or snapshot".into(),
            ));
        }
        let service = self.services.get(logical_ref).cloned().ok_or_else(|| {
            WorkcellError::UnsatisfiedDemand(format!(
                "logical service `{logical_ref}` is not offered"
            ))
        })?;

        if service.target_instance.is_some() {
            return self.resolve_instance_service(request, &service);
        }

        let binding_key = format!(
            "service:external:{}",
            stable_key(&[request.demand_ref.as_str(), logical_ref, &service.endpoint])
        );
        // Repeated resolution must not turn an owned service into an unowned
        // observation merely because the first start made it healthy.
        let mut started_by_provider = self
            .records
            .borrow()
            .get(&binding_key)
            .is_some_and(|record| record.started_by_provider);
        let mut started_now = false;
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
                    if service.stop.is_none() {
                        return Err(WorkcellError::Unsupported("ensure-running requires a declared stop operation for owned-effect reconciliation".into()));
                    }
                    run_required(start, "start external service")?;
                    started_by_provider = true;
                    started_now = true;
                    if !probe_becomes_healthy(
                        &service.status,
                        service.readiness_timeout_ms,
                        service.readiness_interval_ms,
                    ) {
                        return Err(cleanup_readiness_failure(&service, None, format!(
                            "logical service `{logical_ref}` remained unavailable after target-native start within its readiness window"
                        )));
                    }
                }
            }
        }

        if let Some(readiness) = &service.readiness {
            if !probe_becomes_healthy(
                readiness,
                service.readiness_timeout_ms,
                service.readiness_interval_ms,
            ) {
                let failure = format!("logical service `{logical_ref}` is running but not ready");
                return Err(if started_now {
                    cleanup_readiness_failure(&service, None, failure)
                } else {
                    WorkcellError::Unavailable(format!("{failure}; existing target retained"))
                });
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
        properties.insert("lifetime".into(), "target-owned".into());
        properties.insert(
            "started_by_provider".into(),
            started_by_provider.to_string(),
        );
        properties.insert("declaration_digest".into(), service.fingerprint());
        let mut provenance = service.metadata.clone();
        provenance.insert("implementation".into(), "external-managed-service".into());
        provenance.insert("lifetime".into(), "target-owned".into());
        provenance.insert("status_command".into(), service.status.display());

        self.records.borrow_mut().insert(
            material_ref.clone(),
            ExternalServiceRecord {
                service: service.clone(),
                started_by_provider,
                instance_basis: None,
                receipt_properties: properties.clone(),
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
        let observed = self.observe_record(allocation, &record)?;
        if let Some(basis) = &record.instance_basis {
            if original_instance_retired(&observed, basis) {
                let mut records = self.records.borrow_mut();
                if records
                    .get(&allocation.material_ref)
                    .is_some_and(|existing| {
                        existing.receipt_properties == record.receipt_properties
                    })
                {
                    records.remove(&allocation.material_ref);
                }
            } else {
                self.records
                    .borrow_mut()
                    .insert(allocation.material_ref.clone(), record);
            }
        } else {
            self.records
                .borrow_mut()
                .insert(allocation.material_ref.clone(), record);
        }
        Ok(observed)
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
            RetentionExpectation::SuspendIfSupported
            | RetentionExpectation::SnapshotIfSupported => Err(WorkcellError::Unsupported(
                "external service cannot generically suspend or snapshot".into(),
            )),
            RetentionExpectation::Release => {
                let mut changed = false;
                if record.started_by_provider {
                    if self.records.borrow().iter().any(|(reference, other)| {
                        reference != &allocation.material_ref
                            && other.service.logical_ref == record.service.logical_ref
                    }) {
                        return Err(WorkcellError::OperationFailed("target service still has other material bindings; detach them before stopping the owner binding".into()));
                    }
                    if let Some(stop) = &record.service.stop {
                        changed = run_bound_required(
                            stop,
                            record.instance_basis.as_ref(),
                            "stop external service",
                        )?;
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
        Ok(status) if status.success() => HealthState::Healthy,
        Ok(_) => HealthState::Unavailable,
        Err(_) => HealthState::Unavailable,
    }
}

pub(crate) const INSTANCE_SCHEMA: &str = "workcell.external-target-instance/v1";
const OBSERVATION_SCHEMA: &str = "workcell.external-target-observation/v1";
const START_SCHEMA: &str = "workcell.external-target-start/v1";
const ERROR_SCHEMA: &str = "workcell.external-target-error/v1";
const STOP_SCHEMA: &str = "workcell.external-target-stop/v1";
const INSTANCE_ENV: [&str; 4] = [
    "WORKCELL_TARGET_INSTANCE_PROTOCOL",
    "WORKCELL_TARGET_BASIS_PATH",
    "WORKCELL_TARGET_BASIS_SHA256",
    "WORKCELL_TARGET_GENERATION",
];

fn require_complete(output: &BoundedProcessOutput, action: &str) -> Result<()> {
    if output.timed_out || !output.output_complete || output.output_truncated {
        return Err(WorkcellError::ReconciliationFailed(format!(
            "{action}: incomplete native result (timed_out={}, output_complete={}, truncated={}); effects uncertain; command output withheld",
            output.timed_out, output.output_complete, output.output_truncated,
        )));
    }
    if !output.status.success() {
        let cause = match safe_native_cause(&output.stderr) {
            Ok(cause) => format!("native cause {cause}"),
            Err(error) => format!("secondary native cause qualification failure: {error}"),
        };
        return Err(WorkcellError::OperationFailed(format!(
            "{action}: native exit {}; {cause}; effects uncertain; private command output withheld",
            output.status
        )));
    }
    Ok(())
}

fn safe_native_cause(bytes: &[u8]) -> Result<Value> {
    let value = native_document(bytes, ERROR_SCHEMA).map_err(|_| WorkcellError::ReconciliationFailed("native failure result is unqualified; private command output withheld; effects unknown".into()))?;
    let kind = string_field(&value, "kind")?;
    if !matches!(
        kind,
        "target-absent" | "not-ready" | "native-http-refusal" | "operation-failed"
    ) {
        return Err(WorkcellError::ReconciliationFailed(
            "unsupported native failure kind; private command output withheld; effects unknown"
                .into(),
        ));
    }
    let native_type = string_field(&value, "native_type")?;
    if native_type.len() > 128
        || !native_type
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'.' | b'/' | b'-'))
    {
        return Err(WorkcellError::ReconciliationFailed(
            "unqualified native failure type; private command output withheld".into(),
        ));
    }
    let mut cause = json!({"schema": ERROR_SCHEMA, "kind": kind, "native_type": native_type});
    if let Some(status) = value.get("http_status") {
        let status = status
            .as_u64()
            .filter(|status| (100..=599).contains(status))
            .ok_or_else(|| {
                WorkcellError::ReconciliationFailed(
                    "invalid native HTTP failure status; private output withheld".into(),
                )
            })?;
        cause["http_status"] = json!(status);
    }
    if kind == "native-http-refusal" && cause.get("http_status").is_none() {
        return Err(WorkcellError::ReconciliationFailed(
            "native HTTP refusal is missing its actual status; private output withheld".into(),
        ));
    }
    if value.get("generation").is_some() {
        cause["generation"] = json!(string_field(&value, "generation")?);
    }
    if value.get("evidence_path").is_some() || value.get("evidence_sha256").is_some() {
        let path = string_field(&value, "evidence_path")?;
        if !Path::new(path).is_absolute() {
            return Err(WorkcellError::ReconciliationFailed(
                "native error evidence is not an absolute owned reference; private output withheld"
                    .into(),
            ));
        }
        cause["evidence_path"] = json!(path);
        cause["evidence_sha256"] = json!(digest_field(&value, "evidence_sha256")?);
    }
    Ok(cause)
}

fn string_field<'a>(value: &'a Value, field: &str) -> Result<&'a str> {
    value
        .get(field)
        .and_then(Value::as_str)
        .filter(|s| !s.is_empty() && s.len() <= 4096)
        .ok_or_else(|| {
            WorkcellError::OperationFailed(format!(
                "native instance result has invalid `{field}`; output withheld"
            ))
        })
}

fn digest_field<'a>(value: &'a Value, field: &str) -> Result<&'a str> {
    let digest = string_field(value, field)?;
    if digest.len() != 64
        || !digest
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
    {
        return Err(WorkcellError::OperationFailed(format!(
            "native instance result has invalid `{field}` digest; output withheld"
        )));
    }
    Ok(digest)
}

fn native_document(bytes: &[u8], schema: &str) -> Result<Value> {
    let value: Value = serde_json::from_slice(bytes).map_err(|_| {
        WorkcellError::OperationFailed(
            "native instance result is not a complete JSON document; output withheld".into(),
        )
    })?;
    if value.get("schema").and_then(Value::as_str) != Some(schema) {
        return Err(WorkcellError::OperationFailed(
            "native instance result has an unsupported schema; output withheld".into(),
        ));
    }
    Ok(value)
}

fn parse_start(bytes: &[u8]) -> Result<Value> {
    let value = native_document(bytes, START_SCHEMA)?;
    let generation = string_field(&value, "generation")?;
    let intent_path = string_field(&value, "intent_path")?;
    if !Path::new(intent_path).is_absolute() {
        return Err(WorkcellError::OperationFailed(
            "native start intent path is not absolute".into(),
        ));
    }
    let intent_sha256 = digest_field(&value, "intent_sha256")?;
    let started = value
        .get("started")
        .and_then(Value::as_bool)
        .ok_or_else(|| {
            WorkcellError::OperationFailed(
                "native start result has no truthful ownership standing".into(),
            )
        })?;
    Ok(
        json!({"schema": START_SCHEMA, "generation": generation, "started": started,
        "intent_path": intent_path, "intent_sha256": intent_sha256}),
    )
}

fn parse_basis(bytes: &[u8], endpoint: &str) -> Result<Value> {
    let value = native_document(bytes, INSTANCE_SCHEMA)?;
    let basis_path = string_field(&value, "basis_path")?;
    let generation = string_field(&value, "generation")?;
    let actual_endpoint = string_field(&value, "endpoint")?;
    if !Path::new(basis_path).is_absolute() || actual_endpoint != endpoint {
        return Err(WorkcellError::OperationFailed("native instance result differs from declared endpoint or has a non-absolute basis path".into()));
    }
    let basis_sha256 = digest_field(&value, "basis_sha256")?;
    let config_sha256 = digest_field(&value, "config_sha256")?;
    let server = value.get("server").ok_or_else(|| {
        WorkcellError::OperationFailed(
            "native instance result has no actual server identity".into(),
        )
    })?;
    let pid = server
        .get("pid")
        .and_then(Value::as_u64)
        .filter(|pid| *pid > 0)
        .ok_or_else(|| {
            WorkcellError::OperationFailed("native instance result has no actual PID".into())
        })?;
    let uid = server.get("uid").and_then(Value::as_u64).ok_or_else(|| {
        WorkcellError::OperationFailed("native instance result has no actual UID".into())
    })?;
    let start = string_field(server, "start")?;
    // Copy only the public material tuple. Authentication keys and arbitrary
    // controller output never enter receipts, observations or errors.
    Ok(json!({"schema": INSTANCE_SCHEMA, "basis_path": basis_path,
        "basis_sha256": basis_sha256, "generation": generation,
        "config_sha256": config_sha256, "endpoint": actual_endpoint,
        "server": {"pid": pid, "uid": uid, "start": start}}))
}

fn capture_instance(service: &ExternalManagedService, generation: Option<&str>) -> Result<Value> {
    let capture = service.target_instance.as_ref().unwrap();
    let deadline = Instant::now()
        + Duration::from_millis(
            service
                .readiness_timeout_ms
                .unwrap_or(DEFAULT_READINESS_WINDOW_MS),
        );
    let interval = Duration::from_millis(
        service
            .readiness_interval_ms
            .unwrap_or(DEFAULT_READINESS_INTERVAL_MS),
    );
    loop {
        let output = capture.capture(None, generation)?;
        // Partial output is never sufficient authority even if it looks like
        // a complete receipt prefix. Nonzero can be a genuinely late basis.
        if output.timed_out || !output.output_complete || output.output_truncated {
            require_complete(&output, "capture native instance")?;
        }
        if output.status.success() {
            let basis = parse_basis(&output.stdout, &service.endpoint)?;
            if generation.is_some_and(|expected| basis["generation"].as_str() != Some(expected)) {
                return Err(WorkcellError::ReconciliationFailed("capture returned a different generation than the original start intent; no target effect performed".into()));
            }
            return Ok(basis);
        }
        let cause = safe_native_cause(&output.stderr)?;
        if generation.is_some_and(|expected| cause["generation"].as_str() != Some(expected)) {
            return Err(WorkcellError::ReconciliationFailed(format!("native capture failure differs from original start generation: {cause}; receipt retained")));
        }
        if cause["kind"].as_str() != Some("not-ready") {
            return Err(WorkcellError::ReconciliationFailed(format!("native instance capture failed: {cause}; private output withheld; original generation retained")));
        }
        let now = Instant::now();
        if now >= deadline {
            return Err(WorkcellError::OperationFailed(format!("capture native instance: native exit {}; native cause {cause}; private command output withheld", output.status)));
        }
        thread::sleep(interval.min(deadline - now));
    }
}

#[derive(Clone)]
struct NativeProbe {
    health: HealthState,
    cause: Option<Value>,
    witness: Option<Value>,
}

fn run_instance_probe(
    command: &ExternalServiceCommand,
    basis: Option<&Value>,
    endpoint: &str,
) -> Result<NativeProbe> {
    let output = command.capture(basis, None)?;
    if output.timed_out || !output.output_complete || output.output_truncated {
        require_complete(&output, "native probe")?;
    }
    if output.status.success() {
        let parse_witness = || -> Result<NativeProbe> {
            let mut value = native_document(&output.stdout, OBSERVATION_SCHEMA)?;
            let native_healthy = value.get("native_healthy").and_then(Value::as_bool)
                .ok_or_else(|| WorkcellError::ReconciliationFailed("successful native probe has no semantic native health witness; output withheld".into()))?;
            value["schema"] = json!(INSTANCE_SCHEMA);
            let actual = parse_basis(&serde_json::to_vec(&value).expect("native JSON"), endpoint)?;
            if basis.is_some_and(|original| *original != actual) {
                return Err(WorkcellError::ReconciliationFailed("successful native observation differs from the original immutable target basis; no cache, mutation or successor adoption".into()));
            }
            let mut witness = actual;
            witness["schema"] = json!(OBSERVATION_SCHEMA);
            witness["native_healthy"] = json!(native_healthy);
            Ok(NativeProbe {
                health: if native_healthy {
                    HealthState::Healthy
                } else {
                    HealthState::Degraded
                },
                cause: None,
                witness: Some(witness),
            })
        };
        return parse_witness().map_err(|error| WorkcellError::ReconciliationFailed(format!("native successful observation remains unqualified: {error}; original basis {}; effects/health uncertain; private output withheld", basis.map(Value::to_string).unwrap_or_else(|| "unbound observation; no allocation authority".into()))));
    }
    let cause = safe_native_cause(&output.stderr)?;
    if let Some(basis) = basis {
        if cause["generation"] != basis["generation"] {
            return Err(WorkcellError::ReconciliationFailed(format!("native probe failure did not qualify the original generation: {cause}; private output withheld; original receipt retained")));
        }
        if cause["kind"] == "target-absent"
            && (cause.get("evidence_path").is_none() || cause.get("evidence_sha256").is_none())
        {
            return Err(WorkcellError::ReconciliationFailed(format!("original-instance absence has no exact native retirement evidence: {cause}; receipt retained")));
        }
    }
    match cause["kind"].as_str().unwrap() {
        "target-absent" => Ok(NativeProbe { health: HealthState::Unavailable, cause: Some(cause), witness: None }),
        "not-ready" => Ok(NativeProbe { health: HealthState::Degraded, cause: Some(cause), witness: None }),
        _ => Err(WorkcellError::ReconciliationFailed(format!("native probe failed: {cause}; private output withheld; no replacement or replay authorised"))),
    }
}

fn run_bound_probe(command: &ExternalServiceCommand, basis: Option<&Value>) -> Result<NativeProbe> {
    match basis {
        Some(original) => {
            run_instance_probe(command, basis, original["endpoint"].as_str().unwrap())
        }
        None => Ok(NativeProbe {
            health: run_probe(command),
            cause: None,
            witness: None,
        }),
    }
}

fn original_instance_retired(observed: &ProviderObservation, basis: &Value) -> bool {
    observed.health == HealthState::Unavailable
        && ["status_native_cause", "readiness_native_cause"]
            .iter()
            .any(|key| {
                observed
                    .detail
                    .get(*key)
                    .and_then(|raw| serde_json::from_str::<Value>(raw).ok())
                    .is_some_and(|cause| {
                        cause["kind"] == "target-absent"
                            && cause["generation"] == basis["generation"]
                            && cause.get("evidence_path").is_some()
                            && cause.get("evidence_sha256").is_some()
                    })
            })
}

fn run_bound_required(
    command: &ExternalServiceCommand,
    basis: Option<&Value>,
    action: &str,
) -> Result<bool> {
    match basis {
        Some(basis) => {
            let output = command.capture(Some(basis), None)?;
            require_complete(&output, action).map_err(|error| {
                WorkcellError::ReconciliationFailed(format!("{error}; original instance basis retained: {basis}; effects uncertain; native stop not acknowledged by this failed command"))
            })?;
            let value = native_document(&output.stdout, STOP_SCHEMA)?;
            if string_field(&value, "generation")? != basis["generation"].as_str().unwrap()
                || digest_field(&value, "basis_sha256")? != basis["basis_sha256"].as_str().unwrap()
            {
                return Err(WorkcellError::ReconciliationFailed(format!("{action}: native retirement result differs from original instance; receipt retained")));
            }
            let mut result = json!({"schema": STOP_SCHEMA, "generation": value["generation"], "basis_sha256": value["basis_sha256"]});
            for key in ["retired", "acknowledged", "native_quiescence_verified"] {
                result[key] = json!(value.get(key).and_then(Value::as_bool).ok_or_else(|| {
                    WorkcellError::ReconciliationFailed(format!(
                        "{action}: native retirement result lacks `{key}`; effects uncertain"
                    ))
                })?);
            }
            let effect = string_field(&value, "stop_effect")?;
            if !matches!(effect, "stopped" | "already-retired" | "unknown") {
                return Err(WorkcellError::ReconciliationFailed(format!(
                    "{action}: unqualified native stop effect; receipt retained"
                )));
            }
            result["stop_effect"] = json!(effect);
            if value.get("evidence_path").is_some() || value.get("evidence_sha256").is_some() {
                let path = string_field(&value, "evidence_path")?;
                if !Path::new(path).is_absolute() {
                    return Err(WorkcellError::ReconciliationFailed(format!(
                        "{action}: retirement evidence is not absolute; receipt retained"
                    )));
                }
                result["evidence_path"] = json!(path);
                result["evidence_sha256"] = json!(digest_field(&value, "evidence_sha256")?);
            }
            if result["retired"] != true
                || result["acknowledged"] != true
                || result["native_quiescence_verified"] != true
                || effect == "unknown"
            {
                return Err(WorkcellError::ReconciliationFailed(format!("{action}: native stop remains unqualified: {result}; effects uncertain; original receipt retained")));
            }
            Ok(effect == "stopped")
        }
        None => run_required(command, action).map(|()| true),
    }
}

fn cleanup_readiness_failure(
    service: &ExternalManagedService,
    basis: Option<&Value>,
    failure: String,
) -> WorkcellError {
    match service
        .stop
        .as_ref()
        .map(|stop| run_bound_required(stop, basis, "readiness cleanup"))
    {
        Some(Ok(_)) => {
            WorkcellError::Unavailable(format!("{failure}; target-native cleanup succeeded"))
        }
        Some(Err(error)) => WorkcellError::CleanupFailed(format!(
            "{failure}; actual cleanup failure: {error}; effects retained for reconciliation"
        )),
        None => WorkcellError::ReconciliationFailed(format!(
            "{failure}; cleanup unavailable; effects retained for reconciliation"
        )),
    }
}

/// Default polling bounds for a command probe whose declaration does not
/// state its own window: long enough for a detached daemon to bind, short
/// enough that a genuinely failed start is still refused promptly.
const DEFAULT_READINESS_WINDOW_MS: u64 = 5_000;
const DEFAULT_READINESS_INTERVAL_MS: u64 = 200;

fn probe_becomes_healthy(
    command: &ExternalServiceCommand,
    timeout_ms: Option<u64>,
    interval_ms: Option<u64>,
) -> bool {
    let window = Duration::from_millis(timeout_ms.unwrap_or(DEFAULT_READINESS_WINDOW_MS));
    let interval = Duration::from_millis(interval_ms.unwrap_or(DEFAULT_READINESS_INTERVAL_MS));
    let deadline = Instant::now() + window;
    loop {
        if run_probe(command) == HealthState::Healthy {
            return true;
        }
        let now = Instant::now();
        if now >= deadline {
            return false;
        }
        thread::sleep(interval.min(deadline - now));
    }
}

fn run_required(command: &ExternalServiceCommand, action: &str) -> Result<()> {
    let status = command.run().map_err(|error| {
        WorkcellError::Unavailable(format!(
            "{action} via `{}` failed: {error}",
            command.display()
        ))
    })?;
    if status.success() {
        return Ok(());
    }
    Err(WorkcellError::OperationFailed(format!(
        "{action} via `{}` exited {status}; command output withheld",
        command.display()
    )))
}

#[cfg(test)]
mod instance_contract_compatibility {
    use super::*;

    #[test]
    fn absence_of_additive_contract_preserves_the_preimage_declaration_digest() {
        let service = ExternalManagedService::new(
            "service:x",
            "target-native://x",
            ExternalServiceCommand::new("controller").unwrap(),
        )
        .unwrap();
        // Frozen old derived-Debug bytes, used by already durable receipts.
        let old = r#"ExternalManagedService { logical_ref: "service:x", endpoint: "target-native://x", status: ExternalServiceCommand { program: "controller", args: [], environment: {} }, readiness: None, start: None, stop: None, restart: None, acquisition: ObserveExisting, readiness_timeout_ms: None, readiness_interval_ms: None, metadata: {} }"#;
        assert_eq!(format!("{service:?}"), old);
        assert_eq!(
            service.fingerprint(),
            format!("sha256:{:x}", Sha256::digest(old.as_bytes()))
        );
    }
}


#[cfg(all(test,unix))]
mod legacy_command_tests {
    use super::*;
    use crate::bounded_process::status_test_support::{self as support,Fixture};
    use crate::BoundedProcessFailure;
    use std::{fs,time::Instant};

    #[test]
    fn actual_legacy_ten_second_deadline_uses_owned_finite_group_retirement() {
        let name="external_service::legacy_command_tests::actual_legacy_ten_second_deadline_uses_owned_finite_group_retirement";
        if !support::isolated(name,Duration::from_secs(25)) {return;}
        use std::os::unix::process::ExitStatusExt;
        let fixture=Fixture::new("legacy-deadline");let script=fixture.script();
        let command=ExternalServiceCommand::new("python3").unwrap().with_arg("-S")
            .with_arg(script.to_str().unwrap())
            .with_env("WORKCELL_STATUS_FIXTURE_ROOT",fixture.root.to_str().unwrap()).unwrap()
            .with_env("WORKCELL_STATUS_PARENT_STAY","1").unwrap();
        let began=Instant::now();let error=command.run().unwrap_err();let elapsed=began.elapsed();
        assert!(elapsed>=Duration::from_secs(10)&&elapsed<Duration::from_secs(12),
            "actual legacy timeout elapsed {elapsed:?}; no claim of retirement outside the normal killable host gate");
        assert_eq!(error.kind(),std::io::ErrorKind::TimedOut);
        let failure=error.get_ref().and_then(|cause|cause.downcast_ref::<BoundedProcessFailure>())
            .expect("outer io must retain the original typed Child owner failure");
        assert_eq!(failure.kind(),crate::BoundedCaptureFailureKind::DeadlineElapsed);
        assert!(failure.timed_out());assert_eq!(failure.status().unwrap().signal(),Some(libc::SIGKILL));
        assert!(failure.stdout().is_empty()&&failure.stderr().is_empty()&&!failure.output_complete());
        let facts=failure.observation();assert_eq!(facts["output_capture_requested"],false);
        assert_eq!(facts["stdout_eof"],false);assert_eq!(facts["stderr_eof"],false);
        assert_eq!(facts["termination_request_accepted"],true);assert_eq!(facts["reaped_by_owner"],true);
        let parent=fixture.document("parent.json",Instant::now()+Duration::from_secs(2));
        let writer=fixture.document("writer.json",Instant::now()+Duration::from_secs(2));
        assert_eq!(parent["child"],writer["pid"]);assert_eq!(parent["pid"],writer["ppid"]);
        assert_eq!(parent["pid"],writer["pgid"]);
        let direct=support::wait_absent(parent["pid"].as_i64().unwrap() as libc::pid_t,&fixture.root,
            Instant::now()+Duration::from_secs(3),false);
        assert!(direct.is_none());
        let adopted=support::wait_absent(writer["pid"].as_i64().unwrap() as libc::pid_t,&fixture.root,
            Instant::now()+Duration::from_secs(3),true);
        #[cfg(target_os="linux")]
        assert!(adopted.is_some_and(|status|libc::WIFSIGNALED(status)&&libc::WTERMSIG(status)==libc::SIGKILL));
        #[cfg(not(target_os="linux"))]
        let _=adopted;
        assert!(!fixture.root.join("writer-result.json").exists(),"actual killed writer is not a successful late payload");
        fs::write(fixture.root.join("legacy-owner-failure.json"),serde_json::to_vec(&facts).unwrap()).unwrap();
        fixture.finish();
    }

    #[test]
    fn actual_missing_program_keeps_typed_io_cause_and_nonzero_keeps_status() {
        let fixture=Fixture::new("legacy-missing");
        let missing=fixture.root.join("nonexistent-native-program");assert!(!missing.exists());
        let command=ExternalServiceCommand::new(missing.to_str().unwrap()).unwrap()
            .with_arg("private-missing-argument").with_env("PRIVATE_VALUE","private-missing-environment").unwrap();
        let error=command.run().unwrap_err();assert_eq!(error.kind(),std::io::ErrorKind::NotFound);
        // The outer io wrapper has no invented OS errno; the actual error stays
        // in the retained typed owner and its Error::source chain.
        assert!(error.raw_os_error().is_none());
        let failure=error.get_ref().and_then(|cause|cause.downcast_ref::<BoundedProcessFailure>()).unwrap();
        assert_eq!(failure.kind(),crate::BoundedCaptureFailureKind::SpawnFailed);
        let native=failure.native_cause().unwrap();assert_eq!(native.kind(),std::io::ErrorKind::NotFound);
        assert_eq!(native.raw_os_error(),Some(libc::ENOENT));
        let source=std::error::Error::source(failure).unwrap().downcast_ref::<std::io::Error>().unwrap();
        assert_eq!(source.kind(),native.kind());assert_eq!(source.raw_os_error(),native.raw_os_error());
        assert!(failure.status().is_none()&&!failure.spawned()&&failure.spawn_attempted());
        assert_eq!(failure.observation()["output_capture_requested"],false);
        let displayed=error.to_string();let debug=format!("{failure:?}");
        for private in ["private-missing-argument","private-missing-environment"] {
            assert!(!displayed.contains(private)&&!debug.contains(private));
        }
        let nonzero=ExternalServiceCommand::new("python3").unwrap().with_arg("-S").with_arg("-c")
            .with_arg("import os;os.write(1,b'private-output');os.write(2,b'private-error');os._exit(7)")
            .run().unwrap();
        assert_eq!(nonzero.code(),Some(7));
        fs::write(fixture.root.join("missing-program-facts.json"),serde_json::to_vec(&failure.observation()).unwrap()).unwrap();
        fixture.finish();
    }

    #[test]
    fn actual_legacy_probe_required_action_and_optional_none_keep_their_contract() {
        let fixture=Fixture::new("legacy-optional");
        let healthy=ExternalServiceCommand::new("/usr/bin/true").unwrap();
        let failed=ExternalServiceCommand::new("/usr/bin/false").unwrap();
        let absent=ExternalServiceCommand::new(fixture.root.join("missing").to_str().unwrap()).unwrap();
        assert_eq!(run_probe(&healthy),HealthState::Healthy);
        assert_eq!(run_probe(&failed),HealthState::Unavailable);
        assert_eq!(run_probe(&absent),HealthState::Unavailable);
        assert!(run_required(&healthy,"actual status-only operation").is_ok());
        assert!(matches!(run_required(&failed,"actual status-only operation"),Err(WorkcellError::OperationFailed(_))));
        assert!(matches!(run_required(&absent,"actual status-only operation"),Err(WorkcellError::Unavailable(_))));
        // A real declared None provider drives an owned listener. Gates are
        // control inputs only; health/lifetime come from actual TCP and Child facts.
        let script=fixture.root.join("optional-listener.py");
        fs::write(&script,r#"import json,os,pathlib,socket,sys,time
root=pathlib.Path(sys.argv[2]);port=int(sys.argv[3]);operation=sys.argv[1]
def record(name,value):
    stage=root/(name+'.stage-'+str(os.getpid()))
    with stage.open('x') as out:json.dump(value,out);out.flush();os.fsync(out.fileno())
    assert not (root/name).exists();os.rename(stage,root/name)
def probe(kind):
    try:
        with socket.create_connection(('127.0.0.1',port),.2) as client:
            client.settimeout(.2);client.sendall(kind.encode()+b'\n')
            response=b''
            while not response.endswith(b'\n') and len(response)<64:
                part=client.recv(64-len(response))
                if not part:return False
                response+=part
            return response==(b'running\n' if kind=='status' else b'ready\n')
    except (OSError,TimeoutError):return False
if operation=='serve':
    with socket.socket() as server:
        server.bind(('127.0.0.1',0));server.settimeout(.05)
        port=server.getsockname()[1]
        record('server.json',{'pid':os.getpid(),'pgid':os.getpgrp(),'port':port})
        deadline=time.monotonic()+20
        while not (root/'start').exists() and not (root/'release').exists() and time.monotonic()<deadline:time.sleep(.005)
        if not (root/'start').exists():sys.exit(3)
        server.listen();ready_at=time.monotonic()+.15
        while not (root/'stop').exists() and not (root/'release').exists() and time.monotonic()<deadline:
            try:client,_=server.accept()
            except socket.timeout:continue
            with client:
                client.settimeout(.2);request=b''
                while not request.endswith(b'\n') and len(request)<64:
                    part=client.recv(64-len(request))
                    if not part:break
                    request+=part
                request=request.strip()
                response=b'running\n' if request==b'status' else (b'ready\n' if time.monotonic()>=ready_at else b'not-ready\n')
                client.sendall(response)
        record('server-retired.json',{'pid':os.getpid(),'stopped':(root/'stop').exists(),'released':(root/'release').exists()})
        sys.exit(0)
with (root/'actual-commands.log').open('a') as log:log.write(operation+'\n');log.flush()
if operation in ('status','ready'):sys.exit(0 if probe(operation) else 1)
if operation=='start':
    with (root/'start').open('x') as out:out.write('start this exact owned listener')
    sys.exit(0)
if operation=='stop':
    with (root/'stop').open('x') as out:out.write('stop this exact owned listener')
    deadline=time.monotonic()+2
    while probe('status') and time.monotonic()<deadline:time.sleep(.005)
    sys.exit(1 if probe('status') else 0)
raise RuntimeError('unrecognised owned fixture operation')
"#).unwrap();
        let mut daemon=Command::new("python3");
        daemon.arg("-S").arg(&script).arg("serve").arg(&fixture.root).arg("0")
            .stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::piped());
        let mut child=support::OwnedChild::spawn(daemon,&fixture.root);let actual_pid=child.id();
        let server=fixture.document("server.json",Instant::now()+Duration::from_secs(2));
        assert_eq!(server["pid"],actual_pid);assert_eq!(server["pgid"],actual_pid);
        let port=u16::try_from(server["port"].as_u64().expect("actual bound listener port must be an unsigned integer"))
            .expect("actual bound listener port must fit u16");
        assert_ne!(port,0,"bootstrap zero is not a service endpoint");
        let occupied=std::net::TcpListener::bind(("127.0.0.1",port))
            .expect_err("same real child must still hold its published listener before start");
        assert_eq!(occupied.kind(),std::io::ErrorKind::AddrInUse);
        let command=|operation:&str| ExternalServiceCommand::new("python3").unwrap().with_arg("-S")
            .with_arg(script.to_str().unwrap()).with_arg(operation).with_arg(fixture.root.to_str().unwrap()).with_arg(port.to_string());
        let service=ExternalManagedService::new("service:null-command",format!("http://127.0.0.1:{port}"),command("status")).unwrap()
            .with_start(command("start")).with_stop(command("stop")).with_readiness(command("ready"))
            .with_readiness_timing(2_000,20);
        assert!(service.target_instance.is_none());assert_eq!(service.acquisition,ExternalServiceAcquisition::ObserveExisting);
        let request=|label:&str| ServiceMaterialRequest {
            demand_ref:epilogos_workcell_core::DemandRef::new(format!("demand:{label}")).unwrap(),
            connection:epilogos_workcell_core::LogicalConnectionRequirement::new("service:null-command").unwrap(),
            persistence:None,retention:RetentionExpectation::Release,
        };
        let mut observed=ExternalManagedServiceProvider::new(ProviderRef::new("provider:observed-native-command").unwrap(),[service.clone()]).unwrap();
        let offers=observed.offers().unwrap();assert_eq!(offers.len(),1);
        assert_eq!(offers[0].health,HealthState::Unavailable);assert_eq!(offers[0].availability,Availability::Unavailable);
        assert!(matches!(observed.resolve_service(&request("unavailable-observation")),Err(WorkcellError::Unavailable(_))));
        assert!(!fixture.root.join("start").exists());assert!(!fixture.root.join("stop").exists());
        let ensured=service.with_acquisition(ExternalServiceAcquisition::EnsureRunning);
        let mut owner=ExternalManagedServiceProvider::new(ProviderRef::new("provider:ensured-native-command").unwrap(),[ensured]).unwrap();
        let offers=owner.offers().unwrap();assert_eq!(offers.len(),1);assert_eq!(offers[0].availability,Availability::Degraded);
        let allocation=owner.resolve_service(&request("owned-listener")).unwrap();
        assert_eq!(allocation.health,HealthState::Healthy);assert_eq!(allocation.properties["started_by_provider"],"true");
        assert!(!allocation.properties.contains_key("target_instance_basis"));
        assert_eq!(owner.observe_service(&allocation).unwrap().health,HealthState::Healthy);
        let repeated=owner.resolve_service(&request("owned-listener")).unwrap();
        assert_eq!(repeated,allocation);assert_eq!(repeated.properties["started_by_provider"],"true");
        let commands=fs::read_to_string(fixture.root.join("actual-commands.log")).unwrap();
        assert_eq!(commands.lines().filter(|line| *line=="start").count(),1);
        assert!(commands.lines().any(|line| line=="ready"));
        let observer=observed.resolve_service(&request("existing-observer")).unwrap();
        assert_eq!(observer.properties["started_by_provider"],"false");
        assert_eq!(observed.observe_service(&observer).unwrap().health,HealthState::Healthy);
        assert!(!observed.release_service(&observer,&RetentionExpectation::Release).unwrap().changed);
        assert!(!fixture.root.join("stop").exists());
        assert_eq!(owner.observe_service(&allocation).unwrap().health,HealthState::Healthy);
        let dependent=owner.resolve_service(&request("dependent-listener")).unwrap();
        assert_eq!(dependent.properties["started_by_provider"],"false");
        assert!(matches!(owner.release_service(&allocation,&RetentionExpectation::Release),Err(WorkcellError::OperationFailed(_))));
        assert!(!fixture.root.join("stop").exists());
        assert_eq!(owner.observe_service(&allocation).unwrap().health,HealthState::Healthy);
        assert!(!owner.release_service(&dependent,&RetentionExpectation::Release).unwrap().changed);
        assert!(owner.release_service(&allocation,&RetentionExpectation::Release).unwrap().changed);
        let retired=fixture.document("server-retired.json",Instant::now()+Duration::from_secs(2));
        assert_eq!(retired["pid"],actual_pid);assert_eq!(retired["stopped"],true);assert_eq!(retired["released"],false);
        let output=child.complete(Duration::from_secs(2)).unwrap();
        assert!(output.status.success()&&!output.timed_out&&output.output_complete&&!output.output_truncated);
        assert!(support::wait_absent(actual_pid as libc::pid_t,&fixture.root,Instant::now()+Duration::from_secs(2),false).is_none());
        assert_eq!(run_probe(&command("status")),HealthState::Unavailable);
        assert_eq!(fs::read_to_string(fixture.root.join("actual-commands.log")).unwrap().lines().filter(|line| *line=="stop").count(),1);
        fs::write(fixture.root.join("optional-daemon.stdout"),&output.stdout).unwrap();
        fs::write(fixture.root.join("optional-daemon.stderr"),&output.stderr).unwrap();
        // None preserves legacy command ownership. These actual process and
        // endpoint facts do not mint a strict Some target-native instance witness.
        fixture.finish();
    }
}
