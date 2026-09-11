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

/// One material host owns a store at a time. The kernel lock releases on process
/// death; it is not a stale PID lock. Completed receipts and release tombstones
/// survive disconnects. An interrupted effect is never silently re-dispatched.
pub struct DurableCollapsedLocalWorkcell {
    inner: CollapsedLocalWorkcell,
    receipt_root: PathBuf,
    worlds: BTreeMap<String, WorldRef>,
    receipt_paths: BTreeMap<String, PathBuf>,
    _host_lock: File,
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
        let mut world = self.inner.prepare(demand)?;
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
        host.prepare(&demand()).unwrap();
        drop(host);
        let other =
            CollapsedLocalConfig::new(WorkcellRef::new("workcell:other-placement").unwrap(), &root);
        assert!(DurableCollapsedLocalWorkcell::new(other).is_err());
        fs::remove_dir_all(root).unwrap();
    }
}
