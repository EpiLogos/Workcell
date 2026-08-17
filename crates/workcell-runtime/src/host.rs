use std::{
    collections::BTreeMap,
    process::Command,
    time::{SystemTime, UNIX_EPOCH},
};

use epilogos_workcell_core::{
    validate_allocation, Availability, ExecutionMaterialRequest, ExecutionProvider, HealthState,
    OfferRef, OperationalOffer, ProviderAllocation, ProviderObservation, ProviderOperation,
    ProviderOperationResult, ProviderPort, ProviderPortKind, ProviderRef, ProviderReleaseResult,
    ReleaseDisposition, Result, RetentionExpectation, WorkcellError,
};

use crate::support::stable_key;

const HOST_AFFORDANCES: &[&str] = &["shell", "process-execution", "execution:host-process"];

/// Finite material capability for one already-allocated trusted host-process body.
///
/// This is deliberately not semantic Action/Agency authority. The Workcell control
/// plane may register it only after the external authority owner has resolved the
/// act. The provider then enforces the exact material operation at the point where
/// `std::process::Command` would otherwise be reachable.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HostProcessOperationGrant {
    pub grant_ref: String,
    pub authority_ref: String,
    pub material_ref: String,
    pub operation_key: String,
    pub program: String,
    pub args: Vec<String>,
    pub cwd: Option<String>,
    pub expires_at_unix_ms: u64,
    pub max_uses: u32,
}

#[derive(Clone, Debug)]
struct StoredHostProcessGrant {
    grant: HostProcessOperationGrant,
    uses: u32,
    revoked: bool,
    operations: BTreeMap<String, String>,
}

/// Zero-setup execution provider backed by ordinary host processes.
///
/// The provider deliberately advertises no stronger isolation, snapshot, or
/// network property than the host can guarantee generically. A process handle
/// is material provenance and never caller semantic identity. Host-process
/// allocation is not execution authority: every real `Command` requires a finite
/// material operation grant registered by the trusted Workcell control plane.
#[derive(Clone, Debug)]
pub struct HostProcessExecutionProvider {
    provider_ref: ProviderRef,
    operation_grants: BTreeMap<String, StoredHostProcessGrant>,
}

impl HostProcessExecutionProvider {
    pub fn new(provider_ref: ProviderRef) -> Self {
        Self {
            provider_ref,
            operation_grants: BTreeMap::new(),
        }
    }

    pub fn register_operation_grant(
        &mut self,
        allocation: &ProviderAllocation,
        grant: HostProcessOperationGrant,
    ) -> Result<()> {
        validate_allocation(self, allocation)?;
        if grant.grant_ref.trim().is_empty() || grant.authority_ref.trim().is_empty() {
            return Err(WorkcellError::InvalidDemand(
                "host-process operation grant requires stable grant_ref and external authority_ref"
                    .into(),
            ));
        }
        if grant.material_ref != allocation.material_ref {
            return Err(WorkcellError::InvalidDemand(
                "host-process operation grant is bound to a different material allocation".into(),
            ));
        }
        if grant.operation_key != "process" {
            return Err(WorkcellError::InvalidDemand(
                "host-process operation grant must bind operation `process`".into(),
            ));
        }
        if grant.program.trim().is_empty() || grant.max_uses == 0 {
            return Err(WorkcellError::InvalidDemand(
                "host-process operation grant requires a program and non-zero use budget".into(),
            ));
        }
        if grant.expires_at_unix_ms <= now_unix_ms()? {
            return Err(WorkcellError::InvalidDemand(
                "host-process operation grant is already expired".into(),
            ));
        }
        if self.operation_grants.contains_key(&grant.grant_ref) {
            return Err(WorkcellError::InvalidDemand(format!(
                "host-process operation grant `{}` is already registered",
                grant.grant_ref
            )));
        }
        self.operation_grants.insert(
            grant.grant_ref.clone(),
            StoredHostProcessGrant {
                grant,
                uses: 0,
                revoked: false,
                operations: BTreeMap::new(),
            },
        );
        Ok(())
    }

    pub fn revoke_operation_grant(&mut self, grant_ref: &str) -> Result<()> {
        let stored = self.operation_grants.get_mut(grant_ref).ok_or_else(|| {
            WorkcellError::InvalidDemand(format!(
                "unknown host-process operation grant `{grant_ref}`"
            ))
        })?;
        stored.revoked = true;
        Ok(())
    }
}

