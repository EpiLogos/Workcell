use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    path::{Path, PathBuf},
};

use epilogos_workcell_core::{
    validate_allocation, ArtifactChannelRequest, ArtifactStorageProvider, Availability, HealthState,
    OfferRef, OperationalOffer, ProviderAllocation, ProviderCollectedMaterial, ProviderObservation,
    ProviderPort, ProviderPortKind, ProviderRef, ProviderReleaseResult, ReleaseDisposition, Result,
    RetentionExpectation, WorkcellError,
};

use crate::support::stable_key;

#[derive(Clone, Debug)]
struct ChannelRecord {
    path: PathBuf,
    logical_channel: String,
}

pub struct DirectoryArtifactStorageProvider {
    provider_ref: ProviderRef,
    root: PathBuf,
    channels: BTreeSet<String>,
    records: BTreeMap<String, ChannelRecord>,
}

impl DirectoryArtifactStorageProvider {
    pub fn new(
        provider_ref: ProviderRef,
        root: impl Into<PathBuf>,
        channels: impl IntoIterator<Item = String>,
    ) -> Result<Self> {
        let mut supported = BTreeSet::new();
        for channel in channels {
            if channel.trim().is_empty() {
                return Err(WorkcellError::InvalidDemand(
                    "artifact channel name must not be empty".into(),
                ));
            }
            if !supported.insert(channel) {
                return Err(WorkcellError::InvalidDemand(
                    "artifact storage provider contains duplicate channel names".into(),
                ));
            }
        }
        Ok(Self {
            provider_ref,
            root: root.into(),
            channels: supported,
            records: BTreeMap::new(),
        })
    }

    fn record(&self, allocation: &ProviderAllocation) -> Result<ChannelRecord> {
        validate_allocation(self, allocation)?;
        if let Some(record) = self.records.get(&allocation.material_ref) {
            return Ok(record.clone());
        }
        let path = allocation.properties.get("path").ok_or_else(|| {
            WorkcellError::OperationFailed(format!(
                "persisted artifact channel `{}` has no path property",
                allocation.material_ref
            ))
        })?;
        let logical_channel = allocation
            .properties
            .get("logical_channel")
            .ok_or_else(|| {
                WorkcellError::OperationFailed(format!(
                    "persisted artifact channel `{}` has no logical_channel property",
                    allocation.material_ref
                ))
            })?;
        Ok(ChannelRecord {
            path: PathBuf::from(path),
            logical_channel: logical_channel.clone(),
        })
    }
}

impl ProviderPort for DirectoryArtifactStorageProvider {
    fn provider_ref(&self) -> &ProviderRef {
        &self.provider_ref
    }

    fn port_kind(&self) -> ProviderPortKind {
        ProviderPortKind::ArtifactStorage
    }

    fn offers(&self) -> Result<Vec<OperationalOffer>> {
        let mut offers = Vec::with_capacity(self.channels.len());
        for channel in &self.channels {
            let key = stable_key(&[channel]);
            let mut metadata = BTreeMap::new();
            metadata.insert("implementation".into(), "directory-artifact-storage".into());
            metadata.insert("logical_channel".into(), channel.clone());
            offers.push(OperationalOffer {
                offer_ref: OfferRef::new(format!("offer:{}:artifact:{key}", self.provider_ref))
                    .map_err(|error| WorkcellError::OperationFailed(error.into()))?,
                provider_ref: self.provider_ref.clone(),
                port: ProviderPortKind::ArtifactStorage.as_str().into(),
                affordances: vec![format!("artifact-channel:{channel}")],
                connections: vec![],
                exposures: vec![],
                isolation_trust: vec![],
                availability: Availability::Available,
                health: HealthState::Healthy,
                capacity: BTreeMap::new(),
                metadata,
            });
        }
        Ok(offers)
    }
}

impl ArtifactStorageProvider for DirectoryArtifactStorageProvider {
    fn prepare_artifact_channel(
        &mut self,
        request: &ArtifactChannelRequest,
    ) -> Result<ProviderAllocation> {
        if !self.channels.contains(&request.logical_channel) {
            return Err(WorkcellError::UnsatisfiedDemand(format!(
                "artifact channel `{}` is not offered",
                request.logical_channel
            )));
        }
        fs::create_dir_all(&self.root).map_err(|error| {
            WorkcellError::OperationFailed(format!("create artifact storage root: {error}"))
        })?;
        let key = stable_key(&[request.demand_ref.as_str(), &request.logical_channel]);
        let path = self.root.join(&key);
        fs::create_dir_all(&path).map_err(|error| {
            WorkcellError::OperationFailed(format!("create artifact channel: {error}"))
        })?;
        let material_ref = format!("artifact-channel:directory:{key}");
        let record = ChannelRecord {
            path: path.clone(),
            logical_channel: request.logical_channel.clone(),
        };
        self.records.insert(material_ref.clone(), record);

        let mut properties = BTreeMap::new();
        properties.insert("path".into(), path.display().to_string());
        properties.insert("logical_channel".into(), request.logical_channel.clone());
        let mut provenance = BTreeMap::new();
        provenance.insert("implementation".into(), "directory-artifact-storage".into());
        provenance.insert("logical_channel".into(), request.logical_channel.clone());

        Ok(ProviderAllocation {
            provider_ref: self.provider_ref.clone(),
            port: ProviderPortKind::ArtifactStorage,
            material_ref,
            health: HealthState::Healthy,
            properties,
            provenance,
        })
    }

