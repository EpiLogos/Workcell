use std::{
    collections::BTreeMap,
    sync::{Arc, Mutex},
};

use epilogos_workcell_core::{
    Availability, Binding, BindingGraph, BindingPresence, BindingRef, Capacity, DemandRef,
    DesiredMaterialState, ExecutionProvider, ExternalRef, HealthState, MaterialisedExecutionWorld,
    OfferRef, OperationalOffer, PersistenceScope, PreparedWorldControlPlane, ProviderAllocation,
    ProviderObservation, ProviderPort, ProviderPortKind, ProviderRef, ProviderReleaseResult,
    ReconciliationAction, ReleaseDisposition, RequirementNecessity, RetentionExpectation,
    WorkcellControlPlane, WorkcellError, WorkcellRef, WorldRef,
};

#[derive(Clone, Debug, Default)]
struct ProviderState {
    material: BTreeMap<String, HealthState>,
    releases: BTreeMap<String, usize>,
    fail_release_for: Option<String>,
    advertise_offer: bool,
}

struct LifecycleExecutionProvider {
    provider_ref: ProviderRef,
    offer_ref: OfferRef,
    state: Arc<Mutex<ProviderState>>,
}

impl LifecycleExecutionProvider {
    fn new(
        provider_ref: &str,
        offer_ref: &str,
        state: Arc<Mutex<ProviderState>>,
    ) -> Self {
        state.lock().unwrap().advertise_offer = true;
        Self {
            provider_ref: ProviderRef::new(provider_ref).unwrap(),
            offer_ref: OfferRef::new(offer_ref).unwrap(),
            state,
        }
    }
}

impl ProviderPort for LifecycleExecutionProvider {
    fn provider_ref(&self) -> &ProviderRef {
        &self.provider_ref
    }

    fn port_kind(&self) -> ProviderPortKind {
        ProviderPortKind::Execution
    }

    fn offers(&self) -> epilogos_workcell_core::Result<Vec<OperationalOffer>> {
        let state = self.state.lock().unwrap();
        if !state.advertise_offer {
            return Ok(vec![]);
        }
        Ok(vec![OperationalOffer {
            offer_ref: self.offer_ref.clone(),
            provider_ref: self.provider_ref.clone(),
            port: ProviderPortKind::Execution.as_str().into(),
            affordances: vec!["shell".into()],
            connections: vec![],
            exposures: vec![],
            isolation_trust: vec![],
            availability: Availability::Available,
            health: HealthState::Healthy,
            capacity: BTreeMap::from([("cpu".into(), Capacity::Units(4))]),
            metadata: BTreeMap::new(),
        }])
    }
}

impl ExecutionProvider for LifecycleExecutionProvider {
    fn prepare_execution(
        &mut self,
        request: &epilogos_workcell_core::ExecutionMaterialRequest,
    ) -> epilogos_workcell_core::Result<ProviderAllocation> {
        let material_ref = format!("material:{}", request.demand_ref.as_str());
        self.state
            .lock()
            .unwrap()
            .material
            .insert(material_ref.clone(), HealthState::Healthy);
        Ok(ProviderAllocation {
            provider_ref: self.provider_ref.clone(),
            port: ProviderPortKind::Execution,
            material_ref,
            health: HealthState::Healthy,
            properties: BTreeMap::new(),
            provenance: BTreeMap::new(),
        })
    }

    fn observe_execution(
        &self,
        allocation: &ProviderAllocation,
    ) -> epilogos_workcell_core::Result<ProviderObservation> {
        let state = self.state.lock().unwrap();
        let health = state.material.get(&allocation.material_ref).ok_or_else(|| {
            WorkcellError::NotFound(format!("material `{}` is missing", allocation.material_ref))
        })?;
        Ok(ProviderObservation {
            provider_ref: self.provider_ref.clone(),
            material_ref: allocation.material_ref.clone(),
            health: health.clone(),
            detail: BTreeMap::new(),
        })
    }

