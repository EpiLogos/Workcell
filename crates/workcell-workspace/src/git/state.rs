use std::{
    cell::RefCell,
    collections::BTreeMap,
    fs,
    path::{Path, PathBuf},
};

use epilogos_workcell_core::{
    HealthState, ProviderAllocation, ProviderPortKind, ProviderRef, Result, WorkcellError,
    WorkspaceAccess,
};
use serde_json::{json, Value};

/// The durable journal schema for git worktree allocations.
///
/// Records survive process restarts: the provider re-reads this journal at
/// construction, rewrites it on every prepare/release, and prunes entries whose
/// material path has disappeared when they are observed. The journal is the
/// provider's memory — the git checkout itself remains the truth about
/// dirtiness and existence, which `observe_workspace` re-derives from disk.
pub const GIT_WORKTREES_SCHEMA: &str = "workcell.git-worktrees/v1";
/// Conventional journal location: `<workspace root>/git-worktrees.json`, which
/// under the collapsed-local composition is `<state-root>/workspaces/git-worktrees.json`.
pub const GIT_WORKTREES_JOURNAL_FILE: &str = "git-worktrees.json";

#[derive(Clone, Debug)]
pub(super) struct GitRecord {
    pub(super) repository: PathBuf,
    pub(super) path: PathBuf,
    pub(super) commit: String,
    /// The named branch a branch-law worktree materialised on; `None` for a
    /// detached checkout.
    pub(super) branch: Option<String>,
    pub(super) access: WorkspaceAccess,
    pub(super) source_ref: Option<String>,
    pub(super) source_locator: String,
    pub(super) source_dirty: bool,
}

impl GitRecord {
    fn to_journal_value(&self, material_ref: &str) -> Value {
        json!({
            "material_ref": material_ref,
            "repository": self.repository.display().to_string(),
            "path": self.path.display().to_string(),
            "commit": self.commit,
            "branch": self.branch,
            "access": match self.access {
                WorkspaceAccess::ReadOnly => "read-only",
                WorkspaceAccess::Writable => "writable",
            },
            "source_ref": self.source_ref,
            "source_locator": self.source_locator,
            "source_dirty": self.source_dirty,
        })
    }

    fn from_journal_value(value: &Value) -> Result<(String, GitRecord)> {
        let object = value.as_object().ok_or_else(|| {
            WorkcellError::OperationFailed("git worktree journal entry must be an object".into())
        })?;
        let required = |key: &str| -> Result<String> {
            object
                .get(key)
                .and_then(Value::as_str)
                .map(str::to_owned)
                .ok_or_else(|| {
                    WorkcellError::OperationFailed(format!(
                        "git worktree journal entry field `{key}` must be a non-empty string"
                    ))
                })
        };
        let access = match required("access")?.as_str() {
            "read-only" => WorkspaceAccess::ReadOnly,
            "writable" => WorkspaceAccess::Writable,
            other => {
                return Err(WorkcellError::OperationFailed(format!(
                    "git worktree journal entry has unknown access `{other}`"
                )))
            }
        };
        let branch = match object.get("branch") {
            None | Some(Value::Null) => None,
            Some(Value::String(branch)) if !branch.is_empty() => Some(branch.clone()),
            Some(_) => {
                return Err(WorkcellError::OperationFailed(
                    "git worktree journal entry field `branch` must be a string or null".into(),
                ))
            }
        };
        Ok((
            required("material_ref")?,
            GitRecord {
                repository: PathBuf::from(required("repository")?),
                path: PathBuf::from(required("path")?),
                commit: required("commit")?,
                branch,
                access,
                source_ref: object
                    .get("source_ref")
                    .and_then(Value::as_str)
                    .map(str::to_owned),
                source_locator: required("source_locator")?,
                source_dirty: object
                    .get("source_dirty")
                    .and_then(Value::as_bool)
                    .ok_or_else(|| {
                        WorkcellError::OperationFailed(
                            "git worktree journal entry field `source_dirty` must be a boolean"
                                .into(),
                        )
                    })?,
            },
        ))
    }
}

fn load_journal(path: &Path) -> Result<BTreeMap<String, GitRecord>> {
    let encoded = match fs::read_to_string(path) {
        Ok(encoded) => encoded,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(BTreeMap::new()),
        Err(error) => {
            return Err(WorkcellError::OperationFailed(format!(
                "read git worktree journal `{}`: {error}",
                path.display()
            )))
        }
    };
    let value: Value = serde_json::from_str(&encoded).map_err(|error| {
        WorkcellError::OperationFailed(format!(
            "parse git worktree journal `{}`: {error}",
            path.display()
        ))
    })?;
    if value.get("schema").and_then(Value::as_str) != Some(GIT_WORKTREES_SCHEMA) {
        return Err(WorkcellError::OperationFailed(format!(
            "git worktree journal `{}` does not declare {GIT_WORKTREES_SCHEMA}",
            path.display()
        )));
    }
    let entries = value
        .get("worktrees")
        .and_then(Value::as_array)
        .ok_or_else(|| {
            WorkcellError::OperationFailed(
                "git worktree journal must carry a `worktrees` array".into(),
            )
        })?;
    let mut records = BTreeMap::new();
    for entry in entries {
        let (material_ref, record) = GitRecord::from_journal_value(entry)?;
        if records.insert(material_ref.clone(), record).is_some() {
            return Err(WorkcellError::OperationFailed(format!(
                "git worktree journal holds `{material_ref}` more than once"
            )));
        }
    }
    Ok(records)
}

