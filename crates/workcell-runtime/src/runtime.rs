use std::collections::BTreeMap;

use epilogos_workcell_core::{
    Availability, HealthState, MaterialExposureProvider, OfferRef, OperationalOffer,
    ProjectRuntimeMaterialRequest, ProjectRuntimeProvider, ProviderAllocation, ProviderExposedSurface,
    ProviderExposureRequest, ProviderObservation, ProviderPort, ProviderPortKind, ProviderRef,
    ProviderReleaseResult, ReleaseDisposition, Result, RetentionExpectation, WorkcellError,
};

use crate::support::stable_key;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RuntimeMode {
    pub name: String,
    pub connections: Vec<String>,
    pub exposures: Vec<String>,
    pub endpoint: Option<String>,
    pub metadata: BTreeMap<String, String>,
}

impl RuntimeMode {
    pub fn new(name: impl Into<String>) -> Result<Self> {
        let name = name.into();
        if name.trim().is_empty() {
            return Err(WorkcellError::InvalidDemand(
                "runtime mode name must not be empty".into(),
            ));
        }
        Ok(Self {
            name,
            connections: Vec::new(),
            exposures: Vec::new(),
            endpoint: None,
            metadata: BTreeMap::new(),
        })
    }

    pub fn with_connection(mut self, connection: impl Into<String>) -> Self {
        self.connections.push(connection.into());
        self
    }

    pub fn with_exposure(mut self, exposure: impl Into<String>) -> Self {
        self.exposures.push(exposure.into());
        self
    }

    pub fn with_endpoint(mut self, endpoint: impl Into<String>) -> Self {
        self.endpoint = Some(endpoint.into());
        self
    }
}

#[derive(Clone, Debug)]
struct RuntimeRecord {
    mode: RuntimeMode,
}

pub struct ReferenceProjectRuntimeProvider {
    provider_ref: ProviderRef,
    modes: BTreeMap<String, RuntimeMode>,
    records: BTreeMap<String, RuntimeRecord>,
}

impl ReferenceProjectRuntimeProvider {
    pub fn new(
        provider_ref: ProviderRef,
        modes: impl IntoIterator<Item = RuntimeMode>,
    ) -> Result<Self> {
        let mut by_name = BTreeMap::new();
        for mode in modes {
            if mode.name.trim().is_empty() {
                return Err(WorkcellError::InvalidDemand(
                    "runtime mode name must not be empty".into(),
                ));
            }
            if by_name.insert(mode.name.clone(), mode).is_some() {
                return Err(WorkcellError::InvalidDemand(
                    "runtime provider contains duplicate mode names".into(),
                ));
            }
        }
        Ok(Self {
            provider_ref,
            modes: by_name,
            records: BTreeMap::new(),
        })
    }

    fn record(&self, allocation: &ProviderAllocation) -> Result<&RuntimeRecord> {
        self.records.get(&allocation.material_ref).ok_or_else(|| {
            WorkcellError::NotFound(format!(
                "project runtime `{}` is not known by this provider",
                allocation.material_ref
            ))
        })
    }
}

impl ProviderPort for ReferenceProjectRuntimeProvider {
    fn provider_ref(&self) -> &ProviderRef {
        &self.provider_ref
    }

    fn port_kind(&self) -> ProviderPortKind {
        ProviderPortKind::ProjectRuntime
    }

