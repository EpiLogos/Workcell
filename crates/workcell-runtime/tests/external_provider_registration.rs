//! The external-provider admission seam on the collapsed-local composition.
//!
//! The 2026-09-20 SDK campaign's provider round proved an external author can
//! conform through the SDK (`workcell-sdk`'s specimen) — but conformance alone
//! could not be *selected*: the composition had no way to register a provider
//! the host brings from its own technology. These tests pin that seam:
//! registered externals join discovery and planning like the built-in ports,
//! duplicate identities are refused, and a failing construction refuses the
//! composition instead of starting half a Workcell.

use std::collections::BTreeMap;
use std::sync::Arc;

use epilogos_workcell_core::{
    AffordanceRequirement, Availability, DemandRef, ExecutionDemand, ExecutionMaterialRequest,
    ExecutionProvider, HealthState, OfferRef, OperationalOffer, PlanStatus, ProviderAllocation,
    ProviderObservation, ProviderOperation, ProviderOperationResult, ProviderPort,
    ProviderPortKind, ProviderRef, ProviderReleaseResult, RetentionExpectation,
    WorkcellControlPlane, WorkcellError, WorkcellRef,
};
use epilogos_workcell_runtime::{CollapsedLocalConfig, CollapsedLocalWorkcell};

/// A minimal external execution provider whose offer carries a distinctive
/// affordance, so a planned demand proves *selection*, not mere presence.
struct SpecimenProvider {
    provider_ref: ProviderRef,
}

impl SpecimenProvider {
    const AFFORDANCE: &'static str = "specimen.execute";

    fn new() -> Result<Self, &'static str> {
        Ok(Self {
            provider_ref: ProviderRef::new("provider:external/specimen")?,
        })
    }
}

impl ProviderPort for SpecimenProvider {
    fn provider_ref(&self) -> &ProviderRef {
        &self.provider_ref
    }

    fn port_kind(&self) -> ProviderPortKind {
        ProviderPortKind::Execution
    }

    fn offers(&self) -> epilogos_workcell_core::Result<Vec<OperationalOffer>> {
        Ok(vec![OperationalOffer {
            offer_ref: OfferRef::new("offer:specimen/default")?,
            provider_ref: self.provider_ref.clone(),
            port: self.port_kind().as_str().to_string(),
            affordances: vec![Self::AFFORDANCE.to_string()],
            connections: vec![],
            exposures: vec![],
            isolation_trust: vec![],
            availability: Availability::Available,
            health: HealthState::Healthy,
            capacity: BTreeMap::new(),
            metadata: BTreeMap::new(),
        }])
    }
}

impl ExecutionProvider for SpecimenProvider {
    fn prepare_execution(
        &mut self,
        _request: &ExecutionMaterialRequest,
    ) -> epilogos_workcell_core::Result<ProviderAllocation> {
        Err(WorkcellError::Unsupported(
            "specimen is a selection fixture; it prepares nothing".to_string(),
        ))
    }

    fn execute_operation(
        &mut self,
        _allocation: &ProviderAllocation,
        _operation: &ProviderOperation,
    ) -> epilogos_workcell_core::Result<ProviderOperationResult> {
        Err(WorkcellError::Unsupported(
            "specimen is a selection fixture; it executes nothing".to_string(),
        ))
    }

    fn observe_execution(
        &self,
        _allocation: &ProviderAllocation,
    ) -> epilogos_workcell_core::Result<ProviderObservation> {
        Err(WorkcellError::Unsupported(
            "specimen is a selection fixture; it observes nothing".to_string(),
        ))
    }

    fn release_execution(
        &mut self,
        _allocation: &ProviderAllocation,
        _retention: &RetentionExpectation,
    ) -> epilogos_workcell_core::Result<ProviderReleaseResult> {
        Err(WorkcellError::Unsupported(
            "specimen is a selection fixture; it releases nothing".to_string(),
        ))
    }
}

