//! Bind existing caller-owned NOW/state directories through the StorageProvider
//! contract. This provider allocates neither semantic NOWs nor host directories.
//! Release detaches the binding: it never erases, moves or chmods source content.
use crate::{material_path::MaterialPath, support::stable_key};
use epilogos_workcell_core::{
    validate_allocation, AttachedStorageRequest, Availability, HealthState, OfferRef,
    OperationalOffer, ProviderAllocation, ProviderObservation, ProviderPort, ProviderPortKind,
    ProviderRef, ProviderReleaseResult, ReleaseDisposition, Result, RetentionExpectation,
    StorageAccess, StorageProvider, StorageSharing, WorkcellError,
};
use serde_json::Value;
use std::{
    collections::BTreeMap,
    fs,
    path::{Path, PathBuf},
};

pub const DIRECTORY_STORAGE_SCHEMA: &str = "workcell.directory-storage/v1";
pub const DIRECTORY_STORAGE_FILE: &str = "storage.json";
pub const DIRECTORY_STORAGE_PROVIDER_REF: &str = "provider:collapsed-local-directory-storage";

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DirectoryStorage {
    pub logical_ref: String,
    pub path: PathBuf,
}

impl DirectoryStorage {
    pub fn new(logical_ref: impl Into<String>, path: impl Into<PathBuf>) -> Result<Self> {
        let value = Self {
            logical_ref: logical_ref.into(),
            path: path.into(),
        };
        if value.logical_ref.trim().is_empty() {
            return Err(WorkcellError::InvalidDemand(
                "storage logical_ref is required".into(),
            ));
        }
        MaterialPath::directory(&value.path)?;
        Ok(value)
    }
}

pub fn read_directory_storage(path: &Path) -> Result<Vec<DirectoryStorage>> {
    let raw = fs::read_to_string(path).map_err(|e| {
        WorkcellError::Unavailable(format!("read directory storage declaration: {e}"))
    })?;
    let value: Value = serde_json::from_str(&raw).map_err(|e| {
        WorkcellError::InvalidDemand(format!("directory storage declaration JSON: {e}"))
    })?;
    let object = value.as_object().ok_or_else(|| {
        WorkcellError::InvalidDemand("directory storage declaration must be an object".into())
    })?;
    if object.keys().any(|k| k != "schema" && k != "directories")
        || value["schema"] != DIRECTORY_STORAGE_SCHEMA
    {
        return Err(WorkcellError::InvalidDemand(
            "unknown directory storage schema or field".into(),
        ));
    }
    let entries = value["directories"].as_array().ok_or_else(|| {
        WorkcellError::InvalidDemand(
            "directory storage declaration requires directories array".into(),
        )
    })?;
    entries.iter().map(|entry| {
        let o = entry.as_object().ok_or_else(|| WorkcellError::InvalidDemand("directory declaration must be an object".into()))?;
        if o.keys().any(|k| k != "logical_ref" && k != "path") { return Err(WorkcellError::InvalidDemand("unknown directory storage field; restrictions are supplied to the execution boundary, not inferred here".into())); }
        DirectoryStorage::new(
            entry["logical_ref"].as_str().ok_or_else(|| WorkcellError::InvalidDemand("logical_ref must be a string".into()))?,
            entry["path"].as_str().ok_or_else(|| WorkcellError::InvalidDemand("path must be a string".into()))?,
        )
    }).collect()
}

