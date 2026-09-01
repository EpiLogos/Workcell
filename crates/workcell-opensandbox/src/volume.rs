use std::collections::{BTreeMap, BTreeSet};

use epilogos_workcell_sdk::{
    contract::{RetentionExpectation, StorageAccess, StorageRequirement, StorageSharing},
    provider::{
        AttachedStorageRequest, Availability, Capacity, HealthState, OfferRef, OperationalOffer,
        ProviderAllocation, ProviderObservation, ProviderPort, ProviderPortKind, ProviderRef,
        ProviderReleaseResult, ReleaseDisposition, Result, StorageProvider, WorkcellError,
    },
};
use serde_json::{json, Value};

use super::OPENSANDBOX_SOURCE_REVISION;

/// Provider-local reference to an existing OpenSandbox-compatible `pvc`
/// backend. OpenSandbox does not provision or destroy this storage; Docker maps
/// it to a named volume and Kubernetes maps it to a PersistentVolumeClaim.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct OpenSandboxPvcVolume {
    pub logical_ref: String,
    pub claim_name: String,
    pub capacity: Option<Capacity>,
    pub writable: bool,
    pub shared: bool,
}

impl OpenSandboxPvcVolume {
    pub fn validate(&self) -> Result<()> {
        if self.logical_ref.trim().is_empty() || self.claim_name.trim().is_empty() {
            return Err(WorkcellError::InvalidDemand(
                "OpenSandbox named-volume logical_ref/claim_name must not be empty".into(),
            ));
        }
        if self.claim_name.len() > 63
            || self.claim_name.starts_with('-')
            || self.claim_name.ends_with('-')
            || !self
                .claim_name
                .bytes()
                .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-')
        {
            return Err(WorkcellError::InvalidDemand(format!(
                "OpenSandbox pvc claim `{}` must be a DNS-label-compatible named volume",
                self.claim_name
            )));
        }
        if let Some(capacity) = &self.capacity {
            if capacity.amount == 0
                || capacity
                    .unit
                    .as_deref()
                    .is_some_and(|unit| unit.trim().is_empty())
            {
                return Err(WorkcellError::InvalidDemand(
                    "OpenSandbox named-volume capacity must be positive with a non-empty unit"
                        .into(),
                ));
            }
        }
        Ok(())
    }
}

/// Storage façade over pre-existing provider-native named volumes.
///
/// `prepare_storage` acquires a Workcell material binding; it does not create
/// the underlying Docker volume/PVC. Releasing the binding therefore never
/// claims to destroy the storage backend.
pub struct OpenSandboxPvcStorageProvider {
    provider_ref: ProviderRef,
    volumes: Vec<OpenSandboxPvcVolume>,
    active_bindings: BTreeMap<String, usize>,
}

impl OpenSandboxPvcStorageProvider {
    pub fn new(provider_ref: ProviderRef, volumes: Vec<OpenSandboxPvcVolume>) -> Result<Self> {
        if volumes.is_empty() {
            return Err(WorkcellError::InvalidDemand(
                "OpenSandbox pvc storage provider requires at least one existing volume".into(),
            ));
        }
        let mut logical_refs = BTreeSet::new();
        let mut claim_names = BTreeSet::new();
        for volume in &volumes {
            volume.validate()?;
            if !logical_refs.insert(volume.logical_ref.clone()) {
                return Err(WorkcellError::InvalidDemand(format!(
                    "OpenSandbox storage logical_ref `{}` appears more than once",
                    volume.logical_ref
                )));
            }
            if !claim_names.insert(volume.claim_name.clone()) {
                return Err(WorkcellError::InvalidDemand(format!(
                    "OpenSandbox storage claim `{}` appears more than once",
                    volume.claim_name
                )));
            }
        }
        Ok(Self {
            provider_ref,
            volumes,
            active_bindings: BTreeMap::new(),
        })
    }

