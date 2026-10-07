use epilogos_workcell_control::codec::demand_value;
use epilogos_workcell_core::{
    BindingPresence, CollectionBundle, DesiredMaterialState, Discovery, ExecutionDemand,
    ExposureBundle, MaterialisationPlan, MaterialisedExecutionWorld, ObservationBundle,
    ReconciliationResult, ReleaseResult, Result, WorkcellControlPlane, WorkcellError, WorldRef,
};
use epilogos_workcell_runtime::{CollapsedLocalConfig, CollapsedLocalWorkcell};
use epilogos_workcell_wire::{decode_world, encode_world};
use serde_json::json;
use sha2::{Digest, Sha256};
use std::{
    collections::BTreeMap,
    fs::{self, File, OpenOptions},
    io::Write,
    path::{Path, PathBuf},
};

/// One material host owns a store at a time. The kernel lock is released on
/// owner teardown, not merely when one possibly duplicated descriptor closes;
/// it is not a stale PID lock. Completed receipts and release tombstones survive
/// disconnects. An interrupted effect is never silently re-dispatched.
pub struct DurableCollapsedLocalWorkcell {
    inner: CollapsedLocalWorkcell,
    receipt_root: PathBuf,
    worlds: BTreeMap<String, WorldRef>,
    receipt_paths: BTreeMap<String, PathBuf>,
    // Struct fields drop in declaration order. Keep this last so the inner
    // owner and its owned resources are gone before the store is available.
    _host_lock: MaterialHostLock,
}
struct MaterialHostLock {
    file: File,
    owner_pid: u32,
}
impl Drop for MaterialHostLock {
    fn drop(&mut self) {
        // A fork inherits the same open file description. That child must not
        // release the parent's live lease when it drops an inherited guard.
        // This is local destructor ownership, never PID-based lock takeover.
        if self.owner_pid != std::process::id() {
            return;
        }
        if let Err(error) = self.file.unlock() {
            eprintln!("release material host lock: {error}");
        }
    }
}
impl DurableCollapsedLocalWorkcell {
    pub fn new(config: CollapsedLocalConfig) -> Result<Self> {
        let receipt_root = config.state_root.join("control-worlds");
        fs::create_dir_all(&receipt_root).map_err(io_error("create receipt store"))?;
        let lock = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(receipt_root.join("host.lock"))
            .map_err(io_error("open material host lock"))?;
        lock.try_lock().map_err(|e| {
            WorkcellError::Unavailable(format!("another material host owns this state root: {e}"))
        })?;
        // Install the guard as soon as acquisition succeeds, including failed
        // construction. Later local owners drop before this earlier guard.
        let lock = MaterialHostLock {
            file: lock,
            owner_pid: std::process::id(),
        };
        let workcell_ref = config.workcell_ref.clone();
        let mut inner = CollapsedLocalWorkcell::new(config)?;
        let mut worlds = BTreeMap::new();
        let mut receipt_paths = BTreeMap::new();
        for entry in fs::read_dir(&receipt_root).map_err(io_error("read receipts"))? {
            let entry = entry.map_err(io_error("read receipt entry"))?;
            if entry.path().extension().and_then(|s| s.to_str()) != Some("json") {
                continue;
            }
            if !entry
                .file_type()
                .map_err(io_error("inspect receipt"))?
                .is_file()
            {
                return Err(WorkcellError::InvalidDemand(
                    "world receipts cannot be symlinks or directories".into(),
                ));
            }
            let encoded = fs::read_to_string(entry.path()).map_err(io_error("read receipt"))?;
            let world = decode_world(&encoded)?;
            if world.workcell_ref != workcell_ref {
                return Err(WorkcellError::InvalidDemand("receipt belongs to a different Workcell; relocation requires a new material binding, not renaming the host".into()));
            }
            // Recovery history may contain more than one world for a demand.
            if !world.provenance.contains_key("superseded_by")
                && worlds
                    .insert(world.demand_ref.to_string(), world.world_ref.clone())
                    .is_some()
            {
                return Err(WorkcellError::InvalidDemand(
                    "ambiguous active receipts for one demand; explicit reconciliation required"
                        .into(),
                ));
            }
            if receipt_paths
                .insert(world.world_ref.to_string(), entry.path())
                .is_some()
            {
                return Err(WorkcellError::InvalidDemand(
                    "duplicate receipt for one world; reconcile without overwriting history".into(),
                ));
            }
            inner.register_world(world)?;
        }
        Ok(Self {
            inner,
            receipt_root,
            worlds,
            receipt_paths,
            _host_lock: lock,
        })
    }
    pub fn inner(&self) -> &CollapsedLocalWorkcell {
        &self.inner
    }
    fn receipt_path(&self, world: &WorldRef) -> PathBuf {
        self.receipt_paths
            .get(world.as_str())
            .cloned()
            .unwrap_or_else(|| {
                self.receipt_root
                    .join(format!("{}.json", key(world.as_str())))
            })
    }
    fn intent_path(&self, id: &str) -> PathBuf {
        self.receipt_root.join(format!("{}.pending", key(id)))
    }
    fn begin(&self, id: &str, operation: &str, payload: serde_json::Value) -> Result<()> {
        // Unknown effects in one world may still own a shared target service.
        // No second demand/retry/release may bypass an interrupted material act.
        for entry in fs::read_dir(&self.receipt_root).map_err(io_error("read material journal"))? {
            let path = entry
                .map_err(io_error("read material journal entry"))?
                .path();
            if matches!(
                path.extension().and_then(|e| e.to_str()),
                Some("pending" | "writing")
            ) {
                return Err(WorkcellError::OperationFailed("unreconciled material intent/publication; inspect receipts and journal before any new effect".into()));
            }
        }
        let path = self.intent_path(id);
        let mut file = OpenOptions::new().write(true).create_new(true).open(&path).map_err(|e| {
            if e.kind() == std::io::ErrorKind::AlreadyExists {
                WorkcellError::OperationFailed(format!("pending material effects for `{id}`; inspect local journal and reconcile exact effects before retry (no automatic replay)"))
            } else { io_error("create material intent")(e) }
        })?;
        file.write_all(json!({"schema":"workcell.material-intent/v1", "id":id, "operation":operation, "payload":payload}).to_string().as_bytes()).map_err(io_error("write material intent"))?;
        file.sync_all().map_err(io_error("sync material intent"))?;
        sync_directory(&self.receipt_root)
    }
    fn finish(&self, id: &str) -> Result<()> {
        fs::remove_file(self.intent_path(id)).map_err(io_error("complete material intent"))?;
        sync_directory(&self.receipt_root)
    }
    /// Resolve an intent whose effect returned a *deterministic* error: the
    /// inner control plane returned (no unknown partial state), so the intent
    /// is closed as failed and retained as journal evidence (`.failed.json`).
    /// A crash still leaves `.pending`/`.writing`, which keeps refusing new
    /// effects until a human reconciles — this path never touches that law.
    fn resolve_failed(&self, id: &str, operation: &str, error: &WorkcellError) -> Result<()> {
        let pending = self.intent_path(id);
        let resolved = self.receipt_root.join(format!("{}.failed", key(id)));
        let record = json!({
            "schema": "workcell.material-intent-failed/v1",
            "id": id,
            "operation": operation,
            "error_kind": error_kind_name(error),
            "error": error.to_string(),
            "resolved_at_unix_ms": std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_millis() as u64)
                .unwrap_or(0),
        });
        fs::write(&resolved, record.to_string())
            .map_err(io_error("record failed material intent"))?;
        fs::remove_file(&pending).map_err(io_error("close failed material intent"))?;
        sync_directory(&self.receipt_root)
    }
    fn persist_world(&self, world: &MaterialisedExecutionWorld) -> Result<()> {
        let destination = self.receipt_path(&world.world_ref);
        let temporary = destination.with_extension("writing");
        // A host lock excludes other writers. create_new refuses an interrupted
        // temporary publication instead of overwriting its remaining evidence.
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&temporary)
            .map_err(io_error("create receipt publication"))?;
        file.write_all(encode_world(world)?.as_bytes())
            .map_err(io_error("write material receipt"))?;
        file.sync_all().map_err(io_error("sync material receipt"))?;
        fs::rename(&temporary, &destination).map_err(io_error("publish material receipt"))?;
        sync_directory(&self.receipt_root)
    }
    fn mutable_world(&self, world: &WorldRef) -> Result<MaterialisedExecutionWorld> {
        let value = self.inner.inspect(world)?;
        if let Some(successor) = value.provenance.get("superseded_by") {
            return Err(WorkcellError::OperationFailed(format!(
                "material world was superseded by `{successor}`; resolve that binding explicitly"
            )));
        }
        Ok(value)
    }
}
impl WorkcellControlPlane for DurableCollapsedLocalWorkcell {
    fn discover(&self) -> Result<Discovery> {
        self.inner.discover()
    }
    fn plan(&self, demand: &ExecutionDemand) -> Result<MaterialisationPlan> {
        self.inner.plan(demand)
    }
    fn prepare(&mut self, demand: &ExecutionDemand) -> Result<MaterialisedExecutionWorld> {
        demand.validate()?;
        let fingerprint = format!(
            "sha256:{:x}",
            Sha256::digest(demand_value(demand).to_string().as_bytes())
        );
        if let Some(reference) = self.worlds.get(demand.demand_ref.as_str()) {
            let world = self.inner.inspect(reference)?;
            if world.provenance.get("demand_fingerprint") != Some(&fingerprint) {
                return Err(WorkcellError::InvalidDemand("same demand_ref has a different or legacy-unverifiable requirement basis; supply a new attempt ref".into()));
            }
            if world
                .binding_graph
                .bindings
                .iter()
                .any(|b| b.presence == BindingPresence::Released)
            {
                return Err(WorkcellError::OperationFailed(
                    "released material demand cannot be replayed; request a new attempt".into(),
                ));
            }
            if self.intent_path(demand.demand_ref.as_str()).exists() {
                let intent: serde_json::Value = serde_json::from_str(
                    &fs::read_to_string(self.intent_path(demand.demand_ref.as_str()))
                        .map_err(io_error("read completed prepare intent"))?,
                )
                .map_err(|_| WorkcellError::InvalidDemand("invalid material journal".into()))?;
                if intent["operation"] != "prepare" || intent["payload"] != demand_value(demand) {
                    return Err(WorkcellError::OperationFailed(
                        "material intent does not match completed prepare receipt".into(),
                    ));
                }
                self.finish(demand.demand_ref.as_str())?;
            }
            // A completed receipt proves allocation, not current process health.
            return Ok(world);
        }
        let plan = self.inner.plan(demand)?;
        if plan.status == epilogos_workcell_core::PlanStatus::Unsatisfiable {
            return Err(WorkcellError::UnsatisfiedDemand(
                "material demand is unsatisfiable; no preparation effects attempted".into(),
            ));
        }
        self.begin(demand.demand_ref.as_str(), "prepare", demand_value(demand))?;
        let prepared = self.inner.prepare(demand);
        let mut world = match prepared {
            Ok(world) => world,
            // A deterministic refusal (planning, admission, provider) closed
            // the inner operation cleanly: resolve the intent with the
            // failure retained as journal evidence.
            Err(error) => {
                self.resolve_failed(demand.demand_ref.as_str(), "prepare", &error)?;
                return Err(error);
            }
        };
        world
            .provenance
            .insert("demand_fingerprint".into(), fingerprint);
        self.inner.register_world(world.clone())?;
        self.persist_world(&world)?;
        self.worlds
            .insert(demand.demand_ref.to_string(), world.world_ref.clone());
        self.finish(demand.demand_ref.as_str())?;
        Ok(world)
    }
    fn inspect(&self, world: &WorldRef) -> Result<MaterialisedExecutionWorld> {
        self.inner.inspect(world)
    }
    fn observe(&self, world: &WorldRef) -> Result<ObservationBundle> {
        self.inner.observe(world)
    }
    fn expose(&self, world: &WorldRef) -> Result<ExposureBundle> {
        self.inner.expose(world)
    }
    fn collect(&self, world: &WorldRef) -> Result<CollectionBundle> {
        self.inner.collect(world)
    }
    fn recover(&mut self, world: &WorldRef) -> Result<MaterialisedExecutionWorld> {
        let before = self.mutable_world(world)?;
        if before
            .binding_graph
            .bindings
            .iter()
            .any(|b| b.presence == BindingPresence::Released)
        {
            return Err(WorkcellError::OperationFailed(
                "recovery cannot revive released material or its authority".into(),
            ));
        }
        self.begin(
            world.as_str(),
            "recover",
            json!({"world_ref":world.as_str()}),
        )?;
        let result = self.inner.recover(world);
        match result {
            Ok(mut recovered) => {
                if let Some(value) = before.provenance.get("demand_fingerprint") {
                    recovered
                        .provenance
                        .insert("demand_fingerprint".into(), value.clone());
                }
                // If a rematerialisation occurred, the old receipt becomes
                // history. Publication interruption leaves the intent visible.
                if recovered.world_ref != *world {
                    let mut history = before;
                    history
                        .provenance
                        .insert("superseded_by".into(), recovered.world_ref.to_string());
                    self.inner.register_world(history.clone())?;
                    self.persist_world(&history)?;
                }
                self.inner.register_world(recovered.clone())?;
                self.persist_world(&recovered)?;
                self.worlds.insert(
                    recovered.demand_ref.to_string(),
                    recovered.world_ref.clone(),
                );
                self.finish(world.as_str())?;
                Ok(recovered)
            }
            Err(error) => Err(error), // retain intent: no invented rollback/replay
        }
    }
    fn release(&mut self, world: &WorldRef) -> Result<ReleaseResult> {
        self.mutable_world(world)?;
        self.begin(
            world.as_str(),
            "release",
            json!({"world_ref":world.as_str()}),
        )?;
        let result = self.inner.release(world);
        // Persist a successfully observed prefix even if a later release fails.
        // Tombstones prevent a lost response from recreating released work.
        self.persist_world(&self.inner.inspect(world)?)?;
        if result.is_ok() {
            self.finish(world.as_str())?;
        }
        result
    }
    fn reconcile(&mut self, desired: &[DesiredMaterialState]) -> Result<ReconciliationResult> {
        self.begin(
            "control:reconcile",
            "reconcile",
            epilogos_workcell_control::codec::desired_value(desired),
        )?;
        let result = self.inner.reconcile(desired);
        for reference in self.worlds.values() {
            self.persist_world(&self.inner.inspect(reference)?)?;
        }
        if result.is_ok() {
            self.finish("control:reconcile")?;
        }
        result
    }
}
/// The wire name of a Workcell error kind (mirrors the control codec).
fn error_kind_name(error: &WorkcellError) -> &'static str {
    match error {
        WorkcellError::InvalidDemand(_) => "invalid-demand",
        WorkcellError::UnsatisfiedDemand(_) => "unsatisfied-demand",
        WorkcellError::Unavailable(_) => "unavailable",
        WorkcellError::Degraded(_) => "degraded",
        WorkcellError::OperationFailed(_) => "operation-failed",
        WorkcellError::CleanupFailed(_) => "cleanup-failed",
        WorkcellError::ReconciliationFailed(_) => "reconciliation-failed",
        WorkcellError::NotFound(_) => "not-found",
        WorkcellError::Unsupported(_) => "unsupported",
        WorkcellError::Capacity(_) => "waiting-for-capacity",
    }
}