    fn release_execution(
        &mut self,
        allocation: &ProviderAllocation,
        retention: &RetentionExpectation,
    ) -> epilogos_workcell_core::Result<ProviderReleaseResult> {
        let mut state = self.state.lock().unwrap();
        if state.fail_release_for.as_deref() == Some(&allocation.material_ref) {
            return Err(WorkcellError::CleanupFailed(format!(
                "simulated cleanup failure for `{}`",
                allocation.material_ref
            )));
        }
        let releases = state
            .releases
            .entry(allocation.material_ref.clone())
            .or_insert(0);
        *releases += 1;
        let (disposition, changed) = match retention {
            RetentionExpectation::Preserve => (ReleaseDisposition::Preserved, false),
            RetentionExpectation::Release => {
                state.material.remove(&allocation.material_ref);
                (ReleaseDisposition::Released, true)
            }
            RetentionExpectation::SuspendIfSupported => {
                state
                    .material
                    .insert(allocation.material_ref.clone(), HealthState::Unavailable);
                (ReleaseDisposition::Suspended, true)
            }
            RetentionExpectation::SnapshotIfSupported => {
                state.material.remove(&allocation.material_ref);
                (ReleaseDisposition::Snapshotted, true)
            }
        };
        Ok(ProviderReleaseResult {
            provider_ref: self.provider_ref.clone(),
            material_ref: allocation.material_ref.clone(),
            disposition,
            changed,
        })
    }
}

fn binding(logical_ref: &str, material_ref: &str) -> Binding {
    Binding {
        binding_ref: BindingRef::new(format!("binding:{logical_ref}")).unwrap(),
        logical_ref: logical_ref.into(),
        necessity: RequirementNecessity::Required,
        provider_ref: ProviderRef::new("provider:lifecycle").unwrap(),
        offer_ref: OfferRef::new("offer:lifecycle").unwrap(),
        port: ProviderPortKind::Execution,
        material_ref: material_ref.into(),
        health: HealthState::Unknown,
        presence: BindingPresence::Present,
        properties: BTreeMap::new(),
        provenance: BTreeMap::from([("fixture".into(), "persisted-allocation".into())]),
    }
}

fn world(
    world_ref: &str,
    persistence: PersistenceScope,
    retention: RetentionExpectation,
    bindings: Vec<Binding>,
) -> MaterialisedExecutionWorld {
    MaterialisedExecutionWorld {
        world_ref: WorldRef::new(world_ref).unwrap(),
        workcell_ref: WorkcellRef::new("workcell:lifecycle").unwrap(),
        demand_ref: DemandRef::new(format!("demand:{world_ref}")).unwrap(),
        subjects: BTreeMap::from([(
            "candidate".into(),
            ExternalRef::new("client:candidate-stable").unwrap(),
        )]),
        binding_graph: BindingGraph {
            bindings,
            relations: vec![],
        },
        planned_exposures: vec![],
        planned_constraints: vec![],
        plan_degradations: vec![],
        plan_omissions: vec![],
        persistence: Some(persistence),
        retention,
        state: HealthState::Unknown,
        provenance: BTreeMap::new(),
    }
}

#[test]
fn observe_uses_provider_state_and_reports_provider_disappearance() {
    let state = Arc::new(Mutex::new(ProviderState::default()));
    state
        .lock()
        .unwrap()
        .material
        .insert("material:live".into(), HealthState::Healthy);
    let world = world(
        "world:observe",
        PersistenceScope::Project,
        RetentionExpectation::Preserve,
        vec![binding("execution:main", "material:live")],
    );

    let mut live = PreparedWorldControlPlane::new(WorkcellRef::new("workcell:lifecycle").unwrap());
    live.register_world(world.clone()).unwrap();
    live.register_execution_provider(LifecycleExecutionProvider::new(
        "provider:lifecycle",
        "offer:lifecycle",
        state,
    ))
    .unwrap();
    let observed = live.observe(&world.world_ref).unwrap();
    assert_eq!(observed.observations[0].state, HealthState::Healthy);
    assert_eq!(
        observed.observations[0]
            .detail
            .get("lifecycle")
            .map(String::as_str),
        Some("present")
    );
}

#[test]
fn observe_reports_provider_disappearance_as_unavailable() {
    let world = world(
        "world:provider-disappeared",
        PersistenceScope::Project,
        RetentionExpectation::Preserve,
        vec![binding("execution:main", "material:missing-provider")],
    );
    let mut control =
        PreparedWorldControlPlane::new(WorkcellRef::new("workcell:lifecycle").unwrap());
    control.register_world(world.clone()).unwrap();
    let observed = control.observe(&world.world_ref).unwrap();
    assert_eq!(observed.observations[0].state, HealthState::Unavailable);
    assert!(observed.observations[0]
        .detail
        .get("observation_error")
        .unwrap()
        .contains("provider"));
}

