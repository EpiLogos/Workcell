use std::collections::{BTreeMap, BTreeSet};

use crate::{
    HealthState, OperationalOffer, ProviderRef, ReleaseDisposition, Result, WorkcellError,
};

#[non_exhaustive]
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub enum ProviderPortKind {
    Workspace,
    Execution,
    ProjectRuntime,
    Service,
    ArtifactStorage,
}

impl ProviderPortKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Workspace => "workspace",
            Self::Execution => "execution",
            Self::ProjectRuntime => "project-runtime",
            Self::Service => "service",
            Self::ArtifactStorage => "artifact-storage",
        }
    }
}

pub trait ProviderPort {
    fn provider_ref(&self) -> &ProviderRef;
    fn port_kind(&self) -> ProviderPortKind;
    fn offers(&self) -> Result<Vec<OperationalOffer>>;
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ProviderAllocation {
    pub provider_ref: ProviderRef,
    pub port: ProviderPortKind,
    pub material_ref: String,
    pub health: HealthState,
    pub properties: BTreeMap<String, String>,
    pub provenance: BTreeMap<String, String>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ProviderObservation {
    pub provider_ref: ProviderRef,
    pub material_ref: String,
    pub health: HealthState,
    pub detail: BTreeMap<String, String>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ProviderReleaseResult {
    pub provider_ref: ProviderRef,
    pub material_ref: String,
    pub disposition: ReleaseDisposition,
    pub changed: bool,
}

pub fn validate_allocation(
    provider: &impl ProviderPort,
    allocation: &ProviderAllocation,
) -> Result<()> {
    if allocation.provider_ref != *provider.provider_ref() {
        return Err(WorkcellError::OperationFailed(
            "provider allocation changed provider identity".into(),
        ));
    }
    if allocation.port != provider.port_kind() {
        return Err(WorkcellError::OperationFailed(
            "provider allocation changed provider family".into(),
        ));
    }
    if allocation.material_ref.trim().is_empty() {
        return Err(WorkcellError::OperationFailed(
            "provider allocation material_ref must not be empty".into(),
        ));
    }
    Ok(())
}

pub fn validate_provider_port(provider: &impl ProviderPort) -> Result<()> {
    let offers = provider.offers()?;
    let mut refs = BTreeSet::new();
    for offer in offers {
        if offer.provider_ref != *provider.provider_ref() {
            return Err(WorkcellError::OperationFailed(
                "provider offer changed provider identity".into(),
            ));
        }
        if offer.port != provider.port_kind().as_str() {
            return Err(WorkcellError::OperationFailed(format!(
                "provider offer port `{}` does not match `{}`",
                offer.port,
                provider.port_kind().as_str()
            )));
        }
        if !refs.insert(offer.offer_ref) {
            return Err(WorkcellError::OperationFailed(
                "provider returned duplicate offer refs".into(),
            ));
        }
    }
    Ok(())
}
