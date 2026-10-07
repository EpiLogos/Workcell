//! Capacity admission through the collapsed-local lifecycle.
//!
//! These tests pin the commission's admission law on the real composition:
//! minimum floors are accounted simultaneously (not one independently
//! acceptable request at a time), reservations ride the native material
//! lifecycle (release frees, failed prepare reconciles, re-entry restores,
//! reconcile retires absent worlds), "waiting for capacity" names its actual
//! reason, and a required floor is never silently rerouted to weaker
//! placement. Budgets are injected via `capacity_budget` so the tests stay
//! deterministic on any host.

use epilogos_workcell_core::WorkcellError;
use epilogos_workcell_core::{
    AffordanceRequirement, DemandRef, ExecutionDemand, PlanStatus, ResourceRequirement,
    RetentionExpectation, WorkcellControlPlane, WorkcellRef,
};
use epilogos_workcell_runtime::{
    AdmissionLedger, BudgetPolicy, CapacityBudget, CollapsedLocalConfig, CollapsedLocalWorkcell,
    ADMISSION_FILE,
};

fn tempfile_root(label: &str) -> std::path::PathBuf {
    let root = std::env::temp_dir().join(format!(
        "wc-capacity-{label}-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(&root).unwrap();
    root
}

fn fixed_budget(memory_task_mib: u64, cpus: u32) -> CapacityBudget {
    // A declared budget, exactly as an operator or test would pin it: the
    // task ceiling is the stated number, not a policy fraction of something
    // larger.
    CapacityBudget {
        memory_ceiling_bytes: memory_task_mib * 1024 * 1024,
        memory_task_bytes: memory_task_mib * 1024 * 1024,
        cpu_task_count: cpus,
        policy: BudgetPolicy::default(),
        observed_at_unix_ms: 1,
    }
}

fn shell_demand(demand_ref: &str, memory_minimum_mib: Option<u64>) -> ExecutionDemand {
    let mut demand = ExecutionDemand::new(DemandRef::new(demand_ref).unwrap());
    demand
        .affordances
        .required
        .push(AffordanceRequirement::new("shell").unwrap());
    demand.retention = RetentionExpectation::Release;
    if let Some(minimum) = memory_minimum_mib {
        demand.resources.push(ResourceRequirement {
            key: "memory".into(),
            minimum: Some(minimum),
            maximum: None,
            unit: Some("MiB".into()),
        });
    }
    demand
}

#[test]
fn simultaneous_allocations_are_accounted_and_release_frees_the_budget() {
    let state = tempfile_root("simultaneous");
    let budget = fixed_budget(8, 1); // 8 MiB task budget: deliberately tiny.
    let mut workcell = CollapsedLocalWorkcell::new(
        CollapsedLocalConfig::new(
            WorkcellRef::new("workcell:capacity").unwrap(),
            state.clone(),
        )
        .with_capacity_budget(budget.clone()),
    )
    .unwrap();

    // The host-process provider prepares without a live workload, so both
    // prepares reach the ledger with no external effects. 5 MiB + 5 MiB does
    // not fit an 8 MiB budget.
    let first = workcell
        .prepare(&shell_demand("demand:first", Some(5)))
        .expect("first allocation fits the budget");
    let second = workcell.prepare(&shell_demand("demand:second", Some(5)));
    match &second {
        Err(WorkcellError::Capacity(reason)) => {
            assert!(
                reason.contains("exceeds task budget") || reason.contains("plus committed"),
                "the refusal names the actual constraint: {reason}"
            );
        }
        other => panic!("the second allocation must wait for capacity, got {other:?}"),
    }

    // The declined attempt is recorded as bounded evidence.
    let readback = workcell.admission_readback();
    assert_eq!(
        readback["waiting"].as_array().map(Vec::len),
        Some(1),
        "one waiting-for-capacity reason is exposed: {readback}"
    );

    // Release frees the budget; the same demand then admits.
    workcell.release(&first.world_ref).unwrap();
    let retried = workcell.prepare(&shell_demand("demand:second", Some(5)));
    assert!(
        retried.is_ok(),
        "released budget admits the retry: {retried:?}"
    );

    let _ = std::fs::remove_dir_all(&state);
}

#[test]
fn a_failed_prepare_releases_its_reservation() {
    let state = tempfile_root("failed-prepare");
    let budget = fixed_budget(8, 1);
    // No providers offer `specimen.nobody-offers`, but admission runs after
    // planning — so use an affordance that plans against the host-process
    // provider and force the failure at the demand level instead: a floor
    // that fits the budget but a provider that cannot prepare. The simplest
    // honest failure: prepare a demand whose storage requirement references
    // an undeclared logical store, after a memory floor was admitted.
    let mut demand = shell_demand("demand:doomed", Some(5));
    demand
        .storage
        .required
        .push(epilogos_workcell_core::StorageRequirement {
            logical_ref: "now:undeclared".into(),
            access: epilogos_workcell_core::StorageAccess::Writable,
            sharing: epilogos_workcell_core::StorageSharing::Shared,
            minimum_capacity: None,
            unit: None,
            persistence: Some(epilogos_workcell_core::PersistenceScope::Ephemeral),
            retention: RetentionExpectation::Release,
        });

    let mut workcell = CollapsedLocalWorkcell::new(
        CollapsedLocalConfig::new(
            WorkcellRef::new("workcell:capacity").unwrap(),
            state.clone(),
        )
        .with_capacity_budget(budget),
    )
    .unwrap();
    let failed = workcell.prepare(&demand);
    assert!(failed.is_err(), "an undeclared storage ref cannot prepare");

    // The reservation for the failed prepare must be gone: the ledger on
    // disk holds no reservation for this demand.
    let ledger = AdmissionLedger::load(&state).unwrap();
    assert!(
        ledger.reservation_for("demand:doomed").is_none(),
        "a failed prepare must not hold budget; ledger: {:?}",
        ledger.as_json()
    );
    let _ = std::fs::remove_dir_all(&state);
}

#[test]
fn reentry_rebuilds_the_reservation_from_durable_provenance() {
    let state = tempfile_root("reentry");
    let budget = fixed_budget(8, 1);
    let mut workcell = CollapsedLocalWorkcell::new(
        CollapsedLocalConfig::new(
            WorkcellRef::new("workcell:capacity").unwrap(),
            state.clone(),
        )
        .with_capacity_budget(budget.clone()),
    )
    .unwrap();
    let world = workcell
        .prepare(&shell_demand("demand:resident", Some(6)))
        .unwrap();

    // Simulate host restart: a fresh composition over the same state root,
    // same budget. The durable ledger must carry the reservation (the
    // provenance-based rebuild runs through register_world in the durable
    // host; the file itself survives the process).
    let ledger = AdmissionLedger::load(&state).unwrap();
    assert!(
        ledger.reservation_for("demand:resident").is_some(),
        "the reservation is durable: {:?}",
        ledger.as_json()
    );
    assert!(
        std::path::Path::new(&state).join(ADMISSION_FILE).exists(),
        "the ledger file is persisted at the state root"
    );

    // With the resident world holding 6 of 8 MiB, a 5 MiB floor waits.
    let mut restarted = CollapsedLocalWorkcell::new(
        CollapsedLocalConfig::new(
            WorkcellRef::new("workcell:capacity").unwrap(),
            state.clone(),
        )
        .with_capacity_budget(budget),
    )
    .unwrap();
    restarted.register_world(world.clone()).unwrap();
    let waiting = restarted.prepare(&shell_demand("demand:latecomer", Some(5)));
    assert!(
        matches!(&waiting, Err(WorkcellError::Capacity(_))),
        "a re-entered world keeps occupying its budget: {waiting:?}"
    );

    let _ = std::fs::remove_dir_all(&state);
}

#[test]
fn reconcile_retires_reservations_of_verifiably_absent_worlds() {
    let state = tempfile_root("reconcile");
    let budget = fixed_budget(8, 1);
    let mut workcell = CollapsedLocalWorkcell::new(
        CollapsedLocalConfig::new(
            WorkcellRef::new("workcell:capacity").unwrap(),
            state.clone(),
        )
        .with_capacity_budget(budget),
    )
    .unwrap();
    let world = workcell
        .prepare(&shell_demand("demand:vanishing", Some(6)))
        .unwrap();
    // Simulate a world lost without release (provider reap, crash): the
    // control plane's registration is dropped directly.
    workcell.control_forget_world(&world.world_ref);
    let result = workcell
        .reconcile(&[])
        .expect("reconciliation runs against empty desired state");
    let _ = result;
    let ledger = AdmissionLedger::load(&state).unwrap();
    assert!(
        ledger.reservation_for("demand:vanishing").is_none(),
        "a verifiably absent world must not hold budget: {:?}",
        ledger.as_json()
    );
    let _ = std::fs::remove_dir_all(&state);
}

#[test]
fn an_unsatisfiable_plan_is_refused_before_admission() {
    let state = tempfile_root("unsatisfiable");
    let budget = fixed_budget(8, 1);
    let mut workcell = CollapsedLocalWorkcell::new(
        CollapsedLocalConfig::new(
            WorkcellRef::new("workcell:capacity").unwrap(),
            state.clone(),
        )
        .with_capacity_budget(budget),
    )
    .unwrap();
    let mut demand = shell_demand("demand:nobody", Some(1));
    demand
        .affordances
        .required
        .push(AffordanceRequirement::new("specimen.nobody-offers").unwrap());
    let error = workcell.prepare(&demand).unwrap_err();
    assert!(
        error.to_string().contains("cannot satisfy required demand"),
        "planning refusal precedes admission: {error}"
    );
    // Required placement is not rerouted: the plan itself is unsatisfiable.
    let plan = workcell.plan(&demand).unwrap();
    assert_eq!(plan.status, PlanStatus::Unsatisfiable);
    let _ = std::fs::remove_dir_all(&state);
}

#[test]
fn a_declared_ceiling_reaches_the_readback_and_admission_uses_the_floor() {
    // Minimum and maximum are distinct: admission commits the floor; the
    // ceiling is what a provider may enforce. A demand with a large ceiling
    // but small floor admits against the floor.
    let state = tempfile_root("ceiling");
    let budget = fixed_budget(8, 1);
    let mut workcell = CollapsedLocalWorkcell::new(
        CollapsedLocalConfig::new(
            WorkcellRef::new("workcell:capacity").unwrap(),
            state.clone(),
        )
        .with_capacity_budget(budget),
    )
    .unwrap();
    let mut demand = shell_demand("demand:ceiling", None);
    demand.resources.push(ResourceRequirement {
        key: "memory".into(),
        minimum: Some(4),
        maximum: Some(64),
        unit: Some("MiB".into()),
    });
    let world = workcell.prepare(&demand).expect("the floor fits");
    let ledger = AdmissionLedger::load(&state).unwrap();
    let reservation = ledger.reservation_for("demand:ceiling").unwrap();
    assert_eq!(
        reservation.resources.memory_minimum_bytes,
        Some(4 * 1024 * 1024),
        "the floor is what is committed"
    );
    assert_eq!(
        reservation.resources.memory_maximum_bytes,
        Some(64 * 1024 * 1024),
        "the ceiling is recorded for the provider to enforce"
    );
    workcell.release(&world.world_ref).unwrap();
    let _ = std::fs::remove_dir_all(&state);
}
