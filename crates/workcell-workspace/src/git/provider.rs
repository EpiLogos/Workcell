use std::{collections::BTreeMap, fs, path::PathBuf};

use epilogos_workcell_core::{
    Availability, HealthState, OfferRef, OperationalOffer, ProviderAllocation, ProviderObservation,
    ProviderPort, ProviderPortKind, ProviderReleaseResult, ReleaseDisposition, Result,
    RetentionExpectation, WorkcellError, WorkspaceAccess, WorkspaceMaterialRequest,
    WorkspaceProvider,
};

use super::{command, state::GitWorktreeWorkspaceProvider};
use crate::support::{make_directories_writable, set_tree_readonly, stable_key};

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
            // The dedicated `workspace:git-worktree` affordance is the only
            // workspace affordance this provider offers: a plain
            // `workspace:writable` demand keeps binding to its existing
            // provider, and only a demand that names the git-worktree key is
            // routed here.
            affordances: vec![
                "workspace:git-worktree".into(),
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

        if let Some(record) = self.records.borrow().get(&material_ref) {
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

        // Branch law: a writable demand that names a branch materialises on
        // that branch (`git worktree add -b <branch>`); everything else keeps
        // the detached checkout. git itself refuses an existing branch name,
        // so a collision fails loudly instead of reusing someone's branch.
        let branch = match (&request.branch_name, &request.access) {
            (Some(branch), WorkspaceAccess::Writable) => Some(branch.clone()),
            (Some(branch), WorkspaceAccess::ReadOnly) => {
                return Err(WorkcellError::InvalidDemand(format!(
                    "branch law names branch `{branch}` but the demand is read-only; a named branch is a writable materialisation"
                )))
            }
            (None, _) => None,
        };
        let mut worktree_args = vec![String::from("worktree"), String::from("add")];
        let branch_exists = branch.as_ref().is_some_and(|branch| {
            command::stdout(
                &repository,
                &[
                    "show-ref",
                    "--verify",
                    "--quiet",
                    &format!("refs/heads/{branch}"),
                ],
                "check whether the branch law branch already exists",
            )
            .is_ok()
        });
        match (&branch, branch_exists) {
            (Some(branch), true) => {
                // A re-materialised run resumes its existing branch and its
                // commits instead of refusing or forking a copy.
                worktree_args.push(command::path_arg(&target)?.to_owned());
                worktree_args.push(branch.clone());
            }
            (Some(branch), false) => {
                worktree_args.push("-b".into());
                worktree_args.push(branch.clone());
                worktree_args.push(command::path_arg(&target)?.to_owned());
                worktree_args.push(commit.clone());
            }
            (None, _) => {
                worktree_args.push("--detach".into());
                worktree_args.push(command::path_arg(&target)?.to_owned());
                worktree_args.push(commit.clone());
            }
        }
        let worktree_args: Vec<&str> = worktree_args.iter().map(String::as_str).collect();
        command::run(&repository, &worktree_args, "create git worktree")?;
        if request.access == WorkspaceAccess::ReadOnly {
            set_tree_readonly(&target)?;
        }

        let record = super::state::GitRecord {
            repository,
            path: target,
            commit,
            branch,
            access: request.access.clone(),
            source_ref: request.source.as_ref().map(ToString::to_string),
            source_locator: source.locator.clone(),
            source_dirty,
        };
        let allocation = self.allocation(&material_ref, &record);
        self.records
            .borrow_mut()
            .insert(material_ref, record);
        self.persist()?;
        Ok(allocation)
    }

    fn observe_workspace(&self, allocation: &ProviderAllocation) -> Result<ProviderObservation> {
        let record = self.record(allocation)?;
        let mut detail = BTreeMap::new();
        detail.insert("path".into(), record.path.display().to_string());
        detail.insert("commit".into(), record.commit.clone());
        if !record.path.exists() {
            self.prune_if_gone(&allocation.material_ref, &record);
            detail.insert("exists".into(), "false".into());
            detail.insert("pruned".into(), "true".into());
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
        if let Some(branch) = &record.branch {
            detail.insert("branch".into(), branch.clone());
        }
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
        // Capture the record before any observation: observe prunes entries
        // whose path is gone, and the release contract for a missing worktree
        // (prune the repository's stale metadata) must keep working — a
        // tombstoned record still names the repository. Dirtiness is derived
        // directly from git here so a tombstoned record never blocks release.
        let record = self.record_for_release(allocation)?;
        let dirty = record.path.exists()
            && !command::stdout(
                &record.path,
                &["status", "--porcelain"],
                "inspect git worktree status",
            )?
            .is_empty();
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
                let changed = if record.path.exists() {
                    if record.access == WorkspaceAccess::ReadOnly {
                        make_directories_writable(&record.path)?;
                    }
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
                self.records.borrow_mut().remove(&allocation.material_ref);
                self.tombstones
                    .borrow_mut()
                    .remove(&allocation.material_ref);
                self.persist()?;
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