impl ProviderPort for HostProcessExecutionProvider {
    fn provider_ref(&self) -> &ProviderRef {
        &self.provider_ref
    }

    fn port_kind(&self) -> ProviderPortKind {
        ProviderPortKind::Execution
    }

    fn offers(&self) -> Result<Vec<OperationalOffer>> {
        let mut metadata = BTreeMap::new();
        metadata.insert("implementation".into(), "host-process".into());
        metadata.insert("operation_authority".into(), "required".into());
        metadata.insert(
            "untrusted_isolation".into(),
            "unsupported-use-isolated-provider".into(),
        );
        Ok(vec![OperationalOffer {
            offer_ref: OfferRef::new(format!("offer:{}:host-process", self.provider_ref))
                .map_err(|error| WorkcellError::OperationFailed(error.into()))?,
            provider_ref: self.provider_ref.clone(),
            port: ProviderPortKind::Execution.as_str().into(),
            affordances: vec![
                "shell".into(),
                "process-execution".into(),
                "execution:host-process".into(),
                "persistence:ephemeral".into(),
                "retention:preserve".into(),
            ],
            connections: vec![],
            exposures: vec![],
            isolation_trust: vec!["host-process".into()],
            availability: Availability::Available,
            health: HealthState::Healthy,
            capacity: BTreeMap::new(),
            metadata,
        }])
    }
}

