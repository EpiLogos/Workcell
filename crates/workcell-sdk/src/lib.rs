//! Stable author-facing facade over the existing Workcell core/control contracts.
//!
//! This crate does not add a second materialisation model. It packages the
//! already-canonical client and provider ports so external providers/clients do
//! not need to depend on Workcell implementation crates.

pub const WORKCELL_SDK_VERSION: &str = env!("CARGO_PKG_VERSION");

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
        PersistenceScope, PlanStatus, ProjectRuntimeRequirement, RequirementNecessity,
        ResourceRequirement, RetentionExpectation, StorageAccess, StorageRequirement,
        StorageSharing, Tiered, WorkcellRef, WorkspaceAccess, WorkspaceRequirement,
    };
}

pub mod fabric {
    pub use epilogos_workcell_fabric::{
        evaluate_fabric, evaluate_fabric_with_policies, require_fabric_plan, FabricDiagnostic,
        FabricDiagnosticKind, FabricPathOffer, FabricPathProvider, FabricPathState, FabricPlan,
        FabricPolicyOffer, FabricPolicyProvider, FabricPolicyResult, FabricPolicyState,
        MaterialFabricBinding, NetworkEndpoint, NetworkRelationship, NetworkSecurity,
        ReachabilityScope, RequiredNetworkRelationship,
    };
}

pub mod provider {
    pub use epilogos_workcell_core::{
        validate_allocation, validate_provider_port, ArtifactChannelRequest,
        ArtifactStorageProvider, AttachedStorageRequest, Availability, Capacity, CheckpointRequest,
        ExecutionMaterialRequest, ExecutionProvider, HealthState, LeaseRenewalRequest,
        MaterialCheckpoint, MaterialCheckpointProvider, MaterialCheckpointState,
        MaterialExposureProvider, MaterialLease, MaterialLeaseProvider, OfferRef, OperationalOffer,
        ProjectRuntimeMaterialRequest, ProjectRuntimeProvider, ProviderAllocation,
        ProviderCollectedMaterial, ProviderExposedSurface, ProviderExposureRequest,
        ProviderObservation, ProviderOperation, ProviderOperationResult, ProviderPort,
        ProviderPortKind, ProviderRef, ProviderReleaseResult, Result, ServiceMaterialRequest,
        ServiceProvider, StorageProvider, WorkcellError, WorkspaceMaterialRequest,
        WorkspaceMaterialSource, WorkspaceProvider,
    };
}

pub mod testkit {
    use std::collections::{BTreeMap, BTreeSet};

    use epilogos_workcell_core::{
        validate_provider_port, Availability, ExecutionMaterialRequest, ExecutionProvider,
        HealthState, OfferRef, OperationalOffer, ProviderAllocation, ProviderObservation,
        ProviderOperation, ProviderOperationResult, ProviderPort, ProviderPortKind, ProviderRef,
        ProviderReleaseResult, ReleaseDisposition, Result, RetentionExpectation, WorkcellError,
    };

    #[derive(Clone, Debug, Eq, PartialEq)]
    pub struct ProviderConformance {
        pub provider_ref: ProviderRef,
        pub port: ProviderPortKind,
        pub offer_count: usize,
        pub available_offers: usize,
        pub degraded_offers: usize,
        pub unavailable_offers: usize,
    }

    impl ProviderConformance {
        pub fn summary(&self) -> String {
            format!(
                "{} {}: {} offers ({} available, {} degraded, {} unavailable)",
                self.provider_ref,
                self.port.as_str(),
                self.offer_count,
                self.available_offers,
                self.degraded_offers,
                self.unavailable_offers
            )
        }
    }

    /// Verify the provider-neutral invariants common to every public provider
    /// port. Provider-specific live behavior remains the provider's own test
    /// responsibility; this catches identity/port/offer drift before a provider
    /// is admitted to a Workcell composition.
    pub fn verify_provider_port<P: ProviderPort>(provider: &P) -> Result<ProviderConformance> {
        validate_provider_port(provider)?;
        let offers = provider.offers()?;
        Ok(ProviderConformance {
            provider_ref: provider.provider_ref().clone(),
            port: provider.port_kind(),
            offer_count: offers.len(),
            available_offers: offers
                .iter()
                .filter(|offer| offer.availability == Availability::Available)
                .count(),
            degraded_offers: offers
                .iter()
                .filter(|offer| offer.availability == Availability::Degraded)
                .count(),
            unavailable_offers: offers
                .iter()
                .filter(|offer| offer.availability == Availability::Unavailable)
                .count(),
        })
    }

    #[derive(Clone, Debug, Eq, PartialEq)]
    pub struct ProviderInventoryDelta {
        pub added: Vec<ProviderRef>,
        pub removed: Vec<ProviderRef>,
        pub retained: Vec<ProviderRef>,
    }

    /// Compare provider inventory by provider identity only. Removing or
    /// replacing a provider changes material availability; it never rewrites a
    /// caller semantic reference.
    pub fn diff_provider_inventory(
        before: &[ProviderConformance],
        after: &[ProviderConformance],
    ) -> ProviderInventoryDelta {
        let before_refs = before
            .iter()
            .map(|report| report.provider_ref.clone())
            .collect::<BTreeSet<_>>();
        let after_refs = after
            .iter()
            .map(|report| report.provider_ref.clone())
            .collect::<BTreeSet<_>>();
        ProviderInventoryDelta {
            added: after_refs.difference(&before_refs).cloned().collect(),
            removed: before_refs.difference(&after_refs).cloned().collect(),
            retained: before_refs.intersection(&after_refs).cloned().collect(),
        }
    }

