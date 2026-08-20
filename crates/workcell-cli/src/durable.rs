use std::{fs, path::PathBuf};

use epilogos_workcell_core::{
    CollectionBundle, DesiredMaterialState, Discovery, ExecutionDemand, ExposureBundle,
    MaterialisationPlan, MaterialisedExecutionWorld, ObservationBundle, ReconciliationResult,
    ReleaseDisposition, ReleaseResult, Result, WorkcellControlPlane, WorkcellError, WorldRef,
};
use epilogos_workcell_runtime::{CollapsedLocalConfig, CollapsedLocalWorkcell};
use epilogos_workcell_wire::{decode_world, encode_world};

/// Durable host composition for the optional Workcell Control Service.
///
/// `CollapsedLocalWorkcell` remains the zero-daemon application. This adapter
/// only gives a long-running/restarted service process a receipt store for
/// material-world re-entry; it does not add planner or semantic identity.
pub struct DurableCollapsedLocalWorkcell {
    inner: CollapsedLocalWorkcell,
    receipt_root: PathBuf,
}

impl DurableCollapsedLocalWorkcell {
    pub fn new(config: CollapsedLocalConfig) -> Result<Self> {
        let receipt_root = config.state_root.join("control-worlds");
        fs::create_dir_all(&receipt_root).map_err(|error| {
            WorkcellError::OperationFailed(format!(
                "create control-service world receipt root: {error}"
            ))
        })?;
        let mut inner = CollapsedLocalWorkcell::new(config)?;
        for entry in fs::read_dir(&receipt_root).map_err(|error| {
            WorkcellError::OperationFailed(format!("read control-service world receipts: {error}"))
        })? {
            let entry = entry.map_err(|error| {
                WorkcellError::OperationFailed(format!(
                    "read control-service world receipt entry: {error}"
                ))
            })?;
            if !entry
                .file_type()
                .map_err(|error| {
                    WorkcellError::OperationFailed(format!(
                        "inspect control-service world receipt: {error}"
                    ))
                })?
                .is_file()
            {
                continue;
            }
            if entry.path().extension().and_then(|value| value.to_str()) != Some("json") {
                continue;
            }
            let encoded = fs::read_to_string(entry.path()).map_err(|error| {
                WorkcellError::OperationFailed(format!(
                    "read control-service world receipt `{}`: {error}",
                    entry.path().display()
                ))
            })?;
            let world = decode_world(&encoded)?;
            inner.register_world(world)?;
        }
        Ok(Self {
            inner,
            receipt_root,
        })
    }

    pub fn inner(&self) -> &CollapsedLocalWorkcell {
        &self.inner
    }

    fn receipt_path(&self, world: &WorldRef) -> PathBuf {
        self.receipt_root
            .join(format!("{}.json", safe_key(world.as_str())))
    }

    fn persist_world(&self, world: &MaterialisedExecutionWorld) -> Result<()> {
        let destination = self.receipt_path(&world.world_ref);
        let temporary = destination.with_extension(format!("json.tmp-{}", std::process::id()));
        fs::write(&temporary, encode_world(world)?).map_err(|error| {
            WorkcellError::OperationFailed(format!(
                "write control-service world receipt `{}`: {error}",
                temporary.display()
            ))
        })?;
        fs::rename(&temporary, &destination).map_err(|error| {
            let _ = fs::remove_file(&temporary);
            WorkcellError::OperationFailed(format!(
                "commit control-service world receipt `{}`: {error}",
                destination.display()
            ))
        })
    }

    fn remove_receipt(&self, world: &WorldRef) -> Result<()> {
        let path = self.receipt_path(world);
        match fs::remove_file(&path) {
            Ok(()) => Ok(()),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(error) => Err(WorkcellError::CleanupFailed(format!(
                "remove control-service world receipt `{}`: {error}",
                path.display()
            ))),
        }
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
        let world = self.inner.prepare(demand)?;
        self.persist_world(&world)?;
        Ok(world)
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

    fn release(&mut self, world: &WorldRef) -> Result<ReleaseResult> {
        let result = self.inner.release(world)?;
        if result.disposition == ReleaseDisposition::Released {
            self.remove_receipt(world)?;
        }
        Ok(result)
    }

    fn reconcile(&mut self, desired: &[DesiredMaterialState]) -> Result<ReconciliationResult> {
        self.inner.reconcile(desired)
    }
}

fn safe_key(value: &str) -> String {
    value
        .bytes()
        .map(|byte| match byte {
            b'a'..=b'z' | b'A'..=b'Z' | b'0'..=b'9' | b'-' | b'_' | b'.' => byte as char,
            _ => '_',
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use std::{
        path::Path,
        time::{SystemTime, UNIX_EPOCH},
    };

    use epilogos_workcell_core::{AffordanceRequirement, DemandRef, ExecutionDemand, WorkcellRef};

    use super::*;

    fn temp_root() -> PathBuf {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        std::env::temp_dir().join(format!(
            "epilogos-workcell-durable-control-{}-{nonce}",
            std::process::id()
        ))
    }

    fn config(root: &Path) -> CollapsedLocalConfig {
        CollapsedLocalConfig::new(
            WorkcellRef::new("workcell:durable-control-test").unwrap(),
            root,
        )
    }

    #[test]
    fn service_host_restart_restores_world_identity_without_manual_registration() {
        let root = temp_root();
        let mut demand = ExecutionDemand::new(DemandRef::new("demand:restart").unwrap());
        demand
            .affordances
            .required
            .push(AffordanceRequirement::new("shell").unwrap());

        let mut first = DurableCollapsedLocalWorkcell::new(config(&root)).unwrap();
        let world = first.prepare(&demand).unwrap();
        let world_ref = world.world_ref.clone();
        assert!(first.receipt_path(&world_ref).is_file());
        drop(first);

        let mut restarted = DurableCollapsedLocalWorkcell::new(config(&root)).unwrap();
        let observed = restarted.observe(&world_ref).unwrap();
        assert_eq!(observed.world_ref, world_ref);
        let released = restarted.release(&world_ref).unwrap();
        assert_eq!(released.disposition, ReleaseDisposition::Released);
        assert!(!restarted.receipt_path(&world_ref).exists());

        let _ = fs::remove_dir_all(root);
    }
}