fn key(value: &str) -> String {
    format!("{:x}", Sha256::digest(value.as_bytes()))
}
fn io_error(context: &'static str) -> impl FnOnce(std::io::Error) -> WorkcellError {
    move |e| WorkcellError::OperationFailed(format!("{context}: {e}"))
}
fn sync_directory(path: &Path) -> Result<()> {
    #[cfg(unix)]
    {
        File::open(path)
            .and_then(|f| f.sync_all())
            .map_err(io_error("sync receipt directory"))?;
    }
    #[cfg(not(unix))]
    {
        let _ = path;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use epilogos_workcell_core::{
        AffordanceRequirement, DemandRef, ReleaseDisposition, WorkcellRef,
    };
    fn root() -> PathBuf {
        static TEST_ROOT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        std::env::temp_dir().join(format!(
            "workcell-durable-{}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos(),
            TEST_ROOT.fetch_add(1, std::sync::atomic::Ordering::Relaxed),
        ))
    }
    fn config(root: &Path) -> CollapsedLocalConfig {
        CollapsedLocalConfig::new(WorkcellRef::new("workcell:durable").unwrap(), root)
    }
    fn demand() -> ExecutionDemand {
        let mut d = ExecutionDemand::new(DemandRef::new("demand:durable").unwrap());
        d.affordances
            .required
            .push(AffordanceRequirement::new("shell").unwrap());
        d
    }
    #[test]
    fn restart_lost_response_inspect_and_release_tombstone() {
        let root = root();
        let mut host = DurableCollapsedLocalWorkcell::new(config(&root)).unwrap();
        let world = host.prepare(&demand()).unwrap();
        assert_eq!(world, host.prepare(&demand()).unwrap());
        assert!(DurableCollapsedLocalWorkcell::new(config(&root)).is_err());
        drop(host);
        let mut host = DurableCollapsedLocalWorkcell::new(config(&root)).unwrap();
        assert_eq!(host.inspect(&world.world_ref).unwrap(), world);
        assert_eq!(host.prepare(&demand()).unwrap(), world);
        let mut changed = demand();
        changed.extensions.insert("changed".into(), "basis".into());
        assert!(host.prepare(&changed).is_err());
        assert_eq!(
            host.release(&world.world_ref).unwrap().disposition,
            ReleaseDisposition::Released
        );
        drop(host);
        let mut host = DurableCollapsedLocalWorkcell::new(config(&root)).unwrap();
        assert!(host.prepare(&demand()).is_err());
        assert!(host.recover(&world.world_ref).is_err());
        assert!(!host.release(&world.world_ref).unwrap().changed);
        drop(host);
        fs::remove_dir_all(root).unwrap();
    }
    #[test]
    fn duplicate_descriptor_does_not_delay_restart_or_unlock_successor() {
        let root = root();
        let mut host = DurableCollapsedLocalWorkcell::new(config(&root)).unwrap();
        let world = host.prepare(&demand()).unwrap();
        let receipt = host.receipt_path(&world.world_ref);
        let receipt_before = fs::read(&receipt).unwrap();
        let old_descriptor = host._host_lock.file.try_clone().unwrap();
        assert!(DurableCollapsedLocalWorkcell::new(config(&root)).is_err());
        drop(host);
        // No sleep/acquisition retry: the original owner's explicit release
        // must suffice even while its duplicated description remains open.
        let mut successor = DurableCollapsedLocalWorkcell::new(config(&root)).unwrap();
        assert_eq!(successor.inspect(&world.world_ref).unwrap(), world);
        assert_eq!(successor.prepare(&demand()).unwrap(), world);
        assert_eq!(fs::read(&receipt).unwrap(), receipt_before);
        assert!(DurableCollapsedLocalWorkcell::new(config(&root)).is_err());
        drop(old_descriptor);
        assert!(DurableCollapsedLocalWorkcell::new(config(&root)).is_err());
        assert_eq!(
            successor.release(&world.world_ref).unwrap().disposition,
            ReleaseDisposition::Released
        );
        drop(successor);
        let mut restarted = DurableCollapsedLocalWorkcell::new(config(&root)).unwrap();
        assert!(restarted.prepare(&demand()).is_err());
        assert!(!restarted.release(&world.world_ref).unwrap().changed);
        drop(restarted);
        fs::remove_dir_all(root).unwrap();
    }
    #[test]
    #[cfg(unix)]
    fn inherited_guard_cannot_unlock_live_parent_or_delay_handover() {
        use std::{
            io::Read,
            os::fd::AsFd,
            os::unix::fs::MetadataExt,
            process::{Child, Command, Stdio},
            time::{Duration, Instant},
        };
        const PARENT: &str = "WORKCELL_DURABLE_TEST_LOCK_PARENT";
        const ROOT: &str = "WORKCELL_DURABLE_TEST_LOCK_ROOT";
        if let Some(parent) = std::env::var_os(PARENT) {
            let root = PathBuf::from(std::env::var_os(ROOT).unwrap());
            let owner_pid: u32 = parent.to_str().unwrap().parse().unwrap();
            assert_ne!(owner_pid, std::process::id());
            // A real native child has inherited the parent's locked open
            // description on stdin. Clone only that actual descriptor.
            let file = File::from(std::io::stdin().as_fd().try_clone_to_owned().unwrap());
            let inherited = file.metadata().unwrap();
            let actual = fs::metadata(root.join("control-worlds/host.lock")).unwrap();
            assert_eq!(
                (inherited.dev(), inherited.ino()),
                (actual.dev(), actual.ino())
            );
            assert!(DurableCollapsedLocalWorkcell::new(config(&root)).is_err());
            drop(MaterialHostLock { file, owner_pid });
            assert!(DurableCollapsedLocalWorkcell::new(config(&root)).is_err());
            // Parent observes guard disposal before attempting handover. The
            // child still owns stdin's inherited description until exit.
            fs::write(
                root.join("child-guard-dropped"),
                std::process::id().to_string(),
            )
            .unwrap();
            let deadline = Instant::now() + Duration::from_secs(5);
            while !root.join("child-stop").exists() {
                assert!(Instant::now() < deadline, "owned child stop deadline");
                std::thread::sleep(Duration::from_millis(5));
            }
            assert!(DurableCollapsedLocalWorkcell::new(config(&root)).is_err());
            return;
        }
        struct OwnedChild(Child);
        impl OwnedChild {
            // Read only after this exact child has exited. Its test output
            // contains controlled filesystem/process diagnostics, no auth.
            fn diagnostics(&mut self) -> String {
                let mut stdout = Vec::new();
                let mut stderr = Vec::new();
                if let Some(reader) = self.0.stdout.take() {
                    reader.take(64 * 1024).read_to_end(&mut stdout).unwrap();
                }
                if let Some(reader) = self.0.stderr.take() {
                    reader.take(64 * 1024).read_to_end(&mut stderr).unwrap();
                }
                format!(
                    "native child {} stdout={} stderr={}",
                    self.0.id(),
                    String::from_utf8_lossy(&stdout),
                    String::from_utf8_lossy(&stderr)
                )
            }
        }
        impl Drop for OwnedChild {
            fn drop(&mut self) {
                if self.0.try_wait().ok().flatten().is_none() {
                    let _ = self.0.kill();
                }
                let _ = self.0.wait();
            }
        }
        let root = root();
        let mut host = DurableCollapsedLocalWorkcell::new(config(&root)).unwrap();
        let world = host.prepare(&demand()).unwrap();
        let receipt = host.receipt_path(&world.world_ref);
        let receipt_before = fs::read(&receipt).unwrap();
        let mut child = OwnedChild(
            Command::new(std::env::current_exe().unwrap())
                .args([
                    "--exact",
                    "durable::tests::inherited_guard_cannot_unlock_live_parent_or_delay_handover",
                    "--nocapture",
                ])
                .env(PARENT, std::process::id().to_string())
                .env(ROOT, &root)
                .env_remove("CENTRAL_NATIVE_TOKEN")
                .env_remove("WORKCELL_CONTROL_TOKEN")
                .stdin(Stdio::from(host._host_lock.file.try_clone().unwrap()))
                .stdout(Stdio::piped())
                .stderr(Stdio::piped())
                .spawn()
                .unwrap(),
        );
        let deadline = Instant::now() + Duration::from_secs(3);
        // The guard note becomes visible on create before the child's write
        // lands; readiness is the complete note, not the file's existence.
        while fs::read_to_string(root.join("child-guard-dropped")).unwrap_or_default()
            != child.0.id().to_string()
        {
            if let Some(status) = child.0.try_wait().unwrap() {
                panic!(
                    "native child exited before guard disposal: {status}; {}",
                    child.diagnostics()
                );
            }
            assert!(Instant::now() < deadline, "native child readiness deadline");
            std::thread::sleep(Duration::from_millis(5));
        }
        assert_eq!(
            fs::read_to_string(root.join("child-guard-dropped")).unwrap(),
            child.0.id().to_string()
        );
        assert!(DurableCollapsedLocalWorkcell::new(config(&root)).is_err());
        drop(host);
        // The old child's actual stdin remains open. Native ownership still
        // hands over immediately after the old owner's resources are gone.
        assert!(child.0.try_wait().unwrap().is_none());
        let mut successor = DurableCollapsedLocalWorkcell::new(config(&root)).unwrap();
        assert_eq!(successor.inspect(&world.world_ref).unwrap(), world);
        assert_eq!(successor.prepare(&demand()).unwrap(), world);
        assert_eq!(fs::read(&receipt).unwrap(), receipt_before);
        fs::write(root.join("child-stop"), b"owned-stop").unwrap();
        let deadline = Instant::now() + Duration::from_secs(3);
        let status = loop {
            if let Some(status) = child.0.try_wait().unwrap() {
                break status;
            }
            assert!(Instant::now() < deadline, "native child exit deadline");
            std::thread::sleep(Duration::from_millis(5));
        };
        let diagnostics = child.diagnostics();
        assert!(
            status.success(),
            "native inherited-description case failed: {status}; {diagnostics}"
        );
        assert!(DurableCollapsedLocalWorkcell::new(config(&root)).is_err());
        assert_eq!(successor.inspect(&world.world_ref).unwrap(), world);
        drop(successor);
        drop(child);
        fs::remove_dir_all(root).unwrap();
    }
    #[test]
    fn interrupted_recovery_cannot_be_bypassed_by_another_demand() {
        let root = root();
        let mut host = DurableCollapsedLocalWorkcell::new(config(&root)).unwrap();
        let world = host.prepare(&demand()).unwrap();
        host.begin(world.world_ref.as_str(), "recover", json!({}))
            .unwrap();
        let mut history = world.clone();
        history
            .provenance
            .insert("superseded_by".into(), "world:unpublished".into());
        host.persist_world(&history).unwrap();
        drop(host);
        let mut host = DurableCollapsedLocalWorkcell::new(config(&root)).unwrap();
        assert!(host.prepare(&demand()).is_err());
        let mut another = demand();
        another.demand_ref = DemandRef::new("demand:another").unwrap();
        assert!(host.prepare(&another).is_err());
        assert_eq!(
            host.inspect(&world.world_ref).unwrap().subjects,
            world.subjects
        );
        drop(host);
        fs::remove_dir_all(root).unwrap();
    }
    #[test]
    fn interrupted_intent_refuses_replay_and_wrong_host_identity() {
        let root = root();
        let mut host = DurableCollapsedLocalWorkcell::new(config(&root)).unwrap();
        host.begin(demand().demand_ref.as_str(), "prepare", json!({}))
            .unwrap();
        assert!(host.prepare(&demand()).is_err());
        host.finish(demand().demand_ref.as_str()).unwrap();
        let world = host.prepare(&demand()).unwrap();
        drop(host);
        let other =
            CollapsedLocalConfig::new(WorkcellRef::new("workcell:other-placement").unwrap(), &root);
        assert!(DurableCollapsedLocalWorkcell::new(other).is_err());
        let host = DurableCollapsedLocalWorkcell::new(config(&root)).unwrap();
        assert_eq!(host.inspect(&world.world_ref).unwrap(), world);
        drop(host);
        fs::remove_dir_all(root).unwrap();
    }
}
