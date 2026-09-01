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
        ResourceRequirement, RetentionExpectation, StorageAccess, StorageRequirement, StorageSharing,
        Tiered, WorkcellRef, WorkspaceAccess, WorkspaceRequirement,
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

        pub fn fail(mut self, fault: ExecutionFault) -> Self {
            self.faults.insert(fault);
            self
        }

        fn faulted(&self, fault: ExecutionFault) -> bool {
            self.faults.contains(&fault)
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
            if self.faulted(ExecutionFault::Offers) {
                return Err(WorkcellError::Unavailable(
                    "fixture offer discovery failed".into(),
                ));
            }
            Ok(vec![OperationalOffer {
                offer_ref: OfferRef::new(format!("offer:{}:execution", self.provider_ref))?,
                provider_ref: self.provider_ref.clone(),
                port: ProviderPortKind::Execution.as_str().into(),
                affordances: vec!["shell".into()],
                connections: Vec::new(),
                exposures: Vec::new(),
                isolation_trust: Vec::new(),
                availability: self.availability.clone(),
                health: self.health.clone(),
                capacity: BTreeMap::new(),
                metadata: BTreeMap::new(),
            }])
        }
    }

    impl ExecutionProvider for FaultingExecutionProvider {
        fn prepare_execution(
            &mut self,
            request: &ExecutionMaterialRequest,
        ) -> Result<ProviderAllocation> {
            if self.faulted(ExecutionFault::Prepare) {
                return Err(WorkcellError::Unavailable(
                    "fixture execution prepare failed".into(),
                ));
            }
            let material_ref = format!("fixture:{}", request.demand_ref);
            let allocation = ProviderAllocation {
                provider_ref: self.provider_ref.clone(),
                port: ProviderPortKind::Execution,
                material_ref: material_ref.clone(),
                health: self.health.clone(),
                properties: BTreeMap::new(),
                provenance: BTreeMap::from([("fixture".into(), "sdk-testkit".into())]),
            };
            self.allocations.insert(material_ref, allocation.clone());
            Ok(allocation)
        }

        fn execute_operation(
            &mut self,
            allocation: &ProviderAllocation,
            operation: &ProviderOperation,
        ) -> Result<ProviderOperationResult> {
            if self.faulted(ExecutionFault::Execute) {
                return Err(WorkcellError::OperationFailed(
                    "fixture execution operation failed".into(),
                ));
            }
            validate_allocation(self, allocation)?;
            Ok(ProviderOperationResult {
                provider_ref: self.provider_ref.clone(),
                material_ref: allocation.material_ref.clone(),
                operation: operation.key.clone(),
                output: BTreeMap::new(),
                provenance: BTreeMap::from([("fixture".into(), "sdk-testkit".into())]),
            })
        }

        fn observe_execution(&self, allocation: &ProviderAllocation) -> Result<ProviderObservation> {
            if self.faulted(ExecutionFault::Observe) {
                return Err(WorkcellError::Unavailable(
                    "fixture execution observation failed".into(),
                ));
            }
            validate_allocation(self, allocation)?;
            if !self.allocations.contains_key(&allocation.material_ref) {
                return Err(WorkcellError::NotFound(format!(
                    "fixture material `{}` is absent",
                    allocation.material_ref
                )));
            }
            Ok(ProviderObservation {
                provider_ref: self.provider_ref.clone(),
                material_ref: allocation.material_ref.clone(),
                health: self.health.clone(),
                detail: BTreeMap::new(),
            })
        }

        fn release_execution(
            &mut self,
            allocation: &ProviderAllocation,
            retention: &RetentionExpectation,
        ) -> Result<ProviderReleaseResult> {
            if self.faulted(ExecutionFault::Release) {
                return Err(WorkcellError::OperationFailed(
                    "fixture execution release failed".into(),
                ));
            }
            validate_allocation(self, allocation)?;
            let disposition = match retention {
                RetentionExpectation::Release => {
                    self.allocations.remove(&allocation.material_ref);
                    ReleaseDisposition::Released
                }
                RetentionExpectation::Preserve => ReleaseDisposition::Preserved,
                RetentionExpectation::SuspendIfSupported => ReleaseDisposition::Suspended,
                RetentionExpectation::SnapshotIfSupported => ReleaseDisposition::Snapshotted,
            };
            Ok(ProviderReleaseResult {
                provider_ref: self.provider_ref.clone(),
                material_ref: allocation.material_ref.clone(),
                disposition,
                changed: matches!(retention, RetentionExpectation::Release),
            })
        }
    }
}
