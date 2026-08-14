use std::{collections::BTreeMap, path::PathBuf};

use epilogos_workcell_core::{
    HealthState, ProviderAllocation, ProviderPortKind, ProviderRef, Result, WorkcellError,
    WorkspaceAccess,
};

#[derive(Clone, Debug)]
pub(super) struct GitRecord {
    pub(super) repository: PathBuf,
    pub(super) path: PathBuf,
    pub(super) commit: String,
    pub(super) access: WorkspaceAccess,
    pub(super) source_ref: Option<String>,
    pub(super) source_locator: String,
    pub(super) source_dirty: bool,
}

pub struct GitWorktreeWorkspaceProvider {
    pub(super) provider_ref: ProviderRef,
    pub(super) root: PathBuf,
    pub(super) records: BTreeMap<String, GitRecord>,
}

impl GitWorktreeWorkspaceProvider {
    pub fn new(provider_ref: ProviderRef, root: impl Into<PathBuf>) -> Self {
        Self {
            provider_ref,
            root: root.into(),
            records: BTreeMap::new(),
        }
    }

    pub(super) fn record(&self, allocation: &ProviderAllocation) -> Result<&GitRecord> {
        self.records.get(&allocation.material_ref).ok_or_else(|| {
            WorkcellError::NotFound(format!(
                "git worktree `{}` is not known by this provider",
                allocation.material_ref
            ))
        })
    }

    pub(super) fn allocation(
        &self,
        material_ref: &str,
        record: &GitRecord,
    ) -> ProviderAllocation {
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
