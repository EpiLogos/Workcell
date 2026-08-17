use std::{collections::BTreeMap, fs, path::PathBuf};

use epilogos_workcell_core::{
    validate_allocation, Availability, HealthState, OfferRef, OperationalOffer, ProviderAllocation,
    ProviderObservation, ProviderPort, ProviderPortKind, ProviderRef, ProviderReleaseResult,
    ReleaseDisposition, Result, RetentionExpectation, WorkcellError, WorkspaceAccess,
    WorkspaceMaterialRequest, WorkspaceProvider,
};

use crate::support::{
    copy_tree, fingerprint_tree, make_directories_writable, require_directory, set_tree_readonly,
    stable_key,
};

#[derive(Clone, Debug)]
struct DirectoryRecord {
    path: PathBuf,
    baseline: Option<u64>,
    access: WorkspaceAccess,
    source_ref: Option<String>,
    source_locator: Option<String>,
    revision: Option<String>,
}

pub struct DirectoryWorkspaceProvider {
    provider_ref: ProviderRef,
    root: PathBuf,
    records: BTreeMap<String, DirectoryRecord>,
}

impl DirectoryWorkspaceProvider {
    pub fn new(provider_ref: ProviderRef, root: impl Into<PathBuf>) -> Self {
        Self {
            provider_ref,
            root: root.into(),
            records: BTreeMap::new(),
        }
    }

    fn allocation(&self, material_ref: &str, record: &DirectoryRecord) -> ProviderAllocation {
        let mut properties = BTreeMap::new();
        properties.insert("path".into(), record.path.display().to_string());
        properties.insert(
            "access".into(),
            match record.access {
                WorkspaceAccess::ReadOnly => "read-only",
                WorkspaceAccess::Writable => "writable",
            }
            .into(),
        );

        let mut provenance = BTreeMap::new();
        provenance.insert("provider_kind".into(), "directory".into());
        provenance.insert("source_dirty".into(), "unknown".into());
        if let Some(value) = record.baseline {
            provenance.insert("baseline_fingerprint".into(), value.to_string());
        }
        if let Some(value) = &record.source_ref {
            provenance.insert("source_ref".into(), value.clone());
        }
        if let Some(value) = &record.source_locator {
            provenance.insert("source_locator".into(), value.clone());
        }
        if let Some(value) = &record.revision {
            provenance.insert("source_revision".into(), value.clone());
        }

        ProviderAllocation {
            provider_ref: self.provider_ref.clone(),
            port: ProviderPortKind::Workspace,
            material_ref: material_ref.into(),
            health: HealthState::Healthy,
            properties,
            provenance,
        }
    }

    fn record(&self, allocation: &ProviderAllocation) -> Result<DirectoryRecord> {
        validate_allocation(self, allocation)?;
        if let Some(record) = self.records.get(&allocation.material_ref) {
            return Ok(record.clone());
        }

        let path = allocation.properties.get("path").ok_or_else(|| {
            WorkcellError::OperationFailed(format!(
                "persisted directory workspace `{}` has no path property",
                allocation.material_ref
            ))
        })?;
        let access = match allocation.properties.get("access").map(String::as_str) {
            Some("read-only") => WorkspaceAccess::ReadOnly,
            Some("writable") => WorkspaceAccess::Writable,
            other => {
                return Err(WorkcellError::OperationFailed(format!(
                    "persisted directory workspace `{}` has invalid access property {other:?}",
                    allocation.material_ref
                )))
            }
        };
        let baseline = allocation
            .provenance
            .get("baseline_fingerprint")
            .map(|value| {
                value.parse::<u64>().map_err(|error| {
                    WorkcellError::OperationFailed(format!(
                        "persisted directory workspace `{}` has invalid baseline fingerprint: {error}",
                        allocation.material_ref
                    ))
                })
            })
            .transpose()?;

        Ok(DirectoryRecord {
            path: PathBuf::from(path),
            baseline,
            access,
            source_ref: allocation.provenance.get("source_ref").cloned(),
            source_locator: allocation.provenance.get("source_locator").cloned(),
            revision: allocation.provenance.get("source_revision").cloned(),
        })
    }
}

impl ProviderPort for DirectoryWorkspaceProvider {
    fn provider_ref(&self) -> &ProviderRef {
        &self.provider_ref
    }

    fn port_kind(&self) -> ProviderPortKind {
        ProviderPortKind::Workspace
    }

    fn offers(&self) -> Result<Vec<OperationalOffer>> {
        let mut metadata = BTreeMap::new();
        metadata.insert("implementation".into(), "directory".into());
        metadata.insert("root".into(), self.root.display().to_string());
        Ok(vec![OperationalOffer {
            offer_ref: OfferRef::new(format!("offer:{}:directory", self.provider_ref))
                .map_err(|error| WorkcellError::OperationFailed(error.into()))?,
            provider_ref: self.provider_ref.clone(),
            port: ProviderPortKind::Workspace.as_str().into(),
            affordances: vec![
                "workspace:read-only".into(),
                "workspace:writable".into(),
                "persistence:ephemeral".into(),
                "persistence:task-or-run".into(),
                "persistence:candidate".into(),
                "persistence:project".into(),
                "persistence:workcell".into(),
                "retention:preserve".into(),
            ],
            connections: vec![],
            exposures: vec![],
            isolation_trust: vec![],
            availability: Availability::Available,
            health: HealthState::Healthy,
            capacity: BTreeMap::new(),
            metadata,
        }])
    }
}