#[test]
fn persisted_world_can_be_recovered_after_control_plane_restart_without_new_identity() {
    let state = Arc::new(Mutex::new(ProviderState::default()));
    state
        .lock()
        .unwrap()
        .material
        .insert("material:restart".into(), HealthState::Healthy);
    let persisted = world(
        "world:restart",
        PersistenceScope::Project,
        RetentionExpectation::Preserve,
        vec![binding("execution:main", "material:restart")],
    );

    let mut restarted =
        PreparedWorldControlPlane::new(WorkcellRef::new("workcell:lifecycle").unwrap());
    restarted.register_world(persisted.clone()).unwrap();
    restarted
        .register_execution_provider(LifecycleExecutionProvider::new(
            "provider:lifecycle",
            "offer:lifecycle",
            state,
        ))
        .unwrap();
    let observed = restarted.observe(&persisted.world_ref).unwrap();
    assert_eq!(observed.world_ref, persisted.world_ref);
    assert_eq!(observed.observations[0].state, HealthState::Healthy);
}

#[test]
fn missing_ephemeral_material_is_reported_lost_not_recreated() {
    let state = Arc::new(Mutex::new(ProviderState::default()));
    let missing = world(
        "world:lost",
        PersistenceScope::Ephemeral,
        RetentionExpectation::Release,
        vec![binding("execution:main", "material:lost")],
    );
    let mut control =
        PreparedWorldControlPlane::new(WorkcellRef::new("workcell:lifecycle").unwrap());
    control.register_world(missing.clone()).unwrap();
    control
        .register_execution_provider(LifecycleExecutionProvider::new(
            "provider:lifecycle",
            "offer:lifecycle",
            state,
        ))
        .unwrap();

    let result = control
        .reconcile(&[DesiredMaterialState {
            logical_ref: "execution:main".into(),
            desired: "present".into(),
        }])
        .unwrap();
    assert_eq!(result.deltas[0].action, ReconciliationAction::Lost);
}

#[test]
fn withdrawn_offer_marks_binding_stale_and_requests_recovery() {
    let state = Arc::new(Mutex::new(ProviderState::default()));
    state
        .lock()
        .unwrap()
        .material
        .insert("material:withdrawn".into(), HealthState::Healthy);
    let persisted = world(
        "world:withdrawn",
        PersistenceScope::Project,
        RetentionExpectation::Preserve,
        vec![binding("execution:main", "material:withdrawn")],
    );
    let mut provider = LifecycleExecutionProvider::new(
        "provider:lifecycle",
        "offer:lifecycle",
        state.clone(),
    );
    state.lock().unwrap().advertise_offer = false;

    let mut control =
        PreparedWorldControlPlane::new(WorkcellRef::new("workcell:lifecycle").unwrap());
    control.register_world(persisted.clone()).unwrap();
    control.register_execution_provider(provider).unwrap();
    let result = control
        .reconcile(&[DesiredMaterialState {
            logical_ref: "execution:main".into(),
            desired: "present".into(),
        }])
        .unwrap();
    assert_eq!(result.deltas[0].action, ReconciliationAction::Recover);
    let world = control.world(&persisted.world_ref).unwrap();
    assert_eq!(world.binding_graph.bindings[0].presence, BindingPresence::Stale);
}

#[test]
fn repeated_release_is_idempotent_at_the_control_plane_boundary() {
    let state = Arc::new(Mutex::new(ProviderState::default()));
    state
        .lock()
        .unwrap()
        .material
        .insert("material:release".into(), HealthState::Healthy);
    let releasable = world(
        "world:release",
        PersistenceScope::TaskOrRun,
        RetentionExpectation::Release,
        vec![binding("execution:main", "material:release")],
    );
    let mut control =
        PreparedWorldControlPlane::new(WorkcellRef::new("workcell:lifecycle").unwrap());
    control.register_world(releasable.clone()).unwrap();
    control
        .register_execution_provider(LifecycleExecutionProvider::new(
            "provider:lifecycle",
            "offer:lifecycle",
            state.clone(),
        ))
        .unwrap();

    let first = control.release(&releasable.world_ref).unwrap();
    let second = control.release(&releasable.world_ref).unwrap();
    assert!(first.changed);
    assert!(!second.changed);
    assert_eq!(
        *state
            .lock()
            .unwrap()
            .releases
            .get("material:release")
            .unwrap(),
        1
    );
}