impl ExecutionProvider for HostProcessExecutionProvider {
    fn prepare_execution(
        &mut self,
        request: &ExecutionMaterialRequest,
    ) -> Result<ProviderAllocation> {
        for affordance in &request.affordances {
            if !HOST_AFFORDANCES.contains(&affordance.as_str()) {
                return Err(WorkcellError::UnsatisfiedDemand(format!(
                    "host-process execution does not provide affordance `{affordance}`"
                )));
            }
        }
        if !request.connectivity.is_empty() {
            return Err(WorkcellError::UnsatisfiedDemand(
                "host-process execution does not claim or enforce generic network reachability"
                    .into(),
            ));
        }
        if request
            .isolation_trust
            .as_ref()
            .is_some_and(|requirement| requirement.as_str() != "host-process")
        {
            return Err(WorkcellError::UnsatisfiedDemand(
                "host-process execution cannot satisfy stronger isolation; use an isolated provider"
                    .into(),
            ));
        }

        let key = stable_key(&[request.demand_ref.as_str(), self.provider_ref.as_str()]);
        let material_ref = format!("execution:host-process:{key}");
        let mut properties = BTreeMap::new();
        properties.insert("execution_kind".into(), "host-process".into());
        properties.insert("operation_authority".into(), "required".into());
        let mut provenance = BTreeMap::new();
        provenance.insert("implementation".into(), "host-process".into());
        provenance.insert("provider_ref".into(), self.provider_ref.to_string());

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
        validate_allocation(self, allocation)?;
        if operation.key != "process" {
            return Err(WorkcellError::Unsupported(format!(
                "host-process provider does not implement operation `{}`",
                operation.key
            )));
        }

        let grant_ref = operation.parameters.get("authority_ref").ok_or_else(|| {
            WorkcellError::OperationFailed(
                "host-process execution denied before side effect: explicit material authority is required"
                    .into(),
            )
        })?;
        let operation_id = operation.parameters.get("operation_id").ok_or_else(|| {
            WorkcellError::OperationFailed(
                "host-process execution denied before side effect: operation_id is required".into(),
            )
        })?;
        let program = operation.parameters.get("program").ok_or_else(|| {
            WorkcellError::InvalidDemand("process operation requires `program`".into())
        })?;
        let mut indexed_args = operation
            .parameters
            .iter()
            .filter_map(|(key, value)| {
                key.strip_prefix("arg.")
                    .and_then(|index| index.parse::<usize>().ok())
                    .map(|index| (index, value))
            })
            .collect::<Vec<_>>();
        indexed_args.sort_by_key(|(index, _)| *index);
        for pair in indexed_args.windows(2) {
            if pair[0].0 == pair[1].0 {
                return Err(WorkcellError::InvalidDemand(
                    "process operation contains duplicate argument indexes".into(),
                ));
            }
        }
        let args = indexed_args
            .iter()
            .map(|(_, value)| (*value).clone())
            .collect::<Vec<_>>();
        let cwd = operation.parameters.get("cwd").cloned();
        let fingerprint = operation_fingerprint(program, &args, cwd.as_deref());
        let now = now_unix_ms()?;

        let stored = self.operation_grants.get_mut(grant_ref).ok_or_else(|| {
            WorkcellError::OperationFailed(format!(
                "host-process execution denied before side effect: unknown material authority `{grant_ref}`"
            ))
        })?;
        if stored.revoked {
            return Err(WorkcellError::OperationFailed(format!(
                "host-process material authority `{grant_ref}` is revoked"
            )));
        }
        if stored.grant.material_ref != allocation.material_ref
            || stored.grant.operation_key != operation.key
            || stored.grant.program != *program
            || stored.grant.args != args
            || stored.grant.cwd != cwd
        {
            return Err(WorkcellError::OperationFailed(
                "host-process execution denied before side effect: operation does not match exact material authority"
                    .into(),
            ));
        }
        if now >= stored.grant.expires_at_unix_ms {
            return Err(WorkcellError::OperationFailed(format!(
                "host-process material authority `{grant_ref}` is expired"
            )));
        }
        if let Some(previous) = stored.operations.get(operation_id) {
            if previous == &fingerprint {
                return Err(WorkcellError::OperationFailed(format!(
                    "host-process operation `{operation_id}` was already consumed and is not declared retry-safe"
                )));
            }
            return Err(WorkcellError::OperationFailed(format!(
                "host-process operation `{operation_id}` conflicts with an already consumed request"
            )));
        }
        if stored.uses >= stored.grant.max_uses {
            return Err(WorkcellError::OperationFailed(format!(
                "host-process material authority `{grant_ref}` is exhausted"
            )));
        }

        // Consume before the side effect. A spawn/exec failure does not restore
        // authority and therefore cannot be turned into an amplification loop.
        stored.uses += 1;
        stored
            .operations
            .insert(operation_id.clone(), fingerprint);
        let external_authority_ref = stored.grant.authority_ref.clone();

        let mut command = Command::new(program);
        command.args(indexed_args.iter().map(|(_, value)| value.as_str()));
        if let Some(cwd) = operation.parameters.get("cwd") {
            command.current_dir(cwd);
        }
        let result = command.output().map_err(|error| {
            WorkcellError::OperationFailed(format!("execute host process `{program}`: {error}"))
        })?;

        let mut output = BTreeMap::new();
        output.insert(
            "exit_code".into(),
            result
                .status
                .code()
                .map_or_else(|| "signal".into(), |code| code.to_string()),
        );
        output.insert("success".into(), result.status.success().to_string());
        output.insert(
            "stdout".into(),
            String::from_utf8_lossy(&result.stdout).into_owned(),
        );
        output.insert(
            "stderr".into(),
            String::from_utf8_lossy(&result.stderr).into_owned(),
        );
        let mut provenance = allocation.provenance.clone();
        provenance.insert("operation".into(), "process".into());
        provenance.insert("material_authority_ref".into(), grant_ref.clone());
        provenance.insert("external_authority_ref".into(), external_authority_ref);
        provenance.insert("operation_id".into(), operation_id.clone());

        Ok(ProviderOperationResult {
            provider_ref: self.provider_ref.clone(),
            material_ref: allocation.material_ref.clone(),
            operation: operation.key.clone(),
            output,
            provenance,
        })
    }

    fn observe_execution(&self, allocation: &ProviderAllocation) -> Result<ProviderObservation> {
        validate_allocation(self, allocation)?;
        let mut detail = BTreeMap::new();
        detail.insert("execution_kind".into(), "host-process".into());
        detail.insert("available".into(), "true".into());
        detail.insert("operation_authority".into(), "required".into());
        Ok(ProviderObservation {
            provider_ref: self.provider_ref.clone(),
            material_ref: allocation.material_ref.clone(),
            health: HealthState::Healthy,
            detail,
        })
    }

