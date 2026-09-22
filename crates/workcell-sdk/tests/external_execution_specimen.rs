//! An external-style execution provider specimen, proven end to end.
//!
//! This is what a provider author outside the Workcell tree writes: a crate
//! that depends only on `epilogos-workcell-sdk`, implements the execution
//! port over real technology (here: shell processes in per-demand working
//! directories), and proves the full lifecycle — verify the port, prepare a
//! world, execute useful work, observe it, release it under retention
//! semantics, and refuse what it does not support. The 2026-09-20 SDK
//! campaign's external-boundary law in miniature: no private imports, no
//! registry hand-edits, no fixture pretending to be live use.

use std::collections::BTreeMap;
use std::fs;
use std::path::PathBuf;
use std::process::Command;
use std::sync::atomic::{AtomicU64, Ordering};

use epilogos_workcell_sdk::contract::{
    DemandRef, ExecutionDemand, ExternalRef, RetentionExpectation,
};
use epilogos_workcell_sdk::provider::{
    validate_allocation, Availability, ExecutionMaterialRequest, ExecutionProvider, HealthState,
    OfferRef, OperationalOffer, ProviderAllocation, ProviderObservation, ProviderOperation,
    ProviderOperationResult, ProviderPort, ProviderPortKind, ProviderRef, ProviderReleaseResult,
    WorkcellError,
};
use epilogos_workcell_sdk::testkit::verify_provider_port;

/// A std-only scratch root per test: the specimen adds no dependency to the
/// SDK crate, because an external author's proof should not cost the SDK
/// anything.
fn scratch_dir(label: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "workcell-sdk-specimen-{}-{}",
        label,
        std::process::id()
    ));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap();
    dir
}

/// A directory-process executor: every demand gets a fresh working directory
/// under a root, operations run as shell commands inside it, observation
/// reports what is on disk, and release honours the caller's retention
/// expectation — removing only what the provider itself allocated.
struct ProcessDirProvider {
    provider_ref: ProviderRef,
    root: PathBuf,
    seq: AtomicU64,
}

impl ProcessDirProvider {
    fn at_root(root: impl Into<PathBuf>) -> Result<Self, &'static str> {
        Ok(Self {
            provider_ref: ProviderRef::new("provider:external/process-dir")?,
            root: root.into(),
            seq: AtomicU64::new(0),
        })
    }

    fn workdir(&self, allocation: &ProviderAllocation) -> PathBuf {
        PathBuf::from(
            allocation
                .properties
                .get("workdir")
                .cloned()
                .unwrap_or_default(),
        )
    }
}

impl ProviderPort for ProcessDirProvider {
    fn provider_ref(&self) -> &ProviderRef {
        &self.provider_ref
    }

    fn port_kind(&self) -> ProviderPortKind {
        ProviderPortKind::Execution
    }

    fn offers(&self) -> epilogos_workcell_sdk::provider::Result<Vec<OperationalOffer>> {
        Ok(vec![OperationalOffer {
            offer_ref: OfferRef::new("offer:process-dir/shell")?,
            provider_ref: self.provider_ref.clone(),
            port: self.port_kind().as_str().to_string(),
            affordances: vec!["process.execute".to_string()],
            connections: vec![],
            exposures: vec![],
            isolation_trust: vec![],
            availability: Availability::Available,
            health: HealthState::Healthy,
            capacity: BTreeMap::new(),
            metadata: BTreeMap::from([(
                "engine".to_string(),
                "host shell via /bin/sh -c".to_string(),
            )]),
        }])
    }
}

impl ExecutionProvider for ProcessDirProvider {
    fn prepare_execution(
        &mut self,
        request: &ExecutionMaterialRequest,
    ) -> epilogos_workcell_sdk::provider::Result<ProviderAllocation> {
        let id = self.seq.fetch_add(1, Ordering::SeqCst);
        let workdir = self
            .root
            .join(format!("{}-{id}", request.demand_ref.as_str()));
        fs::create_dir_all(&workdir).map_err(|error| {
            WorkcellError::OperationFailed(format!(
                "could not create workdir {}: {error}",
                workdir.display()
            ))
        })?;
        Ok(ProviderAllocation {
            provider_ref: self.provider_ref.clone(),
            port: self.port_kind(),
            material_ref: format!("process-dir/{id}"),
            health: HealthState::Healthy,
            properties: BTreeMap::from([("workdir".to_string(), workdir.display().to_string())]),
            provenance: BTreeMap::from([
                ("provider".to_string(), "process-dir".to_string()),
                (
                    "demand".to_string(),
                    request.demand_ref.as_str().to_string(),
                ),
            ]),
        })
    }