#[test]
fn repeated_reconcile_to_suspended_is_idempotent() {
    let state = Arc::new(Mutex::new(ProviderState::default()));
    state
        .lock()
        .unwrap()
        .material
        .insert("material:suspend".into(), HealthState::Healthy);
    let suspendable = world(
        "world:suspend",
        PersistenceScope::TaskOrRun,
        RetentionExpectation::SuspendIfSupported,
        vec![binding("execution:main", "material:suspend")],
    );
    let mut control =
        PreparedWorldControlPlane::new(WorkcellRef::new("workcell:lifecycle").unwrap());
    control.register_world(suspendable.clone()).unwrap();
    control
        .register_execution_provider(LifecycleExecutionProvider::new(
            "provider:lifecycle",
            "offer:lifecycle",
            state.clone(),
        ))
        .unwrap();

    let desired = [DesiredMaterialState {
        logical_ref: "execution:main".into(),
        desired: "suspended".into(),
    }];
    let first = control.reconcile(&desired).unwrap();
    let second = control.reconcile(&desired).unwrap();
    assert_eq!(first.deltas[0].action, ReconciliationAction::Suspend);
    assert_eq!(second.deltas[0].action, ReconciliationAction::None);
    assert_eq!(
        *state
            .lock()
            .unwrap()
            .releases
            .get("material:suspend")
            .unwrap(),
        1
    );
}

#[test]
fn partial_cleanup_failure_preserves_successful_release_state() {
    let state = Arc::new(Mutex::new(ProviderState::default()));
    {
        let mut state = state.lock().unwrap();
        state
            .material
            .insert("material:good".into(), HealthState::Healthy);
        state
            .material
            .insert("material:bad".into(), HealthState::Healthy);
        state.fail_release_for = Some("material:bad".into());
    }
    let releasable = world(
        "world:partial",
        PersistenceScope::TaskOrRun,
        RetentionExpectation::Release,
        vec![
            binding("execution:good", "material:good"),
            binding("execution:bad", "material:bad"),
        ],
    );
    let mut control =
        PreparedWorldControlPlane::new(WorkcellRef::new("workcell:lifecycle").unwrap());
    control.register_world(releasable.clone()).unwrap();
    control
        .register_execution_provider(LifecycleExecutionProvider::new(
            "provider:lifecycle",
            "offer:lifecycle",
            state,
        ))
        .unwrap();

    assert!(control.release(&releasable.world_ref).is_err());
    let world = control.world(&releasable.world_ref).unwrap();
    assert_eq!(world.binding_graph.bindings[0].presence, BindingPresence::Stale);
    assert_eq!(world.binding_graph.bindings[1].presence, BindingPresence::Released);
}

#[test]
fn reconcile_can_apply_lifecycle_action_and_report_unbound_target() {
    let state = Arc::new(Mutex::new(ProviderState::default()));
    state
        .lock()
        .unwrap()
        .material
        .insert("material:snapshot".into(), HealthState::Healthy);
    let snapshotable = world(
        "world:snapshot",
        PersistenceScope::Candidate,
        RetentionExpectation::SnapshotIfSupported,
        vec![binding("execution:main", "material:snapshot")],
    );
    let mut control =
        PreparedWorldControlPlane::new(WorkcellRef::new("workcell:lifecycle").unwrap());
    control.register_world(snapshotable.clone()).unwrap();
    control
        .register_execution_provider(LifecycleExecutionProvider::new(
            "provider:lifecycle",
            "offer:lifecycle",
            state,
        ))
        .unwrap();

    let result = control
        .reconcile(&[
            DesiredMaterialState {
                logical_ref: "execution:main".into(),
                desired: "snapshotted".into(),
            },
            DesiredMaterialState {
                logical_ref: "execution:absent".into(),
                desired: "present".into(),
            },
        ])
        .unwrap();
    assert_eq!(result.deltas[0].action, ReconciliationAction::Snapshot);
    assert_eq!(result.deltas[1].action, ReconciliationAction::Lost);
}