pub struct DirectoryStorageProvider {
    provider_ref: ProviderRef,
    directories: BTreeMap<String, MaterialPath>,
}
impl DirectoryStorageProvider {
    pub fn new(
        provider_ref: ProviderRef,
        directories: impl IntoIterator<Item = DirectoryStorage>,
    ) -> Result<Self> {
        let mut by_ref = BTreeMap::new();
        for directory in directories {
            let path = MaterialPath::directory(&directory.path)?;
            if directory.logical_ref.trim().is_empty()
                || by_ref.insert(directory.logical_ref, path).is_some()
            {
                return Err(WorkcellError::InvalidDemand(
                    "empty or duplicate directory storage logical_ref".into(),
                ));
            }
        }
        Ok(Self {
            provider_ref,
            directories: by_ref,
        })
    }
    fn bound_path(&self, allocation: &ProviderAllocation) -> Result<&MaterialPath> {
        validate_allocation(self, allocation)?;
        let path = allocation
            .properties
            .get("logical_ref")
            .and_then(|r| self.directories.get(r))
            .ok_or_else(|| {
                WorkcellError::NotFound(
                    "storage declaration was removed; re-resolution required".into(),
                )
            })?;
        path.validate()?;
        if allocation.properties.get("path") != Some(&path.canonical.display().to_string())
            || allocation.properties.get("object_identity") != Some(&path.identity)
        {
            return Err(WorkcellError::OperationFailed("storage declaration/placement changed; old receipt cannot silently bind the new path".into()));
        }
        Ok(path)
    }
}
impl ProviderPort for DirectoryStorageProvider {
    fn provider_ref(&self) -> &ProviderRef {
        &self.provider_ref
    }
    fn port_kind(&self) -> ProviderPortKind {
        ProviderPortKind::Storage
    }
    fn offers(&self) -> Result<Vec<OperationalOffer>> {
        self.directories
            .iter()
            .map(|(logical_ref, path)| {
                let present = path.validate().is_ok();
                Ok(OperationalOffer {
                    offer_ref: OfferRef::new(format!(
                        "offer:{}:{}",
                        self.provider_ref,
                        stable_key(&[logical_ref])
                    ))?,
                    provider_ref: self.provider_ref.clone(),
                    port: "storage".into(),
                    affordances: vec![
                        "storage:attached".into(),
                        "storage:writable".into(),
                        "storage:shared".into(),
                    ],
                    connections: vec![],
                    exposures: vec![],
                    isolation_trust: vec![],
                    availability: if present {
                        Availability::Available
                    } else {
                        Availability::Unavailable
                    },
                    health: if present {
                        HealthState::Healthy
                    } else {
                        HealthState::Unavailable
                    },
                    capacity: BTreeMap::new(),
                    metadata: BTreeMap::from([
                        ("logical_ref".into(), logical_ref.clone()),
                        ("implementation".into(), "existing-directory".into()),
                        ("lifecycle".into(), "target-owned-detach-only".into()),
                        ("storage:read-only".into(), "unsupported".into()),
                        ("storage:exclusive".into(), "unsupported".into()),
                        (
                            "write-restriction".into(),
                            "none; require a separate execution boundary".into(),
                        ),
                        (
                            "writability".into(),
                            "subject-to-current-host-permissions; not-probed".into(),
                        ),
                    ]),
                })
            })
            .collect()
    }
}
impl StorageProvider for DirectoryStorageProvider {
    fn prepare_storage(&mut self, request: &AttachedStorageRequest) -> Result<ProviderAllocation> {
        request.requirement.validate()?;
        if request.requirement.access != StorageAccess::Writable
            || request.requirement.sharing != StorageSharing::Shared
            || request.requirement.minimum_capacity.is_some()
        {
            return Err(WorkcellError::Unsupported("existing-directory storage supports shared writable attachment only; no read-only enforcement, exclusive lease or capacity reservation".into()));
        }
        if matches!(
            request.retention,
            RetentionExpectation::SnapshotIfSupported | RetentionExpectation::SuspendIfSupported
        ) || matches!(
            request.requirement.retention,
            RetentionExpectation::SnapshotIfSupported | RetentionExpectation::SuspendIfSupported
        ) {
            return Err(WorkcellError::Unsupported(
                "existing-directory storage cannot snapshot or suspend".into(),
            ));
        }
        let logical_ref = &request.requirement.logical_ref;
        let path = self.directories.get(logical_ref).ok_or_else(|| {
            WorkcellError::UnsatisfiedDemand(format!("no directory declared for `{logical_ref}`"))
        })?;
        path.validate()?;
        Ok(ProviderAllocation {
            provider_ref: self.provider_ref.clone(),
            port: ProviderPortKind::Storage,
            material_ref: format!(
                "storage:directory:{}",
                stable_key(&[request.demand_ref.as_str(), logical_ref, &path.identity])
            ),
            health: HealthState::Healthy,
            properties: BTreeMap::from([
                ("logical_ref".into(), logical_ref.clone()),
                ("path".into(), path.canonical.display().to_string()),
                ("object_identity".into(), path.identity.clone()),
                ("access".into(), "writable".into()),
                ("sharing".into(), "shared".into()),
                ("lifetime".into(), "target-owned".into()),
            ]),
            provenance: BTreeMap::from([
                ("implementation".into(), "existing-directory".into()),
                (
                    "release_effect".into(),
                    "detach-only; preserve existing bytes".into(),
                ),
            ]),
        })
    }
    fn observe_storage(&self, allocation: &ProviderAllocation) -> Result<ProviderObservation> {
        // Identity drift is an explicit failure, never a Healthy replacement.
        self.bound_path(allocation)?;
        Ok(ProviderObservation {
            provider_ref: self.provider_ref.clone(),
            material_ref: allocation.material_ref.clone(),
            health: HealthState::Healthy,
            detail: allocation.properties.clone(),
        })
    }
    fn release_storage(
        &mut self,
        allocation: &ProviderAllocation,
        retention: &RetentionExpectation,
    ) -> Result<ProviderReleaseResult> {
        validate_allocation(self, allocation)?;
        let disposition = match retention {
            RetentionExpectation::Release => ReleaseDisposition::Released,
            RetentionExpectation::Preserve => ReleaseDisposition::Preserved,
            _ => {
                return Err(WorkcellError::Unsupported(
                    "existing-directory storage cannot snapshot or suspend".into(),
                ))
            }
        };
        // Detaching a stale/removed target is safe too: no filesystem mutation.
        Ok(ProviderReleaseResult {
            provider_ref: self.provider_ref.clone(),
            material_ref: allocation.material_ref.clone(),
            disposition,
            changed: false,
        })
    }
}
