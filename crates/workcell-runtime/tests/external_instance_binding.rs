#![cfg(unix)]

use epilogos_workcell_core::*;
use epilogos_workcell_runtime::*;
use serde_json::Value;
use sha2::Digest;
use std::{
    collections::BTreeMap,
    fs,
    net::TcpListener,
    path::PathBuf,
    process::{Command, Stdio},
    sync::atomic::{AtomicU64, Ordering},
    thread,
    time::{Duration, Instant},
};

static SEQUENCE: AtomicU64 = AtomicU64::new(0);

struct Target {
    root: PathBuf,
    script: PathBuf,
    port: u16,
}

impl Target {
    fn new(label: &str) -> Self {
        // Native gate supplies this allocated T destination. No system-temp
        // writes, model imports, package dependencies or ambient service use.
        let base = PathBuf::from(
            std::env::var_os("WORKCELL_TEST_ARTIFACT_ROOT")
                .expect("real-process gate requires WORKCELL_TEST_ARTIFACT_ROOT"),
        );
        assert!(
            base.is_absolute() && base.is_dir(),
            "native artifact root must be an existing absolute directory"
        );
        let base = fs::canonicalize(base).expect("native artifact root must resolve physically");
        let manifest_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        for ancestor in manifest_dir.ancestors() {
            let clearings = ancestor.join("Control/agents/now/clearings");
            if clearings.is_dir() {
                let relative = base
                    .strip_prefix(fs::canonicalize(&clearings).unwrap())
                    .expect("Central native artifacts must remain in an allocated root clearing T");
                let parts: Vec<_> = relative.components().collect();
                assert!(parts.len() >= 3 && parts[1].as_os_str() == "T",
                    "Central native artifact root must be a destination below an existing clearing T");
                break;
            }
        }
        let root = base.join(format!(
            "{label}-{}-{}",
            std::process::id(),
            SEQUENCE.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir(&root).expect("native test artifact directory must be newly owned");
        let root = fs::canonicalize(root).unwrap();
        let script = root.join("controller.py");
        fs::write(
            &script,
            include_str!("fixtures/external_instance_target.py"),
        )
        .unwrap();
        let port = TcpListener::bind("127.0.0.1:0")
            .unwrap()
            .local_addr()
            .unwrap()
            .port();
        Self { root, script, port }
    }

    fn command(&self, operation: &str) -> ExternalServiceCommand {
        ExternalServiceCommand::new("python3")
            .unwrap()
            .with_arg("-S")
            .with_arg(self.script.display().to_string())
            .with_arg(operation)
            .with_arg(self.root.display().to_string())
            .with_arg(self.port.to_string())
    }

    fn service(&self) -> ExternalManagedService {
        ExternalManagedService::new(
            "service:actual-target",
            format!("http://127.0.0.1:{}", self.port),
            self.command("status"),
        )
        .unwrap()
        .with_start(self.command("start"))
        .with_stop(self.command("stop"))
        .with_readiness(self.command("ready"))
        .with_target_instance(self.command("capture"))
        .with_acquisition(ExternalServiceAcquisition::EnsureRunning)
        .with_readiness_timing(2_000, 20)
    }

    fn declared_service(&self) -> ExternalManagedService {
        let command = |operation: &str| serde_json::json!({"program": "python3", "args": ["-S", self.script.display().to_string(), operation, self.root.display().to_string(), self.port.to_string()]});
        let declaration = serde_json::json!({"schema": "workcell.service-declaration/v1", "services": [{
            "logical_ref": "service:actual-target", "lifetime": "target-owned",
            "endpoint": format!("http://127.0.0.1:{}", self.port), "acquisition": "ensure-running",
            "status": command("status"), "start": command("start"), "stop": command("stop"),
            "readiness": {"program": "python3", "args": ["-S", self.script.display().to_string(), "ready", self.root.display().to_string(), self.port.to_string()], "timeout_ms": 2_000, "interval_ms": 20},
            "target_instance": {"schema": "workcell.external-target-instance/v1", "capture": command("capture")}
        }]});
        let path = self.root.join("services.json");
        fs::write(&path, serde_json::to_vec(&declaration).unwrap()).unwrap();
        read_service_declarations(path)
            .unwrap()
            .target_owned
            .remove(0)
    }

    fn provider(&self, service: ExternalManagedService) -> ExternalManagedServiceProvider {
        ExternalManagedServiceProvider::new(
            ProviderRef::new(TARGET_SERVICE_PROVIDER_REF).unwrap(),
            [service],
        )
        .unwrap()
    }

    fn native_result(
        &self,
        operation: &str,
        basis: &Value,
    ) -> std::result::Result<BoundedProcessOutput, String> {
        let mut command = Command::new("python3");
        command
            .arg("-S")
            .arg(&self.script)
            .arg(operation)
            .arg(&self.root)
            .arg(self.port.to_string())
            .env(
                "WORKCELL_TARGET_INSTANCE_PROTOCOL",
                "workcell.external-target-instance/v1",
            )
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
        bounded_actual_call(command)
    }

    fn native(&self, operation: &str, basis: &Value) -> BoundedProcessOutput {
        self.native_result(operation, basis)
            .expect("actual target command must be bounded and complete; private output withheld")
    }

    fn teardown(&self) -> std::result::Result<(), String> {
        // Release only this fixture's publication aperture, including failed
        // assertions, before exact native basis/identity teardown. It never
        // substitutes a native reply or changes target control authority.
        fs::write(
            self.root.join("basis-publication-release"),
            b"owned teardown release",
        )
        .map_err(|_| "cannot release owned publication barrier; retained native intent")?;
        let deadline = Instant::now() + Duration::from_secs(8);
        loop {
            let mut unqualified_start = false;
            for entry in
                fs::read_dir(&self.root).map_err(|_| "cannot enumerate retained target facts")?
            {
                let entry = entry.map_err(|_| "cannot read retained target entry")?;
                let path = entry.path().join("basis.json");
                if !path.is_file() {
                    unqualified_start |= entry.path().join("intent.json").is_file();
                    continue;
                }
                let bytes = fs::read(&path).map_err(|_| "cannot read original basis")?;
                let mut basis: Value =
                    serde_json::from_slice(&bytes).map_err(|_| "original basis is unqualified")?;
                basis["basis_path"] = Value::String(path.display().to_string());
                use sha2::{Digest, Sha256};
                basis["basis_sha256"] = Value::String(format!("{:x}", Sha256::digest(&bytes)));
                let observe_identity = || -> std::result::Result<bool, String> {
                    let observation = self.native_result("identity", &basis)?;
                    if !observation.status.success() {
                        return Err(
                            "actual OS identity observation failed; retirement unknown".into()
                        );
                    }
                    let value: Value = serde_json::from_slice(&observation.stdout)
                        .map_err(|_| "actual OS identity observation is unqualified")?;
                    if value["generation"] != basis["generation"]
                        || value["server"] != basis["server"]
                    {
                        return Err("OS observation does not identify original generation".into());
                    }
                    value["same_process"]
                        .as_bool()
                        .ok_or_else(|| "OS observation lacks actual presence standing".into())
                };
                if observe_identity()? {
                    if !self.native_result("allow_stop", &basis)?.status.success() {
                        return Err("original native test owner refused teardown admission; retirement unknown".into());
                    }
                    // Native result can honestly retain an unknown ACK. It
                    // never substitutes for the independent exact OS proof.
                    let _stop = self.native_result("stop", &basis)?;
                    if observe_identity()? {
                        return Err("original target identity remained present after teardown (including unreaped zombie)".into());
                    }
                }
            }
            if !unqualified_start {
                return Ok(());
            }
            if Instant::now() >= deadline {
                return Err("native starting identity remained unqualified; no guessed cleanup, retained evidence".into());
            }
            thread::sleep(Duration::from_millis(25));
        }
    }
}

impl Drop for Target {
    fn drop(&mut self) {
        if let Err(error) = self.teardown() {
            let _ = fs::write(self.root.join("teardown-uncertainty.txt"), &error);
            if thread::panicking() {
                eprintln!("native teardown remains uncertain: {error}; original test already failed; retained evidence");
            } else {
                panic!("native teardown remains uncertain: {error}; retained evidence");
            }
        }
        // Keep source/native retained records for gate evidence; day owner
        // performs artifact lifecycle after the test return.
    }
}

fn bounded_actual_call(mut command: Command) -> std::result::Result<BoundedProcessOutput, String> {
    command
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let output = run_bounded_process(command, Duration::from_secs(10), 16_384)
        .map_err(|_| "actual native command could not complete; private output withheld")?;
    if output.timed_out || !output.output_complete || output.output_truncated {
        return Err(format!("actual native command unqualified: timeout={}, complete={}, truncated={}; private output withheld", output.timed_out, output.output_complete, output.output_truncated));
    }
    Ok(output)
}

fn request(label: &str) -> ServiceMaterialRequest {
    ServiceMaterialRequest {
        demand_ref: DemandRef::new(format!("demand:{label}")).unwrap(),
        connection: LogicalConnectionRequirement::new("service:actual-target").unwrap(),
        persistence: Some(PersistenceScope::Project),
        retention: RetentionExpectation::Release,
    }
}

fn basis(allocation: &ProviderAllocation) -> Value {
    serde_json::from_str(&allocation.properties["target_instance_basis"]).unwrap()
}

#[test]
fn actual_os_receipt_reentry_retains_generation_and_releases_the_original_process() {
    let target = Target::new("receipt-reentry");
    let mut original = target.provider(target.declared_service());
    let allocation = original.resolve_service(&request("owner")).unwrap();
    let path = target.root.join("durable-properties.json");
    fs::write(&path, serde_json::to_vec(&allocation.properties).unwrap()).unwrap();
    drop(original);
    let properties: BTreeMap<String, String> =
        serde_json::from_slice(&fs::read(path).unwrap()).unwrap();
    let restored = ProviderAllocation {
        properties,
        ..allocation.clone()
    };
    let mut fresh = target.provider(target.declared_service());
    fresh.restore_allocation(&restored).unwrap();
    assert_eq!(
        fresh.observe_service(&restored).unwrap().health,
        HealthState::Healthy
    );
    assert_eq!(basis(&restored), basis(&allocation));
    let immutable: Value = serde_json::from_slice(
        &fs::read(basis(&restored)["basis_path"].as_str().unwrap()).unwrap(),
    )
    .unwrap();
    let secret = immutable["private_native_key"].as_str().unwrap();
    assert!(!format!("{:?}{:?}", restored.properties, restored.provenance).contains(secret));
    assert!(!format!("{:?}", fresh.observe_service(&restored).unwrap().detail).contains(secret));
    assert!(target.native("status", &basis(&restored)).status.success());
    assert!(
        fresh
            .release_service(&restored, &RetentionExpectation::Release)
            .unwrap()
            .changed
    );
    assert!(!target.native("status", &basis(&restored)).status.success());
}

#[test]
fn retired_receipt_cannot_stop_or_recover_a_successor_at_the_same_endpoint() {
    let target = Target::new("same-endpoint-successor");
    let mut original = target.provider(target.service());
    let old = original.resolve_service(&request("old")).unwrap();
    original
        .release_service(&old, &RetentionExpectation::Release)
        .unwrap();
    let mut successor = target.provider(target.service());
    let new = successor.resolve_service(&request("new")).unwrap();
    assert_ne!(basis(&old)["generation"], basis(&new)["generation"]);
    let mut late = target.provider(target.service());
    late.restore_allocation(&old).unwrap();
    assert!(
        !late
            .release_service(&old, &RetentionExpectation::Release)
            .unwrap()
            .changed
    );
    assert!(late.restart_service(&old).is_err());
    assert!(late.recover_service(&old).is_err());
    assert!(target.native("status", &basis(&new)).status.success());
    successor
        .release_service(&new, &RetentionExpectation::Release)
        .unwrap();
}

#[test]
fn cached_receipt_tampering_and_changed_declaration_refuse_before_target_mutation() {
    let target = Target::new("digest-and-cache");
    let mut provider = target.provider(target.service());
    let allocation = provider.resolve_service(&request("owner")).unwrap();
    let mut changed = allocation.clone();
    changed
        .properties
        .insert("started_by_provider".into(), "false".into());
    assert!(provider.observe_service(&changed).is_err());
    assert!(provider
        .release_service(&changed, &RetentionExpectation::Release)
        .is_err());
    let new_service = target
        .service()
        .with_metadata("configuration_revision", "changed")
        .unwrap();
    let mut changed_source = target.provider(new_service);
    assert!(changed_source.restore_allocation(&allocation).is_err());
    assert!(changed_source
        .release_service(&allocation, &RetentionExpectation::Release)
        .is_err());
    assert!(target
        .native("status", &basis(&allocation))
        .status
        .success());
    provider
        .release_service(&allocation, &RetentionExpectation::Release)
        .unwrap();
}

#[test]
fn known_dependency_blocks_owner_stop_and_observed_binding_never_owns_restart() {
    let target = Target::new("dependency");
    let mut provider = target.provider(target.service());
    let owner = provider.resolve_service(&request("owner")).unwrap();
    let observed = provider.resolve_service(&request("dependent")).unwrap();
    assert_eq!(observed.properties["started_by_provider"], "false");
    assert!(provider.restart_service(&observed).is_err());
    assert!(provider
        .release_service(&owner, &RetentionExpectation::Release)
        .is_err());
    assert!(target.native("status", &basis(&owner)).status.success());
    assert!(
        !provider
            .release_service(&observed, &RetentionExpectation::Release)
            .unwrap()
            .changed
    );
    assert!(
        provider
            .release_service(&owner, &RetentionExpectation::Release)
            .unwrap()
            .changed
    );
}

#[test]
fn native_admission_refuses_stop_for_real_active_work_even_with_one_local_receipt() {
    let target = Target::new("actual-native-active-work");
    let mut provider = target.provider(target.service());
    let owner = provider.resolve_service(&request("owner")).unwrap();
    let retained = basis(&owner);
    thread::scope(|scope| {
        let task = scope.spawn(|| target.native("hold", &retained));
        let deadline = Instant::now() + Duration::from_secs(8);
        loop {
            let observation = target.native("status", &retained);
            let value: Value = serde_json::from_slice(&observation.stdout).unwrap();
            if value["pending"].as_u64().unwrap() > 0 {
                break;
            }
            assert!(
                Instant::now() < deadline,
                "real native work must enter admission before stop"
            );
            thread::sleep(Duration::from_millis(10));
        }
        assert!(provider
            .release_service(&owner, &RetentionExpectation::Release)
            .is_err());
        assert_eq!(basis(&owner), retained);
        assert!(target.native("status", &retained).status.success());
        assert!(task.join().unwrap().status.success());
    });
    // A distinct operation after known work completes may retire the exact
    // original instance. No process-local global-zero assertion authorises it.
    provider
        .release_service(&owner, &RetentionExpectation::Release)
        .unwrap();
}

#[test]
fn a_late_basis_is_captured_from_the_original_start_generation() {
    let target = Target::new("late-basis");
    let service = target.service().with_start(
        target
            .command("start")
            .with_env("TEST_BASIS_DELAY", "0.35")
            .unwrap(),
    );
    let mut provider = target.provider(service);
    let current = target.root.join("current");
    let replace_current = thread::spawn(move || {
        let deadline = Instant::now() + Duration::from_secs(10);
        while !current.is_file() {
            assert!(
                Instant::now() < deadline,
                "actual start must publish its generation"
            );
            thread::sleep(Duration::from_millis(5));
        }
        // A mutable pointer changes while the actual owned server is still
        // publishing its immutable basis. Capture must use the start token.
        fs::write(current, "00000000000000000000000000000000").unwrap();
    });
    let allocation = provider.resolve_service(&request("late")).unwrap();
    replace_current.join().unwrap();
    let start: Value =
        serde_json::from_str(&allocation.properties["target_start_receipt"]).unwrap();
    assert_eq!(start["generation"], basis(&allocation)["generation"]);
    assert!(target
        .native("status", &basis(&allocation))
        .status
        .success());
    provider
        .release_service(&allocation, &RetentionExpectation::Release)
        .unwrap();
}

#[test]
fn missing_basis_returns_reconciliation_error_with_actual_immutable_start_intent() {
    let target = Target::new("basis-not-yet-published");
    let service = target
        .service()
        .with_start(
            target
                .command("start")
                .with_env("TEST_BASIS_BARRIER", "basis-publication-release")
                .unwrap(),
        )
        .with_readiness_timing(25, 5);
    let mut provider = target.provider(service);
    let error = provider.resolve_service(&request("too-early")).unwrap_err();
    assert!(matches!(error, WorkcellError::ReconciliationFailed(_)));
    assert!(error.to_string().contains("intent_path"));
    assert!(error.to_string().contains("intent_sha256"));
    assert!(error.to_string().contains("effects uncertain"));
    assert!(!error.to_string().contains("cleanup succeeded"));
    assert!(error.to_string().contains("\"native_type\":\"NotReady\""));
    let generation = fs::read_to_string(target.root.join("current")).unwrap();
    let directory = target.root.join(&generation);
    let intent = directory.join("intent.json");
    assert!(
        !directory.join("basis.json").exists(),
        "actual publication stays barred until observed refusal and teardown release"
    );
    let held: Value =
        serde_json::from_slice(&fs::read(directory.join("publication-held.json")).unwrap())
            .unwrap();
    assert_eq!(held["generation"].as_str(), Some(generation.as_str()));
    assert!(!target.root.join("basis-publication-release").exists());
    assert!(
        intent.is_file(),
        "the actual target retains the start intent, not a consumer-minted allocation"
    );
    use std::os::unix::fs::MetadataExt;
    let staged = directory.join(".basis.writing");
    let staged_metadata = fs::symlink_metadata(&staged).unwrap();
    assert!(staged_metadata.file_type().is_file());
    assert_eq!(staged_metadata.nlink(), 1);
    let staged_bytes = fs::read(&staged).unwrap();
    let staged_value: Value = serde_json::from_slice(&staged_bytes).unwrap();
    assert_eq!(
        staged_value["schema"],
        "workcell.external-target-instance/v1"
    );
    assert_eq!(staged_value["generation"], held["generation"]);
    assert_eq!(staged_value["server"], held["server"]);
    let staged_digest = format!("{:x}", sha2::Sha256::digest(&staged_bytes));
    // Release only this fixture's actual publication barrier. No reply or
    // process identity is substituted; the native owner publishes its basis.
    fs::write(
        target.root.join("basis-publication-release"),
        b"owned test release",
    )
    .unwrap();
    let published = directory.join("basis.json");
    let deadline = Instant::now() + Duration::from_secs(10);
    while !published.exists() || staged.exists() {
        assert!(
            Instant::now() < deadline,
            "complete native basis publication must finish"
        );
        thread::sleep(Duration::from_millis(5));
    }
    let published_metadata = fs::symlink_metadata(&published).unwrap();
    assert!(published_metadata.file_type().is_file());
    assert_eq!(published_metadata.nlink(), 1);
    assert_eq!(published_metadata.dev(), staged_metadata.dev());
    assert_eq!(published_metadata.ino(), staged_metadata.ino());
    assert!(
        fs::read(&published).unwrap() == staged_bytes,
        "published native basis retains the staged bytes"
    );
    assert_eq!(
        format!("{:x}", sha2::Sha256::digest(fs::read(&published).unwrap())),
        staged_digest
    );
    let mut observed_basis = staged_value;
    observed_basis["basis_path"] = Value::String(published.display().to_string());
    observed_basis["basis_sha256"] = Value::String(staged_digest);
    let identity_reply = target.native("identity", &observed_basis);
    assert!(identity_reply.status.success());
    let identity: Value = serde_json::from_slice(&identity_reply.stdout).unwrap();
    assert_eq!(identity["generation"], observed_basis["generation"]);
    assert_eq!(identity["server"], observed_basis["server"]);
    assert_eq!(identity["same_process"], true);
}

#[test]
fn failed_readiness_retains_native_cleanup_refusal_and_original_basis() {
    let target = Target::new("failed-cleanup");
    let start = target
        .command("start")
        .with_env("TEST_NEVER_READY", "1")
        .unwrap()
        .with_env("TEST_REFUSE_STOP", "1")
        .unwrap();
    let service = target
        .service()
        .with_start(start)
        .with_readiness_timing(250, 20);
    let mut provider = target.provider(service);
    let error = provider.resolve_service(&request("refused")).unwrap_err();
    assert!(matches!(error, WorkcellError::CleanupFailed(_)));
    assert!(error.to_string().contains("actual cleanup failure"));
    assert!(error.to_string().contains("native-http-refusal"));
    assert!(error.to_string().contains("\"http_status\":409"));
    assert!(error.to_string().contains("HTTPError"));
    assert!(error.to_string().contains("\"http_status\":503"));
    assert!(error.to_string().contains("evidence_sha256"));
    assert!(error.to_string().contains("basis_path"));
    assert!(error.to_string().contains("intent_path"));
    assert!(!error.to_string().contains("stopped the started process"));
    let generation = fs::read_to_string(target.root.join("current")).unwrap();
    let file = target.root.join(generation).join("basis.json");
    assert!(
        file.is_file(),
        "real unready owned process retains its immutable basis"
    );
}

#[test]
fn old_provider_protocol_is_refused_before_intent_or_process_effects() {
    let target = Target::new("old-protocol");
    let mut command = Command::new("python3");
    command
        .arg("-S")
        .arg(&target.script)
        .arg("start")
        .arg(&target.root)
        .arg(target.port.to_string())
        .env_remove("WORKCELL_TARGET_INSTANCE_PROTOCOL");
    let output = bounded_actual_call(command).unwrap();
    assert!(!output.status.success());
    assert!(!target.root.join("current").exists());
    assert_eq!(
        fs::read_dir(&target.root).unwrap().count(),
        1,
        "only public test script exists, no native intent or launcher"
    );
}

#[test]
fn same_provider_restored_successor_is_not_blocked_by_late_old_receipt_observation() {
    let target = Target::new("same-provider-retired-reentry");
    let mut first = target.provider(target.service());
    let old = first.resolve_service(&request("old")).unwrap();
    first
        .release_service(&old, &RetentionExpectation::Release)
        .unwrap();
    let new = first.resolve_service(&request("successor")).unwrap();
    drop(first);
    let mut restored = target.provider(target.service());
    restored.restore_allocation(&new).unwrap();
    for _ in 0..2 {
        let old_observation = restored.observe_service(&old).unwrap();
        assert_eq!(old_observation.health, HealthState::Unavailable);
        assert!(old_observation.detail["status_native_cause"].contains("target-absent"));
        assert!(old_observation.detail["status_native_cause"].contains("evidence_sha256"));
    }
    restored.restore_allocation(&old).unwrap();
    assert!(target.native("status", &basis(&new)).status.success());
    // The retired receipt never enters the live dependency census, so it
    // cannot refuse the successor's actual native release in this provider.
    assert!(
        restored
            .release_service(&new, &RetentionExpectation::Release)
            .unwrap()
            .changed
    );
    let observed: Value =
        serde_json::from_slice(&target.native("identity", &basis(&new)).stdout).unwrap();
    assert_eq!(observed["same_process"], false);
}

#[test]
fn actual_failed_probe_is_not_absence_and_cannot_allocate_a_replacement() {
    let target = Target::new("failed-probe-is-not-missing");
    let mut owner = target.provider(target.service());
    let allocation = owner.resolve_service(&request("owner")).unwrap();
    let mut refused = target.service();
    refused.status = target.command("refused_read");
    let mut provider = target.provider(refused);
    let before = fs::read_dir(&target.root)
        .unwrap()
        .flatten()
        .filter(|entry| entry.path().join("intent.json").is_file())
        .count();
    let error = provider
        .resolve_service(&request("must-not-start"))
        .unwrap_err();
    assert!(error.to_string().contains("native-http-refusal"));
    assert!(error.to_string().contains("\"http_status\":409"));
    assert!(provider.offers().is_err());
    let after = fs::read_dir(&target.root)
        .unwrap()
        .flatten()
        .filter(|entry| entry.path().join("intent.json").is_file())
        .count();
    assert_eq!(
        before, after,
        "failed native read cannot mint another start intent"
    );
    assert!(target
        .native("status", &basis(&allocation))
        .status
        .success());
    owner
        .release_service(&allocation, &RetentionExpectation::Release)
        .unwrap();
}

#[test]
fn actual_stop_response_loss_retains_uncertainty_after_exact_physical_retirement() {
    let target = Target::new("stop-response-loss");
    let service = target.service().with_start(
        target
            .command("start")
            .with_env("TEST_LOSE_STOP_ACK", "1")
            .unwrap(),
    );
    let mut provider = target.provider(service);
    let allocation = provider.resolve_service(&request("owner")).unwrap();
    let error = provider
        .release_service(&allocation, &RetentionExpectation::Release)
        .unwrap_err();
    assert!(matches!(error, WorkcellError::ReconciliationFailed(_)));
    assert!(error.to_string().contains("\"stop_effect\":\"unknown\""));
    assert!(error.to_string().contains("\"acknowledged\":false"));
    assert!(error
        .to_string()
        .contains("\"native_quiescence_verified\":false"));
    assert!(!error.to_string().contains("cleanup succeeded"));
    assert_eq!(
        provider.observe_service(&allocation).unwrap().health,
        HealthState::Unavailable
    );
    let observed: Value =
        serde_json::from_slice(&target.native("identity", &basis(&allocation)).stdout).unwrap();
    assert_eq!(observed["same_process"], false);
    assert!(std::net::TcpStream::connect(("127.0.0.1", target.port)).is_err());
    let repeated = provider
        .release_service(&allocation, &RetentionExpectation::Release)
        .unwrap_err();
    assert!(
        repeated.to_string().contains("\"stop_effect\":\"unknown\""),
        "retained unknown ACK cannot become success on re-entry"
    );
}

#[test]
fn durable_world_restore_is_atomic_when_a_later_binding_or_world_owner_is_invalid() {
    let target = Target::new("world-restore");
    let make = || {
        CollapsedLocalConfig::new(
            WorkcellRef::new("workcell:instance-test").unwrap(),
            target.root.join("state"),
        )
        .with_target_owned_service(target.service())
    };
    let mut source = CollapsedLocalWorkcell::new(make()).unwrap();
    let demand = |label: &str| {
        let mut demand = ExecutionDemand::new(DemandRef::new(format!("demand:{label}")).unwrap());
        demand
            .connectivity
            .required
            .push(LogicalConnectionRequirement::new("service:actual-target").unwrap());
        demand.retention = RetentionExpectation::Release;
        demand
    };
    let owner = source.prepare(&demand("owner")).unwrap();
    let dependent = source.prepare(&demand("dependent")).unwrap();
    drop(source);
    let mut fresh = CollapsedLocalWorkcell::new(make()).unwrap();
    let mut bad = dependent.clone();
    let mut later = bad
        .binding_graph
        .bindings
        .iter()
        .find(|binding| binding.port == ProviderPortKind::Service)
        .unwrap()
        .clone();
    later.material_ref.push_str("-invalid");
    later
        .properties
        .insert("declaration_digest".into(), "sha256:changed".into());
    bad.binding_graph.bindings.push(later);
    assert!(fresh.register_world(bad.clone()).is_err());
    assert!(fresh.world(&bad.world_ref).is_none());
    let mut wrong_owner = dependent;
    wrong_owner.workcell_ref = WorkcellRef::new("workcell:wrong-owner").unwrap();
    assert!(fresh.register_world(wrong_owner.clone()).is_err());
    assert!(fresh.world(&wrong_owner.world_ref).is_none());
    fresh.register_world(owner.clone()).unwrap();
    // A leaked first dependency from either failed registration would refuse
    // this native release. Real process retirement proves both commits absent.
    fresh.release(&owner.world_ref).unwrap();
    assert!(std::net::TcpStream::connect(("127.0.0.1", target.port)).is_err());
}

#[test]
fn actual_unreaped_zombie_is_present_until_its_owner_reaps() {
    let target = Target::new("actual-zombie-oracle");
    let mut command = Command::new("python3");
    command
        .arg("-S")
        .arg(&target.script)
        .arg("zombie_oracle")
        .arg(&target.root)
        .arg(target.port.to_string());
    let output = bounded_actual_call(command)
        .expect("actual zombie/reap observation must complete within native bounds");
    assert!(
        output.status.success(),
        "native zombie/reap oracle failed; private output withheld"
    );
    let actual: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert!(actual["observed"]["state"]
        .as_str()
        .unwrap()
        .starts_with('Z'));
    assert!(actual["observed"]["pid"].as_u64().unwrap() > 0);
    assert!(actual["observed"]["uid"].as_u64().is_some());
    assert!(!actual["observed"]["start"].as_str().unwrap().is_empty());
    assert_eq!(actual["same_process_before_reap"], true);
    assert_eq!(actual["retirement_refused_before_reap"], true);
    assert_eq!(actual["owner_wait_exit"], 0);
    assert_eq!(actual["physically_absent_after_owner_reap"], true);
}

#[test]
fn actual_bound_native_read_witness_qualifies_health_and_closed_admission_is_degraded() {
    let target = Target::new("actual-health-witness");
    let mut provider = target.provider(target.declared_service());
    let allocation = provider
        .resolve_service(&request("health-witness"))
        .unwrap();
    let original = basis(&allocation);
    let observed = provider.observe_service(&allocation).unwrap();
    assert_eq!(observed.health, HealthState::Healthy);
    for key in ["status_native_observation", "readiness_native_observation"] {
        let actual: Value = serde_json::from_str(&observed.detail[key]).unwrap();
        assert_eq!(actual["schema"], "workcell.external-target-observation/v1");
        assert_eq!(actual["native_healthy"], true);
        for field in [
            "basis_path",
            "basis_sha256",
            "generation",
            "config_sha256",
            "endpoint",
            "server",
        ] {
            assert_eq!(actual[field], original[field]);
        }
        assert!(actual.get("private_native_key").is_none());
    }
    assert!(target.native("quiesce", &original).status.success());
    let held = provider.observe_service(&allocation).unwrap();
    assert_eq!(held.health, HealthState::Degraded);
    let actual: Value = serde_json::from_str(&held.detail["status_native_observation"]).unwrap();
    assert_eq!(actual["native_healthy"], false);
    assert_eq!(actual["server"], original["server"]);
    assert!(target.native("resume", &original).status.success());
    assert_eq!(
        provider.observe_service(&allocation).unwrap().health,
        HealthState::Healthy
    );
    provider
        .release_service(&allocation, &RetentionExpectation::Release)
        .unwrap();
}

#[test]
fn actual_zero_exit_without_native_observation_cannot_start_or_admit_optional_service() {
    let target = Target::new("actual-empty-success-reply");
    let mut declaration = target.service();
    declaration.status = ExternalServiceCommand::new("/usr/bin/true").unwrap();
    let mut provider = target.provider(declaration.clone());
    assert!(provider.offers().is_err());
    let error = provider
        .resolve_service(&request("empty-witness"))
        .unwrap_err()
        .to_string();
    assert!(error.contains("native successful observation remains unqualified"));
    assert!(error.contains("health uncertain"));
    assert!(!fs::read_dir(&target.root).unwrap().any(|entry| entry
        .unwrap()
        .path()
        .join("intent.json")
        .exists()));
    // Same actual zero-exit program keeps the explicit legacy contract.
    declaration.target_instance = None;
    declaration.readiness = None;
    let mut legacy = target.provider(declaration);
    let allocation = legacy
        .resolve_service(&request("legacy-zero-exit"))
        .unwrap();
    assert_eq!(allocation.health, HealthState::Healthy);
    assert!(
        !legacy
            .release_service(&allocation, &RetentionExpectation::Release)
            .unwrap()
            .changed
    );
}

#[test]
fn actual_current_successor_witness_cannot_qualify_old_receipt_or_mutate_successor_cache() {
    let target = Target::new("actual-successor-witness");
    let mut declaration = target.service();
    declaration.status = target.command("current_status");
    let mut original = target.provider(declaration.clone());
    let old = original
        .resolve_service(&request("original-witness"))
        .unwrap();
    original
        .release_service(&old, &RetentionExpectation::Release)
        .unwrap();
    let new = original
        .resolve_service(&request("successor-witness"))
        .unwrap();
    assert_ne!(basis(&old)["generation"], basis(&new)["generation"]);
    assert_ne!(basis(&old)["server"], basis(&new)["server"]);
    let mut restored = target.provider(declaration);
    restored.restore_allocation(&new).unwrap();
    let error = restored.observe_service(&old).unwrap_err().to_string();
    assert!(error.contains(
        "successful native observation differs from the original immutable target basis"
    ));
    assert!(restored.restore_allocation(&old).is_err());
    let observed = restored.observe_service(&new).unwrap();
    assert_eq!(observed.health, HealthState::Healthy);
    let actual: Value =
        serde_json::from_str(&observed.detail["status_native_observation"]).unwrap();
    assert_eq!(actual["generation"], basis(&new)["generation"]);
    assert!(
        restored
            .release_service(&new, &RetentionExpectation::Release)
            .unwrap()
            .changed
    );
}

#[test]
fn actual_start_envelope_then_nonzero_exit_retains_status_and_public_intent_without_allocation() {
    let target = Target::new("start-envelope-nonzero");
    let service = target.service().with_start(
        target
            .command("start")
            .with_env("TEST_START_EXIT_AFTER_ENVELOPE", "1")
            .unwrap(),
    );
    let mut provider = target.provider(service);
    let error = provider.resolve_service(&request("owner")).unwrap_err();
    let diagnostic = error.to_string();
    assert!(
        diagnostic.contains("native exit exit status: 23;"),
        "{diagnostic}"
    );
    assert!(diagnostic.contains("secondary native cause qualification failure:"));
    assert!(diagnostic.contains("public start intent observation (not authority):"));
    assert!(diagnostic.contains("effects uncertain"));
    let generation = fs::read_to_string(target.root.join("current")).unwrap();
    let directory = target.root.join(&generation);
    let intent = directory.join("intent.json");
    let intent_bytes = fs::read(&intent).unwrap();
    let observed: Value = serde_json::from_slice(&intent_bytes).unwrap();
    assert_eq!(observed["generation"], generation);
    assert!(directory.join("launcher.json").is_file());
    assert!(diagnostic.contains(intent.to_str().unwrap()));
    assert!(diagnostic.contains(&format!("{:x}", sha2::Sha256::digest(&intent_bytes))));
    fs::write(
        target.root.join("native-nonzero-refusal.json"),
        serde_json::to_vec(&serde_json::json!({"actual_exit_code":23,
            "refusal":diagnostic,"allocation_admitted":false,
            "automatic_retry":false}))
        .unwrap(),
    )
    .unwrap();
}

#[test]
fn actual_stop_envelope_then_nonzero_exit_retains_status_and_original_basis_without_cleanup_success(
) {
    let target = Target::new("stop-envelope-nonzero");
    let service = target.service().with_stop(
        target
            .command("stop")
            .with_env("TEST_STOP_EXIT_AFTER_ENVELOPE", "1")
            .unwrap(),
    );
    let mut provider = target.provider(service);
    let allocation = provider.resolve_service(&request("owner")).unwrap();
    let original = basis(&allocation);
    let original_bytes = fs::read(original["basis_path"].as_str().unwrap()).unwrap();
    let error = provider
        .release_service(&allocation, &RetentionExpectation::Release)
        .unwrap_err();
    let diagnostic = error.to_string();
    assert!(
        diagnostic.contains("native exit exit status: 24;"),
        "{diagnostic}"
    );
    assert!(diagnostic.contains("secondary native cause qualification failure:"));
    assert!(diagnostic.contains("original instance basis retained:"));
    assert!(diagnostic.contains(original["basis_path"].as_str().unwrap()));
    assert!(diagnostic.contains(original["basis_sha256"].as_str().unwrap()));
    assert!(diagnostic.contains("effects uncertain"));
    assert_eq!(
        fs::read(original["basis_path"].as_str().unwrap()).unwrap(),
        original_bytes
    );
    let physical: Value =
        serde_json::from_slice(&target.native("identity", &original).stdout).unwrap();
    assert_eq!(physical["same_process"], false);
    assert!(std::net::TcpStream::connect(("127.0.0.1", target.port)).is_err());
    fs::write(
        target.root.join("native-nonzero-refusal.json"),
        serde_json::to_vec(&serde_json::json!({"actual_exit_code":24,
            "refusal":diagnostic,"original_basis":original,
            "cleanup_acknowledged":false,"automatic_retry":false}))
        .unwrap(),
    )
    .unwrap();
}