impl WorkspaceProvider for DirectoryWorkspaceProvider {
    fn prepare_workspace(
        &mut self,
        request: &WorkspaceMaterialRequest,
    ) -> Result<ProviderAllocation> {
        let locator = request
            .material_source
            .as_ref()
            .map(|source| source.locator.as_str())
            .unwrap_or("");
        let revision = request.revision.as_deref().unwrap_or("");
        let access = match request.access {
            WorkspaceAccess::ReadOnly => "read-only",
            WorkspaceAccess::Writable => "writable",
        };
        let key = stable_key(&[request.demand_ref.as_str(), locator, revision, access]);
        let material_ref = format!("workspace:directory:{key}");

        if let Some(record) = self.records.get(&material_ref) {
            if record.path.exists() {
                return Ok(self.allocation(&material_ref, record));
            }
        }

        fs::create_dir_all(&self.root).map_err(|error| {
            WorkcellError::OperationFailed(format!("create directory provider root: {error}"))
        })?;
        let target = self.root.join(&key);
        if target.exists() {
            return Err(WorkcellError::OperationFailed(format!(
                "workspace target `{}` already exists without an owned allocation",
                target.display()
            )));
        }

        if let Some(source) = &request.material_source {
            let source_path = PathBuf::from(&source.locator);
            require_directory(&source_path)?;
            if let Err(error) = copy_tree(&source_path, &target) {
                let _ = fs::remove_dir_all(&target);
                return Err(error);
            }
        } else {
            fs::create_dir_all(&target).map_err(|error| {
                WorkcellError::OperationFailed(format!("create empty workspace: {error}"))
            })?;
        }

        let baseline = fingerprint_tree(&target)?;
        if request.access == WorkspaceAccess::ReadOnly {
            set_tree_readonly(&target)?;
        }

        let record = DirectoryRecord {
            path: target,
            baseline: Some(baseline),
            access: request.access.clone(),
            source_ref: request.source.as_ref().map(ToString::to_string),
            source_locator: request
                .material_source
                .as_ref()
                .map(|source| source.locator.clone()),
            revision: request.revision.clone(),
        };
        let allocation = self.allocation(&material_ref, &record);
        self.records.insert(material_ref, record);
        Ok(allocation)
    }

    fn observe_workspace(&self, allocation: &ProviderAllocation) -> Result<ProviderObservation> {
        let record = self.record(allocation)?;
        let mut detail = BTreeMap::new();
        detail.insert("path".into(), record.path.display().to_string());
        if !record.path.exists() {
            detail.insert("exists".into(), "false".into());
            detail.insert("dirty".into(), "unknown".into());
            return Ok(ProviderObservation {
                provider_ref: self.provider_ref.clone(),
                material_ref: allocation.material_ref.clone(),
                health: HealthState::Unavailable,
                detail,
            });
        }

        let dirty = record
            .baseline
            .map(|baseline| fingerprint_tree(&record.path).map(|current| current != baseline))
            .transpose()?
            .map_or_else(|| "unknown".into(), |value| value.to_string());
        detail.insert("exists".into(), "true".into());
        detail.insert("dirty".into(), dirty);
        Ok(ProviderObservation {
            provider_ref: self.provider_ref.clone(),
            material_ref: allocation.material_ref.clone(),
            health: HealthState::Healthy,
            detail,
        })
    }

    fn release_workspace(
        &mut self,
        allocation: &ProviderAllocation,
        retention: &RetentionExpectation,
    ) -> Result<ProviderReleaseResult> {
        let observation = self.observe_workspace(allocation)?;
        let dirty = observation.detail.get("dirty").map(String::as_str);
        match retention {
            RetentionExpectation::Preserve => Ok(ProviderReleaseResult {
                provider_ref: self.provider_ref.clone(),
                material_ref: allocation.material_ref.clone(),
                disposition: ReleaseDisposition::Preserved,
                changed: false,
            }),
            RetentionExpectation::SuspendIfSupported
            | RetentionExpectation::SnapshotIfSupported => Err(WorkcellError::Unsupported(
                "directory workspace provider does not support suspend/snapshot".into(),
            )),
            RetentionExpectation::Release => {
                if dirty == Some("true") {
                    return Err(WorkcellError::CleanupFailed(
                        "directory workspace is dirty; refusing silent discard".into(),
                    ));
                }
                if observation.detail.get("exists").map(String::as_str) == Some("true")
                    && dirty == Some("unknown")
                {
                    return Err(WorkcellError::CleanupFailed(
                        "directory workspace baseline is unavailable; refusing unverifiable discard"
                            .into(),
                    ));
                }
                let record = self.record(allocation)?;
                let changed = if record.path.exists() {
                    if record.access == WorkspaceAccess::ReadOnly {
                        make_directories_writable(&record.path)?;
                    }
                    fs::remove_dir_all(&record.path).map_err(|error| {
                        WorkcellError::CleanupFailed(format!(
                            "remove directory workspace `{}`: {error}",
                            record.path.display()
                        ))
                    })?;
                    true
                } else {
                    false
                };
                self.records.remove(&allocation.material_ref);
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