fn write_journal(path: &Path, records: &BTreeMap<String, GitRecord>) -> Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|error| {
            WorkcellError::OperationFailed(format!(
                "create git worktree journal directory `{}`: {error}",
                parent.display()
            ))
        })?;
    }
    let mut entries: Vec<Value> = records
        .iter()
        .map(|(material_ref, record)| record.to_journal_value(material_ref))
        .collect();
    entries.sort_by(|left, right| {
        left["material_ref"]
            .as_str()
            .cmp(&right["material_ref"].as_str())
    });
    let document = json!({
        "schema": GIT_WORKTREES_SCHEMA,
        "worktrees": entries,
    });
    let staged = path.with_extension("json.tmp");
    fs::write(
        &staged,
        serde_json::to_vec_pretty(&document).map_err(|error| {
            WorkcellError::OperationFailed(format!("encode git worktree journal: {error}"))
        })?,
    )
    .map_err(|error| {
        WorkcellError::OperationFailed(format!(
            "write git worktree journal `{}`: {error}",
            staged.display()
        ))
    })?;
    fs::rename(&staged, path).map_err(|error| {
        WorkcellError::OperationFailed(format!(
            "commit git worktree journal `{}`: {error}",
            path.display()
        ))
    })
}

pub struct GitWorktreeWorkspaceProvider {
    pub(super) provider_ref: ProviderRef,
    pub(super) root: PathBuf,
    journal_path: PathBuf,
    pub(super) records: RefCell<BTreeMap<String, GitRecord>>,
    /// Entries pruned from the journal because their path is gone, kept so a
    /// later release still knows the repository well enough to run
    /// `git worktree prune` (the pre-journal release-after-loss contract).
    pub(super) tombstones: RefCell<BTreeMap<String, GitRecord>>,
}

impl GitWorktreeWorkspaceProvider {
    /// Construct the provider and re-read its durable journal. Allocations
    /// recorded by a previous process are known again; entries whose material
    /// path has disappeared are pruned here and again on observation. A
    /// malformed journal is an honest construction failure, not a
    /// silently-forgotten set of worktrees.
    pub fn new(provider_ref: ProviderRef, root: impl Into<PathBuf>) -> Result<Self> {
        let root = root.into();
        let journal_path = root.join(GIT_WORKTREES_JOURNAL_FILE);
        let mut records = load_journal(&journal_path)?;
        // Prune entries whose material path is gone: the worktree is no longer
        // material, so the journal must not claim it.
        records.retain(|material_ref, record| {
            let present = record.path.exists();
            if !present {
                eprintln!(
                    "git worktree journal: pruning `{material_ref}` — path `{}` is gone",
                    record.path.display()
                );
            }
            present
        });
        write_journal(&journal_path, &records)?;
        Ok(Self {
            provider_ref,
            root,
            journal_path,
            records: RefCell::new(records),
            tombstones: RefCell::new(BTreeMap::new()),
        })
    }

    pub(super) fn persist(&self) -> Result<()> {
        write_journal(&self.journal_path, &self.records.borrow())
    }

    pub(super) fn record(&self, allocation: &ProviderAllocation) -> Result<GitRecord> {
        self.records
            .borrow()
            .get(&allocation.material_ref)
            .cloned()
            .ok_or_else(|| {
                WorkcellError::NotFound(format!(
                    "git worktree `{}` is not known by this provider",
                    allocation.material_ref
                ))
            })
    }

    /// Drop a journal entry whose material path is gone (prune-on-observe).
    /// The record moves to an in-memory tombstone so a later release still
    /// knows the repository; only the durable claim is lifted. Returns true
    /// when the entry was pruned.
    pub(super) fn prune_if_gone(&self, material_ref: &str, record: &GitRecord) -> bool {
        if record.path.exists() {
            return false;
        }
        let mut records = self.records.borrow_mut();
        if records.remove(material_ref).is_some() {
            self.tombstones
                .borrow_mut()
                .insert(material_ref.to_owned(), record.clone());
            drop(records);
            if let Err(error) = self.persist() {
                eprintln!("git worktree journal: prune rewrite failed: {error}");
            }
            return true;
        }
        false
    }

    /// The record for a release: live, or a tombstone left by prune-on-observe.
    pub(super) fn record_for_release(&self, allocation: &ProviderAllocation) -> Result<GitRecord> {
        if let Some(record) = self.records.borrow().get(&allocation.material_ref).cloned() {
            return Ok(record);
        }
        self.tombstones
            .borrow()
            .get(&allocation.material_ref)
            .cloned()
            .ok_or_else(|| {
                WorkcellError::NotFound(format!(
                    "git worktree `{}` is not known by this provider",
                    allocation.material_ref
                ))
            })
    }

    pub(super) fn allocation(&self, material_ref: &str, record: &GitRecord) -> ProviderAllocation {
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
        provenance.insert("provider_kind".into(), "git-worktree".into());
        provenance.insert("source_locator".into(), record.source_locator.clone());
        provenance.insert("source_commit".into(), record.commit.clone());
        provenance.insert("source_dirty".into(), record.source_dirty.to_string());
        if let Some(source_ref) = &record.source_ref {
            provenance.insert("source_ref".into(), source_ref.clone());
        }
        if let Some(branch) = &record.branch {
            provenance.insert("branch".into(), branch.clone());
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
}
