use std::collections::BTreeMap;

use epilogos_workcell_core::{
    Availability, HealthState, OfferRef, OperationalOffer, ProviderAllocation, ProviderObservation,
    ProviderPort, ProviderPortKind, ProviderRef, ProviderReleaseResult, ReleaseDisposition, Result,
    RetentionExpectation, ServiceMaterialRequest, ServiceProvider, WorkcellError,
};

use crate::support::stable_key;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct StaticService {
    pub logical_ref: String,
    pub endpoint: String,
    pub health: HealthState,
    pub metadata: BTreeMap<String, String>,
}

impl StaticService {
    pub fn new(logical_ref: impl Into<String>, endpoint: impl Into<String>) -> Result<Self> {
        let logical_ref = logical_ref.into();
        let endpoint = endpoint.into();
        if logical_ref.trim().is_empty() {
            return Err(WorkcellError::InvalidDemand(
                "static service logical ref must not be empty".into(),
            ));
        }
        if endpoint.trim().is_empty() {
            return Err(WorkcellError::InvalidDemand(
                "static service endpoint must not be empty".into(),
            ));
        }
        Ok(Self {
            logical_ref,
            endpoint,
            health: HealthState::Healthy,
            metadata: BTreeMap::new(),
        })
    }

    pub fn with_health(mut self, health: HealthState) -> Self {
        self.health = health;
        self
    }
}

pub struct StaticServiceProvider {
    provider_ref: ProviderRef,
    services: BTreeMap<String, StaticService>,
    records: BTreeMap<String, StaticService>,
}

impl StaticServiceProvider {
    pub fn new(
        provider_ref: ProviderRef,
        services: impl IntoIterator<Item = StaticService>,
    ) -> Result<Self> {
        let mut by_ref = BTreeMap::new();
        for service in services {
            if by_ref
                .insert(service.logical_ref.clone(), service)
                .is_some()
            {
                return Err(WorkcellError::InvalidDemand(
                    "static service provider contains duplicate logical refs".into(),
                ));
            }
        }
        Ok(Self {
            provider_ref,
            services: by_ref,
            records: BTreeMap::new(),
        })
    }

    fn record(&self, allocation: &ProviderAllocation) -> Result<&StaticService> {
        self.records.get(&allocation.material_ref).ok_or_else(|| {
            WorkcellError::NotFound(format!(
                "service binding `{}` is not known by this provider",
                allocation.material_ref
            ))
        })
    }
}

impl ProviderPort for StaticServiceProvider {
    fn provider_ref(&self) -> &ProviderRef {
        &self.provider_ref
    }

    fn port_kind(&self) -> ProviderPortKind {
        ProviderPortKind::Service
    }

    fn offers(&self) -> Result<Vec<OperationalOffer>> {
        let mut offers = Vec::with_capacity(self.services.len());
        for service in self.services.values() {
            let availability = match service.health {
                HealthState::Healthy => Availability::Available,
                HealthState::Degraded | HealthState::Unknown => Availability::Degraded,
                HealthState::Unavailable => Availability::Unavailable,
            };
            let mut metadata = service.metadata.clone();
            metadata.insert("implementation".into(), "static-service".into());
            metadata.insert("logical_ref".into(), service.logical_ref.clone());
            offers.push(OperationalOffer {
                offer_ref: OfferRef::new(format!(
                    "offer:{}:service:{}",
                    self.provider_ref,
                    stable_key(&[&service.logical_ref])
                ))
                .map_err(|error| WorkcellError::OperationFailed(error.into()))?,
                provider_ref: self.provider_ref.clone(),
                port: ProviderPortKind::Service.as_str().into(),
                affordances: vec![],
                connections: vec![service.logical_ref.clone()],
                exposures: vec![],
                isolation_trust: vec![],
                availability,
                health: service.health.clone(),
                capacity: BTreeMap::new(),
                metadata,
            });
        }
        Ok(offers)
    }
}

impl ServiceProvider for StaticServiceProvider {
    fn resolve_service(&mut self, request: &ServiceMaterialRequest) -> Result<ProviderAllocation> {
        let logical_ref = request.connection.as_str();
        let service = self.services.get(logical_ref).cloned().ok_or_else(|| {
            WorkcellError::UnsatisfiedDemand(format!(
                "logical service `{logical_ref}` is not offered"
            ))
        })?;
        if service.health == HealthState::Unavailable {
            return Err(WorkcellError::Unavailable(format!(
                "logical service `{logical_ref}` is unavailable"
            )));
        }

        let key = stable_key(&[request.demand_ref.as_str(), logical_ref, &service.endpoint]);
        let material_ref = format!("service:static:{key}");
        let mut properties = BTreeMap::new();
        properties.insert("logical_ref".into(), logical_ref.into());
        properties.insert("endpoint".into(), service.endpoint.clone());
        let mut provenance = BTreeMap::new();
        provenance.insert("implementation".into(), "static-service".into());
        provenance.insert("logical_ref".into(), logical_ref.into());

        self.records.insert(material_ref.clone(), service.clone());
        Ok(ProviderAllocation {
            provider_ref: self.provider_ref.clone(),
            port: ProviderPortKind::Service,
            material_ref,
            health: service.health,
            properties,
            provenance,
        })
    }

    fn observe_service(&self, allocation: &ProviderAllocation) -> Result<ProviderObservation> {
        let service = self.record(allocation)?;
        let mut detail = BTreeMap::new();
        detail.insert("logical_ref".into(), service.logical_ref.clone());
        detail.insert("endpoint".into(), service.endpoint.clone());
        Ok(ProviderObservation {
            provider_ref: self.provider_ref.clone(),
            material_ref: allocation.material_ref.clone(),
            health: service.health.clone(),
            detail,
        })
    }

    fn release_service(
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
            _ => {
                self.records.remove(&allocation.material_ref);
                Ok(ProviderReleaseResult {
                    provider_ref: self.provider_ref.clone(),
                    material_ref: allocation.material_ref.clone(),
                    disposition: ReleaseDisposition::Released,
                    changed: false,
                })
            }
        }
    }
}