    fn compatible<'a>(&'a self, requirement: &StorageRequirement) -> Option<&'a OpenSandboxPvcVolume> {
        self.volumes.iter().find(|volume| {
            volume.logical_ref == requirement.logical_ref
                && (requirement.access == StorageAccess::ReadOnly || volume.writable)
                && (requirement.sharing == StorageSharing::Exclusive || volume.shared)
                && capacity_satisfies(volume.capacity.as_ref(), requirement)
                && (requirement.sharing != StorageSharing::Exclusive
                    || self
                        .active_bindings
                        .get(&volume.claim_name)
                        .copied()
                        .unwrap_or_default()
                        == 0)
        })
    }
}

impl ProviderPort for OpenSandboxPvcStorageProvider {
    fn provider_ref(&self) -> &ProviderRef {
        &self.provider_ref
    }

    fn port_kind(&self) -> ProviderPortKind {
        ProviderPortKind::Storage
    }

    fn offers(&self) -> Result<Vec<OperationalOffer>> {
        self.volumes
            .iter()
            .map(|volume| {
                let mut capacity = BTreeMap::new();
                if let Some(value) = &volume.capacity {
                    capacity.insert("storage".into(), value.clone());
                }
                Ok(OperationalOffer {
                    offer_ref: OfferRef::new(format!(
                        "offer:{}:storage:{}",
                        self.provider_ref, volume.logical_ref
                    ))?,
                    provider_ref: self.provider_ref.clone(),
                    port: ProviderPortKind::Storage.as_str().into(),
                    affordances: vec![
                        format!(
                            "storage:access:{}",
                            if volume.writable { "writable" } else { "read-only" }
                        ),
                        format!(
                            "storage:sharing:{}",
                            if volume.shared { "shared" } else { "exclusive" }
                        ),
                    ],
                    connections: Vec::new(),
                    exposures: Vec::new(),
                    isolation_trust: Vec::new(),
                    availability: Availability::Available,
                    health: HealthState::Healthy,
                    capacity,
                    metadata: BTreeMap::from([
                        ("logical_ref".into(), volume.logical_ref.clone()),
                        ("backend".into(), "opensandbox:pvc".into()),
                        ("lifecycle".into(), "externally-managed".into()),
                    ]),
                })
            })
            .collect()
    }
}

impl StorageProvider for OpenSandboxPvcStorageProvider {
    fn prepare_storage(&mut self, request: &AttachedStorageRequest) -> Result<ProviderAllocation> {
        request.requirement.validate()?;
        let volume = self.compatible(&request.requirement).cloned().ok_or_else(|| {
            WorkcellError::UnsatisfiedDemand(format!(
                "no OpenSandbox named volume satisfies `{}`",
                request.requirement.logical_ref
            ))
        })?;
        *self
            .active_bindings
            .entry(volume.claim_name.clone())
            .or_default() += 1;
        Ok(ProviderAllocation {
            provider_ref: self.provider_ref.clone(),
            port: ProviderPortKind::Storage,
            material_ref: volume.claim_name.clone(),
            health: HealthState::Healthy,
            properties: BTreeMap::from([
                ("logical_ref".into(), volume.logical_ref.clone()),
                (
                    "access".into(),
                    match request.requirement.access {
                        StorageAccess::ReadOnly => "read-only",
                        StorageAccess::Writable => "writable",
                    }
                    .into(),
                ),
                (
                    "sharing".into(),
                    match request.requirement.sharing {
                        StorageSharing::Exclusive => "exclusive",
                        StorageSharing::Shared => "shared",
                    }
                    .into(),
                ),
            ]),
            provenance: BTreeMap::from([
                ("provider".into(), "opensandbox:pvc-storage".into()),
                ("upstream.revision".into(), OPENSANDBOX_SOURCE_REVISION.into()),
                ("opensandbox.volume_backend".into(), "pvc".into()),
                ("storage.lifecycle".into(), "externally-managed".into()),
            ]),
        })
    }