    fn release_execution(
        &mut self,
        allocation: &ProviderAllocation,
        retention: &RetentionExpectation,
    ) -> Result<ProviderReleaseResult> {
        validate_allocation(self, allocation)?;
        let material_ref = allocation.material_ref.clone();
        for stored in self.operation_grants.values_mut() {
            if stored.grant.material_ref == material_ref {
                stored.revoked = true;
            }
        }
        match retention {
            RetentionExpectation::Release => Ok(ProviderReleaseResult {
                provider_ref: self.provider_ref.clone(),
                material_ref,
                disposition: ReleaseDisposition::Released,
                changed: true,
            }),
            RetentionExpectation::Preserve => Ok(ProviderReleaseResult {
                provider_ref: self.provider_ref.clone(),
                material_ref,
                disposition: ReleaseDisposition::Preserved,
                changed: false,
            }),
            RetentionExpectation::SuspendIfSupported
            | RetentionExpectation::SnapshotIfSupported => Err(WorkcellError::Unsupported(
                "host-process execution does not support suspend or snapshot".into(),
            )),
        }
    }
}

fn operation_fingerprint(program: &str, args: &[String], cwd: Option<&str>) -> String {
    format!("{}|{}|{}", program, args.join("\u{1f}"), cwd.unwrap_or("-"))
}

fn now_unix_ms() -> Result<u64> {
    let duration = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|error| WorkcellError::OperationFailed(format!("system clock error: {error}")))?;
    u64::try_from(duration.as_millis())
        .map_err(|_| WorkcellError::OperationFailed("system time exceeds u64 milliseconds".into()))
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use epilogos_workcell_core::{DemandRef, ProviderOperation};

    fn process_operation(authority_ref: Option<&str>, operation_id: &str) -> ProviderOperation {
        let mut parameters = BTreeMap::from([
            ("program".into(), "/bin/sh".into()),
            ("arg.0".into(), "-c".into()),
            ("arg.1".into(), "printf workcell".into()),
            ("operation_id".into(), operation_id.into()),
        ]);
        if let Some(authority_ref) = authority_ref {
            parameters.insert("authority_ref".into(), authority_ref.into());
        }
        ProviderOperation {
            key: "process".into(),
            parameters,
        }
    }

    #[test]
    fn allocation_and_operation_discovery_do_not_authorise_a_real_host_process() {
        let mut provider = HostProcessExecutionProvider::new(
            ProviderRef::new("provider:test-host-process").unwrap(),
        );
        let allocation = provider
            .prepare_execution(&ExecutionMaterialRequest {
                demand_ref: DemandRef::new("demand:host-process-test").unwrap(),
                affordances: vec!["shell".into()],
                resources: vec![],
                connectivity: vec![],
                isolation_trust: None,
                retention: RetentionExpectation::Release,
            })
            .unwrap();

        let denied = provider
            .execute_operation(&allocation, &process_operation(None, "operation/no-authority"))
            .unwrap_err();
        assert!(denied.to_string().contains("explicit material authority"));

        provider
            .register_operation_grant(
                &allocation,
                HostProcessOperationGrant {
                    grant_ref: "material-authority/host-process-test".into(),
                    authority_ref: "aikit/execution-authority/test".into(),
                    material_ref: allocation.material_ref.clone(),
                    operation_key: "process".into(),
                    program: "/bin/sh".into(),
                    args: vec!["-c".into(), "printf workcell".into()],
                    cwd: None,
                    expires_at_unix_ms: u64::MAX,
                    max_uses: 1,
                },
            )
            .unwrap();
        let result = provider
            .execute_operation(
                &allocation,
                &process_operation(
                    Some("material-authority/host-process-test"),
                    "operation/authorised",
                ),
            )
            .unwrap();
        assert_eq!(
            result.output.get("success").map(String::as_str),
            Some("true")
        );
        assert_eq!(
            result.output.get("stdout").map(String::as_str),
            Some("workcell")
        );
        assert_eq!(
            result
                .provenance
                .get("external_authority_ref")
                .map(String::as_str),
            Some("aikit/execution-authority/test")
        );

        let replay = provider
            .execute_operation(
                &allocation,
                &process_operation(
                    Some("material-authority/host-process-test"),
                    "operation/authorised",
                ),
            )
            .unwrap_err();
        assert!(replay.to_string().contains("already consumed"));
    }
}
