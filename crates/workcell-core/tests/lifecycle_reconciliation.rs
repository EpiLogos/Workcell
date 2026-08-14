use std::{
    collections::{BTreeMap, BTreeSet},
    sync::{Arc, Mutex},
};

use epilogos_workcell_core::{
    Availability, Binding, BindingGraph, BindingPresence, BindingRef, DemandRef,
    DesiredMaterialState, ExecutionMaterialRequest, ExecutionProvider, ExternalRef, HealthState,
    MaterialisedExecutionWorld, OfferRef, OperationalOffer, PersistenceScope,
    PreparedWorldControlPlane, ProviderAllocation, ProviderObservation, ProviderOperation,
    ProviderOperationResult, ProviderPort, ProviderPortKind, ProviderRef, ProviderReleaseResult,
    ReleaseDisposition, RequirementNecessity, RetentionExpectation, WorkcellControlPlane,
    WorkcellError, WorkcellRef, WorldRef,
};

#[derive(Default)]
struct ProviderState {
    material: BTreeMap<String, HealthState>,
    releases: BTreeMap<String, usize>,
}

struct LifecycleExecutionProvider {
    provider_ref: ProviderRef,
    offer_ref: OfferRef,
    state: Arc<Mutex<ProviderState>>,
    advertise: bool,
    fail_release: BTreeSet<String>,
}

impl LifecycleExecutionProvider {
    fn new(provider_ref: &str, offer_ref: &str, state: Arc<Mutex<ProviderState>>) -> Self {
        Self {
            provider_ref: ProviderRef::new(provider_ref).unwrap(),
            offer_ref: OfferRef::new(offer_ref).unwrap(),
            state,
            advertise: true,
            fail_release: BTreeSet::new(),
        }
    }

    fn without_offer(mut self) -> Self {
        self.advertise = false;
        self
    }

    fn failing_release(mut self, material_ref: &str) -> Self {
        self.fail_release.insert(material_ref.into());
        self
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
        if !self.advertise {
            return Ok(Vec::new());
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
            capacity: BTreeMap::new(),
            metadata: BTreeMap::new(),
        }])
    }
}

impl ExecutionProvider for LifecycleExecutionProvider {
    fn prepare_execution(
        &mut self,
        _: &ExecutionMaterialRequest,
    ) -> epilogos_workcell_core::Result<ProviderAllocation> {
        Err(WorkcellError::Unsupported(
            "lifecycle fixture does not prepare".into(),
        ))
    }

    fn execute_operation(
        &mut self,
        _: &ProviderAllocation,
        _: &ProviderOperation,
    ) -> epilogos_workcell_core::Result<ProviderOperationResult> {
        Err(WorkcellError::Unsupported(
            "lifecycle fixture does not execute operations".into(),
        ))
    }

    fn observe_execution(
        &self,
        allocation: &ProviderAllocation,
    ) -> epilogos_workcell_core::Result<ProviderObservation> {
        let state = self.state.lock().unwrap();
        let health = state
            .material
            .get(&allocation.material_ref)
            .cloned()
            .ok_or_else(|| {
                WorkcellError::NotFound(format!("material `{}` is absent", allocation.material_ref))
            })?;
        Ok(ProviderObservation {
            provider_ref: self.provider_ref.clone(),
            material_ref: allocation.material_ref.clone(),
            health,
            detail: BTreeMap::from([("observed_by".into(), "lifecycle-fixture".into())]),
        })
    }

