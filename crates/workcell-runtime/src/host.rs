use std::{collections::BTreeMap, process::Command};

use epilogos_workcell_core::{
    validate_allocation, Availability, ExecutionMaterialRequest, ExecutionProvider, HealthState,
    OfferRef, OperationalOffer, ProviderAllocation, ProviderObservation, ProviderOperation,
    ProviderOperationResult, ProviderPort, ProviderPortKind, ProviderRef, ProviderReleaseResult,
    ReleaseDisposition, Result, RetentionExpectation, WorkcellError,
};

use crate::support::stable_key;

const HOST_AFFORDANCES: &[&str] = &["shell", "process-execution", "execution:host-process"];

/// Zero-setup execution provider backed by ordinary host processes.
///
/// The provider deliberately advertises no stronger isolation, snapshot, or
/// network property than the host can guarantee generically. A process handle
/// is material provenance and never caller semantic identity.
#[derive(Clone, Debug)]
pub struct HostProcessExecutionProvider {
    provider_ref: ProviderRef,
}

impl HostProcessExecutionProvider {
    pub fn new(provider_ref: ProviderRef) -> Self {
        Self { provider_ref }
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
                "host-process execution does not claim generic network reachability".into(),
            ));
        }
        if request
            .isolation_trust
            .as_ref()
            .is_some_and(|requirement| requirement.as_str() != "host-process")
        {
            return Err(WorkcellError::UnsatisfiedDemand(
                "host-process execution cannot satisfy stronger isolation".into(),
            ));
        }

        let key = stable_key(&[request.demand_ref.as_str(), self.provider_ref.as_str()]);
        let material_ref = format!("execution:host-process:{key}");
        let mut properties = BTreeMap::new();
        properties.insert("execution_kind".into(), "host-process".into());
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
        match retention {
            RetentionExpectation::Release => Ok(ProviderReleaseResult {
                provider_ref: self.provider_ref.clone(),
                material_ref: allocation.material_ref.clone(),
                disposition: ReleaseDisposition::Released,
                changed: true,
            }),
            RetentionExpectation::Preserve => Ok(ProviderReleaseResult {
                provider_ref: self.provider_ref.clone(),
                material_ref: allocation.material_ref.clone(),
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

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use epilogos_workcell_core::{DemandRef, ProviderOperation};

    #[test]
    fn executes_a_real_host_process_without_shell_string_interpolation() {
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
        let result = provider
            .execute_operation(
                &allocation,
                &ProviderOperation {
                    key: "process".into(),
                    parameters: BTreeMap::from([
                        ("program".into(), "/bin/sh".into()),
                        ("arg.0".into(), "-c".into()),
                        ("arg.1".into(), "printf workcell".into()),
                    ]),
                },
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
    }
}
