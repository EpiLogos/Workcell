use std::{collections::BTreeMap, fs, path::PathBuf};

use epilogos_workcell_core::{
    Availability, HealthState, OfferRef, OperationalOffer, ProviderAllocation, ProviderObservation,
    ProviderPort, ProviderPortKind, ProviderReleaseResult, ReleaseDisposition, Result,
    RetentionExpectation, WorkcellError, WorkspaceAccess, WorkspaceMaterialRequest,
    WorkspaceProvider,
};

use super::{command, state::GitRecord, state::GitWorktreeWorkspaceProvider};
use crate::support::{set_files_readonly, stable_key};

impl ProviderPort for GitWorktreeWorkspaceProvider {
    fn provider_ref(&self) -> &epilogos_workcell_core::ProviderRef {
        &self.provider_ref
    }

    fn port_kind(&self) -> ProviderPortKind {
        ProviderPortKind::Workspace
    }

    fn offers(&self) -> Result<Vec<OperationalOffer>> {
        let available = command::available();
        let mut metadata = BTreeMap::new();
        metadata.insert("implementation".into(), "git-worktree".into());
        metadata.insert("root".into(), self.root.display().to_string());
        Ok(vec![OperationalOffer {
            offer_ref: OfferRef::new(format!("offer:{}:git-worktree", self.provider_ref))
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
            availability: if available {
                Availability::Available
            } else {
                Availability::Unavailable
            },
            health: if available {
                HealthState::Healthy
            } else {
                HealthState::Unavailable
            },
            capacity: BTreeMap::new(),
            metadata,
        }])
    }
}

impl WorkspaceProvider for GitWorktreeWorkspaceProvider {
    fn prepare_workspace(
        &mut self,
        request: &WorkspaceMaterialRequest,
    ) -> Result<ProviderAllocation> {
        let source = request.material_source.as_ref().ok_or_else(|| {
            WorkcellError::InvalidDemand(
                "git worktree provider requires a material source locator".into(),
            )
        })?;
        if source.locator.trim().is_empty() {
            return Err(WorkcellError::InvalidDemand(
                "git material source locator must not be empty".into(),
            ));
        }

        let repository = PathBuf::from(command::stdout(
            &PathBuf::from(&source.locator),
            &["rev-parse", "--show-toplevel"],
            "resolve git repository",
        )?);
        let requested_revision = request.revision.as_deref().unwrap_or("HEAD");
        let commit_query = format!("{requested_revision}^{{commit}}");
        let commit = command::stdout(
            &repository,
            &["rev-parse", "--verify", &commit_query],
            "resolve git revision",
        )
        .map_err(|_| {
            WorkcellError::Unavailable(format!(
                "git revision `{requested_revision}` is not available"
            ))
        })?;
        let source_dirty = !command::stdout(
            &repository,
            &["status", "--porcelain"],
            "inspect git source status",
        )?
        .is_empty();
        let access = match request.access {
            WorkspaceAccess::ReadOnly => "read-only",
            WorkspaceAccess::Writable => "writable",
        };
        let key = stable_key(&[
            request.demand_ref.as_str(),
            repository.to_string_lossy().as_ref(),
            &commit,
            access,
        ]);
        let material_ref = format!("workspace:git-worktree:{key}");

        if let Some(record) = self.records.get(&material_ref) {
            if record.path.exists() {
                return Ok(self.allocation(&material_ref, record));
            }
        }

        fs::create_dir_all(&self.root).map_err(|error| {
            WorkcellError::OperationFailed(format!("create git workspace root: {error}"))
        })?;
        let target = self.root.join(&key);
        if target.exists() {
            return Err(WorkcellError::OperationFailed(format!(
                "git workspace target `{}` exists without an owned allocation",
                target.display()
            )));
        }

        command::run(
            &repository,
            &[
                "worktree",
                "add",
                "--detach",
                command::path_arg(&target)?,
                &commit,
            ],
            "create git worktree",
        )?;
        if request.access == WorkspaceAccess::ReadOnly {
            set_files_readonly(&target)?;
        }

        let record = GitRecord {
            repository,
            path: target,
            commit,
            access: request.access.clone(),
            source_ref: request.source.as_ref().map(ToString::to_string),
            source_locator: source.locator.clone(),
            source_dirty,
        };
        let allocation = self.allocation(&material_ref, &record);
        self.records.insert(material_ref, record);
        Ok(allocation)
    }

    fn observe_workspace(&self, allocation: &ProviderAllocation) -> Result<ProviderObservation> {
        let record = self.record(allocation)?;
        let mut detail = BTreeMap::new();
        detail.insert("path".into(), record.path.display().to_string());
        detail.insert("commit".into(), record.commit.clone());
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

        let dirty = !command::stdout(
            &record.path,
            &["status", "--porcelain"],
            "inspect git worktree status",
        )?
        .is_empty();
        detail.insert("exists".into(), "true".into());
        detail.insert("dirty".into(), dirty.to_string());
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
        let dirty = observation
            .detail
            .get("dirty")
            .is_some_and(|value| value == "true");
        match retention {
            RetentionExpectation::Preserve => Ok(ProviderReleaseResult {
                provider_ref: self.provider_ref.clone(),
                material_ref: allocation.material_ref.clone(),
                disposition: ReleaseDisposition::Preserved,
                changed: false,
            }),
            RetentionExpectation::SuspendIfSupported
            | RetentionExpectation::SnapshotIfSupported => Err(WorkcellError::Unsupported(
                "git worktree provider does not support suspend/snapshot".into(),
            )),
            RetentionExpectation::Release => {
                if dirty {
                    return Err(WorkcellError::CleanupFailed(
                        "git worktree is dirty; refusing silent discard".into(),
                    ));
                }
                let record = self.record(allocation)?.clone();
                let changed = if record.path.exists() {
                    command::run(
                        &record.repository,
                        &["worktree", "remove", command::path_arg(&record.path)?],
                        "remove git worktree",
                    )?;
                    true
                } else {
                    command::run(
                        &record.repository,
                        &["worktree", "prune"],
                        "prune missing git worktree",
                    )?;
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