    fn release_execution(
        &mut self,
        allocation: &ProviderAllocation,
        retention: &RetentionExpectation,
    ) -> epilogos_workcell_core::Result<ProviderReleaseResult> {
        if self.fail_release.contains(&allocation.material_ref) {
            return Err(WorkcellError::CleanupFailed(format!(
                "fixture cleanup failed for `{}`",
                allocation.material_ref
            )));
        }
        let mut state = self.state.lock().unwrap();
        *state
            .releases
            .entry(allocation.material_ref.clone())
            .or_insert(0) += 1;
        let (disposition, changed) = match retention {
            RetentionExpectation::Preserve => (ReleaseDisposition::Preserved, false),
            RetentionExpectation::Release => {
                state.material.remove(&allocation.material_ref);
                (ReleaseDisposition::Released, true)
            }
            RetentionExpectation::SuspendIfSupported => {
                state
                    .material
                    .insert(allocation.material_ref.clone(), HealthState::Degraded);
                (ReleaseDisposition::Suspended, true)
            }
            RetentionExpectation::SnapshotIfSupported => (ReleaseDisposition::Snapshotted, true),
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
            .get("observed_by")
            .map(String::as_str),
        Some("lifecycle-fixture")
    );

    let mut missing =
        PreparedWorldControlPlane::new(WorkcellRef::new("workcell:lifecycle").unwrap());
    missing.register_world(world).unwrap();
    let observed = missing
        .observe(&WorldRef::new("world:observe").unwrap())
        .unwrap();
    assert_eq!(observed.observations[0].state, HealthState::Unavailable);
    assert!(observed.observations[0]
        .detail
        .contains_key("observation_error"));
}

#[test]
fn persisted_world_can_be_recovered_after_control_plane_restart_without_new_identity() {
    let state = Arc::new(Mutex::new(ProviderState::default()));
    state
        .lock()
        .unwrap()
        .material
        .insert("material:persistent".into(), HealthState::Healthy);
    let mut persisted = world(
        "world:restart",
        PersistenceScope::Project,
        RetentionExpectation::Preserve,
        vec![binding("execution:main", "material:persistent")],
    );
    persisted.binding_graph.bindings[0].presence = BindingPresence::Stale;
    let semantic_ref = persisted.subjects.get("candidate").unwrap().clone();

    let mut restarted =
        PreparedWorldControlPlane::new(WorkcellRef::new("workcell:lifecycle").unwrap());
    restarted.register_world(persisted).unwrap();
    restarted
        .register_execution_provider(LifecycleExecutionProvider::new(
            "provider:lifecycle",
            "offer:lifecycle",
            state,
        ))
        .unwrap();

    let result = restarted
        .reconcile(&[DesiredMaterialState {
            logical_ref: "execution:main".into(),
            desired: "present".into(),
        }])
        .unwrap();
    assert_eq!(
        result.deltas[0].observed.as_deref(),
        Some("present:healthy")
    );
    assert_eq!(result.deltas[0].action, None);
    let recovered = restarted
        .world(&WorldRef::new("world:restart").unwrap())
        .unwrap();
    assert_eq!(
        recovered.binding_graph.bindings[0].presence,
        BindingPresence::Present
    );
    assert_eq!(recovered.subjects.get("candidate"), Some(&semantic_ref));
}

#[test]
fn missing_ephemeral_material_is_reported_lost_not_recreated() {
    let state = Arc::new(Mutex::new(ProviderState::default()));
    let mut plane = PreparedWorldControlPlane::new(WorkcellRef::new("workcell:lifecycle").unwrap());
    plane
        .register_world(world(
            "world:ephemeral",
            PersistenceScope::Ephemeral,
            RetentionExpectation::Release,
            vec![binding("execution:ephemeral", "material:gone")],
        ))
        .unwrap();
    plane
        .register_execution_provider(LifecycleExecutionProvider::new(
            "provider:lifecycle",
            "offer:lifecycle",
            state,
        ))
        .unwrap();

    let result = plane
        .reconcile(&[DesiredMaterialState {
            logical_ref: "execution:ephemeral".into(),
            desired: "present".into(),
        }])
        .unwrap();
    assert!(result.deltas[0]
        .observed
        .as_deref()
        .unwrap()
        .starts_with("missing:"));
    assert_eq!(result.deltas[0].action.as_deref(), Some("lost"));
    assert_eq!(
        plane
            .world(&WorldRef::new("world:ephemeral").unwrap())
            .unwrap()
            .binding_graph
            .bindings[0]
            .presence,
        BindingPresence::Missing
    );
}

#[test]
fn withdrawn_offer_marks_binding_stale_and_requests_recovery() {
    let state = Arc::new(Mutex::new(ProviderState::default()));
    state
        .lock()
        .unwrap()
        .material
        .insert("material:orphan".into(), HealthState::Healthy);
    let mut plane = PreparedWorldControlPlane::new(WorkcellRef::new("workcell:lifecycle").unwrap());
    plane
        .register_world(world(
            "world:orphan",
            PersistenceScope::Project,
            RetentionExpectation::Preserve,
            vec![binding("execution:orphan", "material:orphan")],
        ))
        .unwrap();
    plane
        .register_execution_provider(
            LifecycleExecutionProvider::new("provider:lifecycle", "offer:lifecycle", state)
                .without_offer(),
        )
        .unwrap();

    let result = plane
        .reconcile(&[DesiredMaterialState {
            logical_ref: "execution:orphan".into(),
            desired: "present".into(),
        }])
        .unwrap();
    assert!(result.deltas[0]
        .observed
        .as_deref()
        .unwrap()
        .starts_with("stale:"));
    assert_eq!(result.deltas[0].action.as_deref(), Some("recover"));
    assert_eq!(
        plane
            .world(&WorldRef::new("world:orphan").unwrap())
            .unwrap()
            .binding_graph
            .bindings[0]
            .presence,
        BindingPresence::Stale
    );
}

#[test]
fn repeated_release_is_idempotent_at_the_control_plane_boundary() {
    let state = Arc::new(Mutex::new(ProviderState::default()));
    state
        .lock()
        .unwrap()
        .material
        .insert("material:release".into(), HealthState::Healthy);
    let mut plane = PreparedWorldControlPlane::new(WorkcellRef::new("workcell:lifecycle").unwrap());
    plane
        .register_world(world(
            "world:release",
            PersistenceScope::TaskOrRun,
            RetentionExpectation::Release,
            vec![binding("execution:release", "material:release")],
        ))
        .unwrap();
    plane
        .register_execution_provider(LifecycleExecutionProvider::new(
            "provider:lifecycle",
            "offer:lifecycle",
            state.clone(),
        ))
        .unwrap();

    let first = plane
        .release(&WorldRef::new("world:release").unwrap())
        .unwrap();
    let second = plane
        .release(&WorldRef::new("world:release").unwrap())
        .unwrap();
    assert!(first.changed);
    assert!(!second.changed);
    assert_eq!(
        state
            .lock()
            .unwrap()
            .releases
            .get("material:release")
            .copied(),
        Some(1)
    );
}

#[test]
fn partial_cleanup_failure_preserves_successful_release_state() {
    let state = Arc::new(Mutex::new(ProviderState::default()));
    {
        let mut locked = state.lock().unwrap();
        locked
            .material
            .insert("material:first".into(), HealthState::Healthy);
        locked
            .material
            .insert("material:second".into(), HealthState::Healthy);
    }
    let mut plane = PreparedWorldControlPlane::new(WorkcellRef::new("workcell:lifecycle").unwrap());
    plane
        .register_world(world(
            "world:partial",
            PersistenceScope::TaskOrRun,
            RetentionExpectation::Release,
            vec![
                binding("execution:first", "material:first"),
                binding("execution:second", "material:second"),
            ],
        ))
        .unwrap();
    plane
        .register_execution_provider(
            LifecycleExecutionProvider::new("provider:lifecycle", "offer:lifecycle", state)
                .failing_release("material:second"),
        )
        .unwrap();

    assert!(matches!(
        plane.release(&WorldRef::new("world:partial").unwrap()),
        Err(WorkcellError::CleanupFailed(_))
    ));
    let world = plane
        .world(&WorldRef::new("world:partial").unwrap())
        .unwrap();
    assert_eq!(
        world.binding_graph.bindings[0].presence,
        BindingPresence::Released
    );
    assert_eq!(
        world.binding_graph.bindings[1].presence,
        BindingPresence::Present
    );
}

#[test]
fn reconcile_can_apply_lifecycle_action_and_report_unbound_target() {
    let state = Arc::new(Mutex::new(ProviderState::default()));
    state
        .lock()
        .unwrap()
        .material
        .insert("material:suspend".into(), HealthState::Healthy);
    let mut plane = PreparedWorldControlPlane::new(WorkcellRef::new("workcell:lifecycle").unwrap());
    plane
        .register_world(world(
            "world:reconcile",
            PersistenceScope::TaskOrRun,
            RetentionExpectation::Preserve,
            vec![binding("execution:suspend", "material:suspend")],
        ))
        .unwrap();
    plane
        .register_execution_provider(LifecycleExecutionProvider::new(
            "provider:lifecycle",
            "offer:lifecycle",
            state,
        ))
        .unwrap();

    let result = plane
        .reconcile(&[
            DesiredMaterialState {
                logical_ref: "execution:suspend".into(),
                desired: "suspended".into(),
            },
            DesiredMaterialState {
                logical_ref: "execution:missing-target".into(),
                desired: "present".into(),
            },
        ])
        .unwrap();
    assert_eq!(result.deltas[0].action.as_deref(), Some("suspended"));
    assert_eq!(result.deltas[1].observed, None);
    assert_eq!(result.deltas[1].action.as_deref(), Some("unbound"));
    assert_eq!(
        plane
            .world(&WorldRef::new("world:reconcile").unwrap())
            .unwrap()
            .binding_graph
            .bindings[0]
            .presence,
        BindingPresence::Suspended
    );
}

#[test]
fn repeated_reconcile_to_suspended_is_idempotent() {
    let state = Arc::new(Mutex::new(ProviderState::default()));
    state
        .lock()
        .unwrap()
        .material
        .insert("material:idempotent-suspend".into(), HealthState::Healthy);
    let mut plane = PreparedWorldControlPlane::new(WorkcellRef::new("workcell:lifecycle").unwrap());
    plane
        .register_world(world(
            "world:idempotent-suspend",
            PersistenceScope::TaskOrRun,
            RetentionExpectation::Preserve,
            vec![binding(
                "execution:idempotent-suspend",
                "material:idempotent-suspend",
            )],
        ))
        .unwrap();
    plane
        .register_execution_provider(LifecycleExecutionProvider::new(
            "provider:lifecycle",
            "offer:lifecycle",
            state.clone(),
        ))
        .unwrap();

    let desired = [DesiredMaterialState {
        logical_ref: "execution:idempotent-suspend".into(),
        desired: "suspended".into(),
    }];
    let first = plane.reconcile(&desired).unwrap();
    let second = plane.reconcile(&desired).unwrap();

    assert_eq!(first.deltas[0].action.as_deref(), Some("suspended"));
    assert_eq!(second.deltas[0].action, None);
    assert_eq!(
        state
            .lock()
            .unwrap()
            .releases
            .get("material:idempotent-suspend")
            .copied(),
        Some(1)
    );
}