fn specimen_factory() -> epilogos_workcell_runtime::ExternalExecutionProviderFactory {
    Arc::new(|| Ok(Box::new(SpecimenProvider::new().unwrap())))
}

#[test]
fn a_registered_external_provider_joins_discovery_and_planning() {
    let state = tempfile_root("selection");
    let workcell = CollapsedLocalWorkcell::new(
        CollapsedLocalConfig::new(
            WorkcellRef::new("workcell:external-registration").unwrap(),
            state.clone(),
        )
        .with_external_execution_provider(specimen_factory()),
    )
    .unwrap();

    // Discovery: the external offer sits beside the built-in ports.
    let discovery = workcell.discover().unwrap();
    assert!(
        discovery
            .offers
            .iter()
            .any(|offer| offer.provider_ref.as_str() == "provider:external/specimen"),
        "the external provider's offer must be discoverable: {discovery:?}"
    );

    // Planning: a demand for the external affordance binds the external
    // provider — the caller can tell WHO was selected, not just that
    // something was.
    let mut demand = ExecutionDemand::new(DemandRef::new("demand:specimen").unwrap());
    demand
        .affordances
        .required
        .push(AffordanceRequirement::new(SpecimenProvider::AFFORDANCE).unwrap());
    demand.retention = RetentionExpectation::Release;
    let plan = workcell.plan(&demand).unwrap();
    assert_eq!(plan.status, PlanStatus::Satisfiable);
    assert!(
        plan.planned_bindings
            .iter()
            .any(|binding| binding.provider_ref.as_str() == "provider:external/specimen"),
        "the plan must bind the external provider: {plan:?}"
    );

    // A demand nothing offers stays honestly unsatisfiable.
    let mut nothing = ExecutionDemand::new(DemandRef::new("demand:nothing").unwrap());
    nothing
        .affordances
        .required
        .push(AffordanceRequirement::new("specimen.nobody-offers").unwrap());
    let refused = workcell.plan(&nothing).unwrap();
    assert_eq!(
        refused.status,
        PlanStatus::Unsatisfiable,
        "an unoffered required affordance is unsatisfiable: {refused:?}"
    );
    let _ = state;
}

#[test]
fn a_duplicate_external_identity_is_refused_like_any_built_in() {
    let state = tempfile_root("duplicate");
    let error = match CollapsedLocalWorkcell::new(
        CollapsedLocalConfig::new(
            WorkcellRef::new("workcell:duplicate").unwrap(),
            state.clone(),
        )
        .with_external_execution_provider(specimen_factory())
        // Two externals cannot claim one identity: the control plane's
        // duplicate law answers to them exactly as to the built-ins.
        .with_external_execution_provider(Arc::new(|| {
            Ok(Box::new(SpecimenProvider::new().unwrap()) as Box<dyn ExecutionProvider>)
        })),
    ) {
        Err(error) => error,
        Ok(_) => panic!("duplicate external identity must refuse composition"),
    };
    assert!(
        error.to_string().contains("already registered"),
        "duplicate registration is refused, not merged: {error}"
    );
    let _ = state;
}

#[test]
fn a_failing_construction_refuses_the_whole_composition() {
    let state = tempfile_root("failing");
    let error = match CollapsedLocalWorkcell::new(
        CollapsedLocalConfig::new(WorkcellRef::new("workcell:failing").unwrap(), state)
            .with_external_execution_provider(Arc::new(|| {
                Err(WorkcellError::Unavailable(
                    "specimen backend unreachable".to_string(),
                ))
            })),
    ) {
        Err(error) => error,
        Ok(_) => panic!("a failing construction must refuse the composition"),
    };
    assert!(
        error.to_string().contains("specimen backend unreachable"),
        "the factory's own error surfaces: {error}"
    );
}

fn tempfile_root(label: &str) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "workcell-external-registration-{label}-{}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}