    fn collect_material(
        &self,
        allocation: &ProviderAllocation,
    ) -> Result<Vec<ProviderCollectedMaterial>> {
        let record = self.record(allocation)?;
        if !record.path.exists() {
            return Err(WorkcellError::Unavailable(format!(
                "artifact channel `{}` no longer exists",
                allocation.material_ref
            )));
        }
        let mut files = Vec::new();
        collect_files(&record.path, &record.path, &mut files)?;
        files.sort();
        Ok(files
            .into_iter()
            .map(|path| {
                let relative = path
                    .strip_prefix(&record.path)
                    .expect("collected path remains inside channel")
                    .to_string_lossy()
                    .replace('\\', "/");
                let mut provenance = allocation.provenance.clone();
                provenance.insert("relative_path".into(), relative.clone());
                ProviderCollectedMaterial {
                    provider_ref: self.provider_ref.clone(),
                    material_ref: allocation.material_ref.clone(),
                    logical_output: format!("{}/{}", record.logical_channel, relative),
                    locator: path.display().to_string(),
                    provenance,
                }
            })
            .collect())
    }

    fn observe_artifact_channel(
        &self,
        allocation: &ProviderAllocation,
    ) -> Result<ProviderObservation> {
        let record = self.record(allocation)?;
        let mut detail = BTreeMap::new();
        detail.insert("path".into(), record.path.display().to_string());
        detail.insert("logical_channel".into(), record.logical_channel.clone());
        if !record.path.exists() {
            detail.insert("exists".into(), "false".into());
            return Ok(ProviderObservation {
                provider_ref: self.provider_ref.clone(),
                material_ref: allocation.material_ref.clone(),
                health: HealthState::Unavailable,
                detail,
            });
        }
        detail.insert("exists".into(), "true".into());
        Ok(ProviderObservation {
            provider_ref: self.provider_ref.clone(),
            material_ref: allocation.material_ref.clone(),
            health: HealthState::Healthy,
            detail,
        })
    }

    fn release_artifact_channel(
        &mut self,
        allocation: &ProviderAllocation,
        retention: &RetentionExpectation,
    ) -> Result<ProviderReleaseResult> {
        let record = self.record(allocation)?;
        match retention {
            RetentionExpectation::Preserve => Ok(ProviderReleaseResult {
                provider_ref: self.provider_ref.clone(),
                material_ref: allocation.material_ref.clone(),
                disposition: ReleaseDisposition::Preserved,
                changed: false,
            }),
            RetentionExpectation::Release => {
                let changed = if record.path.exists() {
                    fs::remove_dir_all(&record.path).map_err(|error| {
                        WorkcellError::CleanupFailed(format!(
                            "remove artifact channel `{}`: {error}",
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
            RetentionExpectation::SuspendIfSupported
            | RetentionExpectation::SnapshotIfSupported => Err(WorkcellError::Unsupported(
                "directory artifact storage does not support suspend/snapshot".into(),
            )),
        }
    }
}

fn collect_files(root: &Path, current: &Path, output: &mut Vec<PathBuf>) -> Result<()> {
    let mut entries = fs::read_dir(current)
        .map_err(|error| WorkcellError::OperationFailed(format!("read artifact channel: {error}")))?
        .collect::<std::io::Result<Vec<_>>>()
        .map_err(|error| {
            WorkcellError::OperationFailed(format!("read artifact channel entry: {error}"))
        })?;
    entries.sort_by_key(|entry| entry.file_name());
    for entry in entries {
        let path = entry.path();
        let file_type = entry.file_type().map_err(|error| {
            WorkcellError::OperationFailed(format!("inspect artifact output: {error}"))
        })?;
        if file_type.is_symlink() {
            return Err(WorkcellError::Unsupported(format!(
                "artifact output contains unsupported symlink `{}`",
                path.display()
            )));
        }
        if file_type.is_dir() {
            collect_files(root, &path, output)?;
        } else if file_type.is_file() {
            let _ = path.strip_prefix(root).map_err(|error| {
                WorkcellError::OperationFailed(format!("artifact output escaped channel: {error}"))
            })?;
            output.push(path);
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use epilogos_workcell_core::DemandRef;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn temp_path(label: &str) -> PathBuf {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        std::env::temp_dir().join(format!(
            "epilogos-workcell-{label}-{}-{nonce}",
            std::process::id()
        ))
    }

    #[test]
    fn persisted_allocation_can_be_collected_and_released_after_provider_restart() {
        let root = temp_path("artifact-restart");
        let provider_ref = ProviderRef::new("provider:artifact-restart").unwrap();
        let request = ArtifactChannelRequest {
            demand_ref: DemandRef::new("demand:artifact-restart").unwrap(),
            logical_channel: "logs:run".into(),
            persistence: None,
            retention: RetentionExpectation::Release,
        };
        let mut first = DirectoryArtifactStorageProvider::new(
            provider_ref.clone(),
            &root,
            ["logs:run".into()],
        )
        .unwrap();
        let allocation = first.prepare_artifact_channel(&request).unwrap();
        let path = PathBuf::from(allocation.properties.get("path").unwrap());
        fs::write(path.join("restart.log"), "persisted\n").unwrap();
        drop(first);

        let mut restarted = DirectoryArtifactStorageProvider::new(
            provider_ref,
            &root,
            ["logs:run".into()],
        )
        .unwrap();
        let collected = restarted.collect_material(&allocation).unwrap();
        assert_eq!(collected.len(), 1);
        assert!(collected[0].locator.ends_with("restart.log"));
        restarted
            .release_artifact_channel(&allocation, &RetentionExpectation::Release)
            .unwrap();
        assert!(!path.exists());
        let _ = fs::remove_dir_all(root);
    }
}
