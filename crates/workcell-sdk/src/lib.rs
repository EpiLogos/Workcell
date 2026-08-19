//! Stable author-facing facade over the existing Workcell core/control contracts.
//!
//! This crate does not add a second materialisation model. It packages the
//! already-canonical client and provider ports so external providers/clients do
//! not need to depend on Workcell implementation crates.

pub mod client {
    pub use epilogos_workcell_control::{
        ControlClient, ControlClientError, ControlTransport, DirectTransport,
        LengthPrefixedTransport, TcpControlServer, TcpControlTransport, TransportFailure,
        UnavailableTransport, CONTROL_PROTOCOL_VERSION,
    };
}

pub mod contract {
    pub use epilogos_workcell_core::{
        AffordanceRequirement, DemandRef, ExecutionDemand, ExposureRequirement, ExternalRef,
        IsolationTrustRequirement, LogicalConnectionRequirement, OutputRequirement,
        PersistenceScope, ProjectRuntimeRequirement, ResourceRequirement, RetentionExpectation,
        Tiered, WorkspaceAccess, WorkspaceRequirement,
    };
}

pub mod fabric {
    pub use epilogos_workcell_fabric::{
        evaluate_fabric, require_fabric_plan, FabricDiagnostic, FabricDiagnosticKind,
        FabricPathOffer, FabricPathProvider, FabricPathState, FabricPlan, FabricPolicyResult,
        MaterialFabricBinding, NetworkRelationship, NetworkSecurity, ReachabilityScope,
        RequiredNetworkRelationship,
    };
}

pub mod provider {
    pub use epilogos_workcell_core::{
        validate_allocation, validate_provider_port, ArtifactChannelRequest,
        ArtifactStorageProvider, Availability, Capacity, ExecutionMaterialRequest,
        ExecutionProvider, HealthState, MaterialExposureProvider, OfferRef, OperationalOffer,
        ProjectRuntimeMaterialRequest, ProjectRuntimeProvider, ProviderAllocation,
        ProviderCollectedMaterial, ProviderExposedSurface, ProviderExposureRequest,
        ProviderObservation, ProviderOperation, ProviderOperationResult, ProviderPort,
        ProviderPortKind, ProviderRef, ProviderReleaseResult, Result, ServiceMaterialRequest,
        ServiceProvider, WorkcellError, WorkspaceMaterialRequest, WorkspaceMaterialSource,
        WorkspaceProvider,
    };
}

pub mod testkit {
    use epilogos_workcell_core::{
        validate_provider_port, ProviderPort, ProviderPortKind, ProviderRef, Result,
    };

    #[derive(Clone, Debug, Eq, PartialEq)]
    pub struct ProviderConformance {
        pub provider_ref: ProviderRef,
        pub port: ProviderPortKind,
        pub offer_count: usize,
    }

    /// Verify the provider-neutral invariants common to every public provider
    /// port. Provider-specific live behavior remains the provider's own test
    /// responsibility; this catches identity/port/offer drift before a provider
    /// is admitted to a Workcell composition.
    pub fn verify_provider_port<P: ProviderPort>(provider: &P) -> Result<ProviderConformance> {
        validate_provider_port(provider)?;
        Ok(ProviderConformance {
            provider_ref: provider.provider_ref().clone(),
            port: provider.port_kind(),
            offer_count: provider.offers()?.len(),
        })
    }
}