    fn observe_storage(&self, allocation: &ProviderAllocation) -> Result<ProviderObservation> {
        require_storage_allocation(&self.provider_ref, allocation)?;
        if self
            .active_bindings
            .get(&allocation.material_ref)
            .copied()
            .unwrap_or_default()
            == 0
        {
            return Err(WorkcellError::NotFound(format!(
                "OpenSandbox storage binding `{}` is not active",
                allocation.material_ref
            )));
        }
        Ok(ProviderObservation {
            provider_ref: self.provider_ref.clone(),
            material_ref: allocation.material_ref.clone(),
            health: HealthState::Healthy,
            detail: BTreeMap::from([("storage.lifecycle".into(), "externally-managed".into())]),
        })
    }

    fn release_storage(
        &mut self,
        allocation: &ProviderAllocation,
        _retention: &RetentionExpectation,
    ) -> Result<ProviderReleaseResult> {
        require_storage_allocation(&self.provider_ref, allocation)?;
        let count = self.active_bindings.get_mut(&allocation.material_ref).ok_or_else(|| {
            WorkcellError::NotFound(format!(
                "OpenSandbox storage binding `{}` is not active",
                allocation.material_ref
            ))
        })?;
        *count = count.saturating_sub(1);
        if *count == 0 {
            self.active_bindings.remove(&allocation.material_ref);
        }
        Ok(ProviderReleaseResult {
            provider_ref: self.provider_ref.clone(),
            material_ref: allocation.material_ref.clone(),
            disposition: ReleaseDisposition::Released,
            changed: true,
        })
    }
}

/// Provider-native mount of one already-selected Storage allocation into an
/// OpenSandbox sandbox. The Storage provider/material identity is retained here
/// for Workcell provenance but only the provider-native claim name is sent on
/// OpenSandbox's lifecycle wire.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct OpenSandboxVolumeMount {
    pub name: String,
    pub storage_provider_ref: ProviderRef,
    pub storage_material_ref: String,
    pub mount_path: String,
    pub read_only: bool,
    pub sub_path: Option<String>,
}

impl OpenSandboxVolumeMount {
    pub fn pvc_from_storage_allocation(
        name: impl Into<String>,
        allocation: &ProviderAllocation,
        mount_path: impl Into<String>,
        read_only: bool,
        sub_path: Option<String>,
    ) -> Result<Self> {
        if allocation.port != ProviderPortKind::Storage
            || allocation
                .provenance
                .get("opensandbox.volume_backend")
                .map(String::as_str)
                != Some("pvc")
        {
            return Err(WorkcellError::InvalidDemand(
                "OpenSandbox pvc mount requires a Storage allocation explicitly compatible with the pvc backend"
                    .into(),
            ));
        }
        let mount = Self {
            name: name.into(),
            storage_provider_ref: allocation.provider_ref.clone(),
            storage_material_ref: allocation.material_ref.clone(),
            mount_path: mount_path.into(),
            read_only,
            sub_path,
        };
        mount.validate()?;
        Ok(mount)
    }

    pub fn validate(&self) -> Result<()> {
        if self.name.trim().is_empty()
            || self.storage_material_ref.trim().is_empty()
            || !self.mount_path.starts_with('/')
            || self.mount_path == "/"
        {
            return Err(WorkcellError::InvalidDemand(
                "OpenSandbox volume mount requires a name, material ref and non-root absolute mount path"
                    .into(),
            ));
        }
        if self
            .sub_path
            .as_deref()
            .is_some_and(|value| value.trim().is_empty() || value.starts_with('/') || value.contains(".."))
        {
            return Err(WorkcellError::InvalidDemand(
                "OpenSandbox volume subPath must be safe and relative".into(),
            ));
        }
        Ok(())
    }

    pub(crate) fn wire_value(&self) -> Result<Value> {
        self.validate()?;
        let mut value = json!({
            "name": self.name,
            "pvc": {"claimName": self.storage_material_ref},
            "mountPath": self.mount_path,
            "readOnly": self.read_only,
        });
        if let Some(sub_path) = &self.sub_path {
            value
                .as_object_mut()
                .expect("volume wire object")
                .insert("subPath".into(), Value::String(sub_path.clone()));
        }
        Ok(value)
    }
}