    fn offers(&self) -> Result<Vec<OperationalOffer>> {
        let mut offers = Vec::with_capacity(self.modes.len());
        for mode in self.modes.values() {
            let mut metadata = mode.metadata.clone();
            metadata.insert("runtime_mode".into(), mode.name.clone());
            metadata.insert("implementation".into(), "reference-runtime".into());
            offers.push(OperationalOffer {
                offer_ref: OfferRef::new(format!(
                    "offer:{}:runtime:{}",
                    self.provider_ref, mode.name
                ))
                .map_err(|error| WorkcellError::OperationFailed(error.into()))?,
                provider_ref: self.provider_ref.clone(),
                port: ProviderPortKind::ProjectRuntime.as_str().into(),
                affordances: vec![format!("runtime-mode:{}", mode.name)],
                connections: mode.connections.clone(),
                exposures: mode.exposures.clone(),
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

impl ProjectRuntimeProvider for ReferenceProjectRuntimeProvider {
    fn prepare_runtime(
        &mut self,
        request: &ProjectRuntimeMaterialRequest,
    ) -> Result<ProviderAllocation> {
        let mode = self.modes.get(&request.mode).cloned().ok_or_else(|| {
            WorkcellError::UnsatisfiedDemand(format!(
                "project runtime mode `{}` is not offered",
                request.mode
            ))
        })?;
        for connection in &request.connectivity {
            if !mode
                .connections
                .iter()
                .any(|available| available == connection.as_str())
            {
                return Err(WorkcellError::UnsatisfiedDemand(format!(
                    "runtime mode `{}` does not provide connection `{}`",
                    request.mode,
                    connection.as_str()
                )));
            }
        }

        let key = stable_key(&[request.demand_ref.as_str(), &request.mode]);
        let material_ref = format!("runtime:reference:{key}");
        let mut properties = BTreeMap::new();
        properties.insert("mode".into(), mode.name.clone());
        if let Some(endpoint) = &mode.endpoint {
            properties.insert("endpoint".into(), endpoint.clone());
        }
        if !mode.connections.is_empty() {
            properties.insert("connections".into(), mode.connections.join(","));
        }
        if !mode.exposures.is_empty() {
            properties.insert("exposures".into(), mode.exposures.join(","));
        }
        let mut provenance = BTreeMap::new();
        provenance.insert("implementation".into(), "reference-runtime".into());
        provenance.insert("runtime_mode".into(), mode.name.clone());

        self.records
            .insert(material_ref.clone(), RuntimeRecord { mode: mode.clone() });
        Ok(ProviderAllocation {
            provider_ref: self.provider_ref.clone(),
            port: ProviderPortKind::ProjectRuntime,
            material_ref,
            health: HealthState::Healthy,
            properties,
            provenance,
        })
    }

    fn observe_runtime(&self, allocation: &ProviderAllocation) -> Result<ProviderObservation> {
        let record = self.record(allocation)?;
        let mut detail = BTreeMap::new();
        detail.insert("running".into(), "true".into());
        detail.insert("mode".into(), record.mode.name.clone());
        if let Some(endpoint) = &record.mode.endpoint {
            detail.insert("endpoint".into(), endpoint.clone());
        }
        Ok(ProviderObservation {
            provider_ref: self.provider_ref.clone(),
            material_ref: allocation.material_ref.clone(),
            health: HealthState::Healthy,
            detail,
        })
    }

    fn release_runtime(
        &mut self,
        allocation: &ProviderAllocation,
        retention: &RetentionExpectation,
    ) -> Result<ProviderReleaseResult> {
        self.record(allocation)?;
        match retention {
            RetentionExpectation::Preserve => Ok(ProviderReleaseResult {
                provider_ref: self.provider_ref.clone(),
                material_ref: allocation.material_ref.clone(),
                disposition: ReleaseDisposition::Preserved,
                changed: false,
            }),
            RetentionExpectation::Release => {
                self.records.remove(&allocation.material_ref);
                Ok(ProviderReleaseResult {
                    provider_ref: self.provider_ref.clone(),
                    material_ref: allocation.material_ref.clone(),
                    disposition: ReleaseDisposition::Released,
                    changed: true,
                })
            }
            RetentionExpectation::SuspendIfSupported
            | RetentionExpectation::SnapshotIfSupported => Err(WorkcellError::Unsupported(
                "reference project runtime does not support suspend/snapshot".into(),
            )),
        }
    }
}

impl MaterialExposureProvider for ReferenceProjectRuntimeProvider {
    fn expose_material(
        &self,
        allocation: &ProviderAllocation,
        request: &ProviderExposureRequest,
    ) -> Result<ProviderExposedSurface> {
        let record = self.record(allocation)?;
        let requirement = request.requirement.as_str();
        if !record
            .mode
            .exposures
            .iter()
            .any(|exposure| exposure == requirement)
        {
            return Err(WorkcellError::UnsatisfiedDemand(format!(
                "runtime mode `{}` does not expose `{requirement}`",
                record.mode.name
            )));
        }
        let endpoint = record.mode.endpoint.as_ref().ok_or_else(|| {
            WorkcellError::Unavailable(format!(
                "runtime mode `{}` has no material endpoint for `{requirement}`",
                record.mode.name
            ))
        })?;

        let mut material = BTreeMap::new();
        material.insert("endpoint".into(), endpoint.clone());
        material.insert("runtime_mode".into(), record.mode.name.clone());
        let mut provenance = allocation.provenance.clone();
        provenance.insert("exposure".into(), requirement.into());
        provenance.insert("provider_ref".into(), self.provider_ref.to_string());

        Ok(ProviderExposedSurface {
            provider_ref: self.provider_ref.clone(),
            material_ref: allocation.material_ref.clone(),
            logical_ref: format!("exposure:{requirement}"),
            interaction: requirement.into(),
            material,
            provenance,
        })
    }
}