    fn execute_operation(
        &mut self,
        allocation: &ProviderAllocation,
        operation: &ProviderOperation,
    ) -> epilogos_workcell_sdk::provider::Result<ProviderOperationResult> {
        validate_allocation(self, allocation)?;
        let workdir = self.workdir(allocation);
        if !workdir.is_dir() {
            return Err(WorkcellError::Unavailable(format!(
                "allocated world {} is gone",
                workdir.display()
            )));
        }
        let command = operation.parameters.get("command").ok_or_else(|| {
            WorkcellError::InvalidDemand(
                "process-dir operations carry the shell command in the `command` parameter"
                    .to_string(),
            )
        })?;
        let output = Command::new("/bin/sh")
            .arg("-c")
            .arg(command)
            .current_dir(&workdir)
            .output()
            .map_err(|error| {
                WorkcellError::OperationFailed(format!("could not spawn /bin/sh: {error}"))
            })?;
        let mut result = ProviderOperationResult {
            provider_ref: self.provider_ref.clone(),
            material_ref: allocation.material_ref.clone(),
            operation: operation.key.clone(),
            output: BTreeMap::from([
                (
                    "exit".to_string(),
                    output.status.code().unwrap_or(-1).to_string(),
                ),
                (
                    "stdout".to_string(),
                    String::from_utf8_lossy(&output.stdout).to_string(),
                ),
                (
                    "stderr".to_string(),
                    String::from_utf8_lossy(&output.stderr).to_string(),
                ),
            ]),
            provenance: BTreeMap::from([("engine".to_string(), "/bin/sh".to_string())]),
        };
        if !output.status.success() {
            result
                .provenance
                .insert("health".to_string(), "degraded".to_string());
        }
        Ok(result)
    }

    fn observe_execution(
        &self,
        allocation: &ProviderAllocation,
    ) -> epilogos_workcell_sdk::provider::Result<ProviderObservation> {
        validate_allocation(self, allocation)?;
        let workdir = self.workdir(allocation);
        let (health, entries) = match fs::read_dir(&workdir) {
            Ok(entries) => (HealthState::Healthy, entries.count()),
            Err(_) => (HealthState::Unavailable, 0),
        };
        Ok(ProviderObservation {
            provider_ref: self.provider_ref.clone(),
            material_ref: allocation.material_ref.clone(),
            health,
            detail: BTreeMap::from([
                ("workdir".to_string(), workdir.display().to_string()),
                ("entries".to_string(), entries.to_string()),
            ]),
        })
    }

    fn release_execution(
        &mut self,
        allocation: &ProviderAllocation,
        retention: &RetentionExpectation,
    ) -> epilogos_workcell_sdk::provider::Result<ProviderReleaseResult> {
        validate_allocation(self, allocation)?;
        let workdir = self.workdir(allocation);
        let disposition = match retention {
            RetentionExpectation::Release => {
                if workdir.is_dir() {
                    fs::remove_dir_all(&workdir).map_err(|error| {
                        WorkcellError::CleanupFailed(format!(
                            "could not remove {}: {error}",
                            workdir.display()
                        ))
                    })?;
                }
                epilogos_workcell_sdk::contract::ReleaseDisposition::Released
            }
            RetentionExpectation::Preserve => {
                epilogos_workcell_sdk::contract::ReleaseDisposition::Preserved
            }
            // A provider that cannot honour an expectation refuses it rather
            // than silently keeping or dropping the world.
            RetentionExpectation::SuspendIfSupported
            | RetentionExpectation::SnapshotIfSupported => {
                return Err(WorkcellError::OperationFailed(
                    "process-dir supports Release and Preserve retention only".to_string(),
                ))
            }
        };
        Ok(ProviderReleaseResult {
            provider_ref: self.provider_ref.clone(),
            material_ref: allocation.material_ref.clone(),
            disposition,
            changed: matches!(retention, RetentionExpectation::Release),
        })
    }
}

fn execution_request(demand: &str, retention: RetentionExpectation) -> ExecutionMaterialRequest {
    ExecutionMaterialRequest {
        demand_ref: DemandRef::new(demand).unwrap(),
        affordances: vec!["process.execute".to_string()],
        resources: vec![],
        connectivity: vec![],
        isolation_trust: None,
        retention,
    }
}