    #[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
    pub enum ExecutionFault {
        Offers,
        Prepare,
        Execute,
        Observe,
        Release,
    }

    /// Public deterministic execution-provider fixture for SDK authors.
    ///
    /// It deliberately has no privileged runtime access. Authors can inject
    /// offer and lifecycle failures through the same public traits their real
    /// provider must implement.
    pub struct FaultingExecutionProvider {
        provider_ref: ProviderRef,
        availability: Availability,
        health: HealthState,
        faults: BTreeSet<ExecutionFault>,
        allocations: BTreeMap<String, ProviderAllocation>,
    }

    impl FaultingExecutionProvider {
        pub fn new(provider_ref: ProviderRef) -> Self {
            Self {
                provider_ref,
                availability: Availability::Available,
                health: HealthState::Healthy,
                faults: BTreeSet::new(),
                allocations: BTreeMap::new(),
            }
        }

        pub fn with_availability(
            mut self,
            availability: Availability,
            health: HealthState,
        ) -> Self {
            self.availability = availability;
            self.health = health;
            self
        }

        pub fn with_fault(mut self, fault: ExecutionFault) -> Self {
            self.faults.insert(fault);
            self
        }

        fn fail_if(&self, fault: ExecutionFault, operation: &str) -> Result<()> {
            if self.faults.contains(&fault) {
                return Err(WorkcellError::OperationFailed(format!(
                    "injected execution-provider {operation} failure"
                )));
            }
            Ok(())
        }

        fn known_allocation(&self, allocation: &ProviderAllocation) -> Result<()> {
            if allocation.provider_ref != self.provider_ref {
                return Err(WorkcellError::OperationFailed(
                    "fixture allocation changed provider identity".into(),
                ));
            }
            if !self.allocations.contains_key(&allocation.material_ref) {
                return Err(WorkcellError::NotFound(format!(
                    "fixture allocation `{}` is not present",
                    allocation.material_ref
                )));
            }
            Ok(())
        }
    }

    impl ProviderPort for FaultingExecutionProvider {
        fn provider_ref(&self) -> &ProviderRef {
            &self.provider_ref
        }

        fn port_kind(&self) -> ProviderPortKind {
            ProviderPortKind::Execution
        }

        fn offers(&self) -> Result<Vec<OperationalOffer>> {
            self.fail_if(ExecutionFault::Offers, "offers")?;
            Ok(vec![OperationalOffer {
                offer_ref: OfferRef::new(format!("offer:{}:fixture", self.provider_ref))
                    .map_err(|error| WorkcellError::OperationFailed(error.into()))?,
                provider_ref: self.provider_ref.clone(),
                port: ProviderPortKind::Execution.as_str().into(),
                affordances: vec!["shell".into()],
                connections: Vec::new(),
                exposures: Vec::new(),
                isolation_trust: Vec::new(),
                availability: self.availability.clone(),
                health: self.health.clone(),
                capacity: BTreeMap::new(),
                metadata: BTreeMap::from([("fixture".into(), "faulting-execution".into())]),
            }])
        }
    }

    impl ExecutionProvider for FaultingExecutionProvider {
        fn prepare_execution(
            &mut self,
            request: &ExecutionMaterialRequest,
        ) -> Result<ProviderAllocation> {
            self.fail_if(ExecutionFault::Prepare, "prepare")?;
            let material_ref = format!("execution:fixture:{}", request.demand_ref.as_str());
            let allocation = ProviderAllocation {
                provider_ref: self.provider_ref.clone(),
                port: ProviderPortKind::Execution,
                material_ref: material_ref.clone(),
                health: self.health.clone(),
                properties: BTreeMap::from([("logical_ref".into(), "execution:fixture".into())]),
                provenance: BTreeMap::from([("fixture".into(), "faulting-execution".into())]),
            };
            self.allocations.insert(material_ref, allocation.clone());
            Ok(allocation)
        }

        fn execute_operation(
            &mut self,
            allocation: &ProviderAllocation,
            operation: &ProviderOperation,
        ) -> Result<ProviderOperationResult> {
            self.fail_if(ExecutionFault::Execute, "execute")?;
            self.known_allocation(allocation)?;
            Ok(ProviderOperationResult {
                provider_ref: self.provider_ref.clone(),
                material_ref: allocation.material_ref.clone(),
                operation: operation.key.clone(),
                output: BTreeMap::new(),
                provenance: BTreeMap::from([("fixture".into(), "faulting-execution".into())]),
            })
        }

        fn observe_execution(
            &self,
            allocation: &ProviderAllocation,
        ) -> Result<ProviderObservation> {
            self.fail_if(ExecutionFault::Observe, "observe")?;
            self.known_allocation(allocation)?;
            Ok(ProviderObservation {
                provider_ref: self.provider_ref.clone(),
                material_ref: allocation.material_ref.clone(),
                health: self.health.clone(),
                detail: BTreeMap::from([("fixture".into(), "faulting-execution".into())]),
            })
        }

        fn release_execution(
            &mut self,
            allocation: &ProviderAllocation,
            retention: &RetentionExpectation,
        ) -> Result<ProviderReleaseResult> {
            self.fail_if(ExecutionFault::Release, "release")?;
            self.known_allocation(allocation)?;
            let disposition = match retention {
                RetentionExpectation::Preserve => ReleaseDisposition::Preserved,
                _ => ReleaseDisposition::Released,
            };
            let changed = disposition == ReleaseDisposition::Released;
            if changed {
                self.allocations.remove(&allocation.material_ref);
            }
            Ok(ProviderReleaseResult {
                provider_ref: self.provider_ref.clone(),
                material_ref: allocation.material_ref.clone(),
                disposition,
                changed,
            })
        }
    }
}