fn capacity_satisfies(capacity: Option<&Capacity>, requirement: &StorageRequirement) -> bool {
    match requirement.minimum_capacity {
        None => true,
        Some(minimum) => capacity.is_some_and(|capacity| {
            capacity.amount >= minimum && capacity.unit.as_deref() == requirement.unit.as_deref()
        }),
    }
}

fn require_storage_allocation(
    provider_ref: &ProviderRef,
    allocation: &ProviderAllocation,
) -> Result<()> {
    if allocation.provider_ref != *provider_ref
        || allocation.port != ProviderPortKind::Storage
        || allocation.material_ref.trim().is_empty()
    {
        return Err(WorkcellError::OperationFailed(
            "OpenSandbox storage allocation escaped its provider/material identity".into(),
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use epilogos_workcell_sdk::contract::{DemandRef, PersistenceScope};

    use super::*;

    fn provider() -> OpenSandboxPvcStorageProvider {
        OpenSandboxPvcStorageProvider::new(
            ProviderRef::new("provider:storage").unwrap(),
            vec![OpenSandboxPvcVolume {
                logical_ref: "state:project".into(),
                claim_name: "project-state".into(),
                capacity: Some(Capacity {
                    amount: 20,
                    unit: Some("GiB".into()),
                }),
                writable: true,
                shared: true,
            }],
        )
        .unwrap()
    }

    fn request(sharing: StorageSharing) -> AttachedStorageRequest {
        AttachedStorageRequest {
            demand_ref: DemandRef::new("demand:storage").unwrap(),
            requirement: StorageRequirement {
                logical_ref: "state:project".into(),
                access: StorageAccess::Writable,
                sharing,
                minimum_capacity: Some(4),
                unit: Some("GiB".into()),
                persistence: Some(PersistenceScope::Project),
                retention: RetentionExpectation::Preserve,
            },
            persistence: Some(PersistenceScope::Project),
            retention: RetentionExpectation::Preserve,
        }
    }

    #[test]
    fn storage_binding_is_independent_of_underlying_volume_lifecycle() {
        let mut provider = provider();
        let allocation = provider.prepare_storage(&request(StorageSharing::Shared)).unwrap();
        assert_eq!(allocation.port, ProviderPortKind::Storage);
        assert_eq!(allocation.material_ref, "project-state");
        assert_eq!(
            allocation.provenance["storage.lifecycle"],
            "externally-managed"
        );
        provider.observe_storage(&allocation).unwrap();
        let released = provider
            .release_storage(&allocation, &RetentionExpectation::Release)
            .unwrap();
        assert_eq!(released.disposition, ReleaseDisposition::Released);
        assert!(provider.observe_storage(&allocation).is_err());
    }

    #[test]
    fn mount_consumes_storage_allocation_without_collapsing_storage_identity() {
        let mut provider = provider();
        let allocation = provider.prepare_storage(&request(StorageSharing::Shared)).unwrap();
        let mount = OpenSandboxVolumeMount::pvc_from_storage_allocation(
            "project-state",
            &allocation,
            "/workspace/state",
            false,
            Some("candidate-42".into()),
        )
        .unwrap();
        let wire = mount.wire_value().unwrap();
        assert_eq!(wire["pvc"]["claimName"], "project-state");
        assert_eq!(wire["mountPath"], "/workspace/state");
        assert_eq!(mount.storage_provider_ref, allocation.provider_ref);
        assert_eq!(mount.storage_material_ref, allocation.material_ref);
    }

    #[test]
    fn exclusive_storage_cannot_be_bound_twice() {
        let mut provider = provider();
        provider
            .prepare_storage(&request(StorageSharing::Exclusive))
            .unwrap();
        assert!(provider
            .prepare_storage(&request(StorageSharing::Exclusive))
            .is_err());
    }
}