#[test]
fn an_external_provider_proves_the_full_execution_lifecycle() {
    let temp = scratch_dir("lifecycle");
    let mut provider = ProcessDirProvider::at_root(&temp).unwrap();

    // Port conformance first: identity, port and offers must be coherent.
    let report = verify_provider_port(&provider).unwrap();
    assert_eq!(report.offer_count, 1);
    assert_eq!(report.available_offers, 1);

    // Prepare: a real working directory comes into existence.
    let request = execution_request("demand:specimen", RetentionExpectation::Release);
    let allocation = provider.prepare_execution(&request).unwrap();
    let workdir = provider.workdir(&allocation);
    assert!(
        workdir.is_dir(),
        "prepare materialises the working directory"
    );

    // Execute: useful work with collected output.
    let operation = ProviderOperation {
        key: "write-and-read".to_string(),
        parameters: BTreeMap::from([(
            "command".to_string(),
            "echo specimen > note.txt && cat note.txt".to_string(),
        )]),
    };
    let result = provider.execute_operation(&allocation, &operation).unwrap();
    assert_eq!(result.output["exit"], "0");
    assert_eq!(result.output["stdout"].trim(), "specimen");
    assert!(workdir.join("note.txt").is_file(), "the effect is real");

    // Observe: the provider reports live state, not a recorded receipt.
    let observation = provider.observe_execution(&allocation).unwrap();
    assert_eq!(observation.health, HealthState::Healthy);
    assert_eq!(observation.detail["entries"], "1");

    // Release under Release: only the provider's own world goes away.
    let released = provider
        .release_execution(&allocation, &RetentionExpectation::Release)
        .unwrap();
    assert_eq!(
        released.disposition,
        epilogos_workcell_sdk::contract::ReleaseDisposition::Released
    );
    assert!(!workdir.exists(), "release removes the allocated world");
    assert!(temp.is_dir(), "the root itself survives");
}

#[test]
fn retention_is_honoured_and_unsupported_retention_refuses() {
    let temp = scratch_dir("retention");
    let mut provider = ProcessDirProvider::at_root(&temp).unwrap();

    let request = execution_request("demand:preserve", RetentionExpectation::Preserve);
    let allocation = provider.prepare_execution(&request).unwrap();
    let workdir = provider.workdir(&allocation);

    let preserved = provider
        .release_execution(&allocation, &RetentionExpectation::Preserve)
        .unwrap();
    assert_eq!(
        preserved.disposition,
        epilogos_workcell_sdk::contract::ReleaseDisposition::Preserved
    );
    assert!(workdir.is_dir(), "preserve keeps the world");

    let error = provider
        .release_execution(&allocation, &RetentionExpectation::SnapshotIfSupported)
        .unwrap_err();
    assert!(
        error.to_string().contains("supports Release and Preserve"),
        "unsupported retention is refused, not silently degraded: {error}"
    );
    assert!(workdir.is_dir(), "a refused release leaves the world alone");
}

#[test]
fn a_foreign_allocation_is_refused_not_executed() {
    let temp = scratch_dir("identity");
    let mut provider = ProcessDirProvider::at_root(&temp).unwrap();
    let request = execution_request("demand:identity", RetentionExpectation::Release);
    let allocation = provider.prepare_execution(&request).unwrap();

    let forged = ProviderAllocation {
        provider_ref: ProviderRef::new("provider:external/impostor").unwrap(),
        ..allocation.clone()
    };
    let error = provider
        .execute_operation(
            &forged,
            &ProviderOperation {
                key: "run".to_string(),
                parameters: BTreeMap::from([("command".to_string(), "echo pwned".to_string())]),
            },
        )
        .unwrap_err();
    assert!(
        error.to_string().contains("changed provider identity"),
        "identity non-collapse is enforced: {error}"
    );
}

#[test]
fn the_specimen_proves_a_useful_demand_through_the_contract_shapes() {
    // The demand a caller would plan through the contract module, showing the
    // specimen integrates with the full demand grammar, not a private shape.
    let mut demand = ExecutionDemand::new(DemandRef::new("demand:contract-shape").unwrap());
    demand.subjects.insert(
        "specimen".to_string(),
        ExternalRef::new("external:specimen-subject").unwrap(),
    );
    assert_eq!(demand.demand_ref.as_str(), "demand:contract-shape");

    let temp = scratch_dir("contract");
    let mut provider = ProcessDirProvider::at_root(&temp).unwrap();
    let request = ExecutionMaterialRequest {
        demand_ref: demand.demand_ref.clone(),
        affordances: vec!["process.execute".to_string()],
        resources: demand.resources.clone(),
        connectivity: vec![],
        isolation_trust: None,
        retention: RetentionExpectation::Release,
    };
    let allocation = provider.prepare_execution(&request).unwrap();
    assert_eq!(
        allocation.provenance["demand"], "demand:contract-shape",
        "provenance carries the caller's demand identity"
    );
    // Path used by the workdir must stay inside the provider's own root.
    let workdir = provider.workdir(&allocation);
    assert!(workdir.starts_with(&temp));
    let _ = fs::remove_dir_all(&temp);
}
