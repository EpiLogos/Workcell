use std::{cmp::Ordering, collections::{BTreeMap, BTreeSet}};

use epilogos_workcell_core::{
    plan, Capacity, Discovery, ExecutionDemand, HealthState, MaterialisationPlan, PlanStatus, Result,
    WorkcellError, WorkcellRef,
};

/// Replaceable discovery/transport seam for one reachable Workcell.
///
/// Implementations may be in-process, RPC, SSH-backed, HTTP, test doubles, or
/// future transports. Placement sees only the canonical Workcell `Discovery`
/// result plus opaque locality/policy metadata; transport/auth details remain
/// outside the semantic Workcell contract.
pub trait WorkcellDiscoverySource: Send + Sync {
    fn source_ref(&self) -> &str;
    fn workcell_hint(&self) -> Option<&WorkcellRef>;
    fn locality_cost(&self) -> u32;
    fn policy_tags(&self) -> BTreeSet<String> {
        BTreeSet::new()
    }
    fn transport_provenance(&self) -> BTreeMap<String, String> {
        BTreeMap::new()
    }
    fn discover(&self) -> Result<Discovery>;
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PlacementPolicy {
    pub required_tags: BTreeSet<String>,
    /// Reject sources farther away than this opaque locality cost.
    pub max_locality_cost: Option<u32>,
    /// When true locality sorts ahead of spare aggregate capacity; otherwise
    /// capacity sorts ahead of locality.
    pub prefer_locality_over_capacity: bool,
    /// When true, a resource requirement with a minimum must have a compatible
    /// Workcell-wide aggregate capacity entry. Provider offer capacity is still
    /// checked independently by the core planner.
    pub require_declared_aggregate_capacity: bool,
}

impl Default for PlacementPolicy {
    fn default() -> Self {
        Self {
            required_tags: BTreeSet::new(),
            max_locality_cost: None,
            prefer_locality_over_capacity: false,
            require_declared_aggregate_capacity: false,
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum PlacementDiagnosticKind {
    TransportUnavailable,
    WorkcellUnavailable,
    PolicyRejected,
    CapacityRejected,
    Unsatisfiable,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PlacementDiagnostic {
    pub source_ref: String,
    pub workcell_ref: Option<WorkcellRef>,
    pub kind: PlacementDiagnosticKind,
    pub detail: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PlacementProvenance {
    pub source_ref: String,
    pub workcell_ref: WorkcellRef,
    pub previous_workcell_ref: Option<WorkcellRef>,
    pub placement_changed: bool,
    pub locality_cost: u32,
    pub capacity_headroom: BTreeMap<String, u64>,
    pub provider_refs: Vec<String>,
    pub offer_refs: Vec<String>,
    pub transport: BTreeMap<String, String>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PlacementDecision {
    pub workcell_ref: WorkcellRef,
    pub plan: MaterialisationPlan,
    pub provenance: PlacementProvenance,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct PlacementEvaluation {
    pub eligible: Vec<PlacementDecision>,
    pub diagnostics: Vec<PlacementDiagnostic>,
}

impl PlacementEvaluation {
    pub fn selected(&self) -> Option<&PlacementDecision> {
        self.eligible.first()
    }

    pub fn into_selected(self) -> Result<PlacementDecision> {
        self.eligible.into_iter().next().ok_or_else(|| {
            let detail = self
                .diagnostics
                .iter()
                .map(|diagnostic| {
                    format!(
                        "{}:{:?}:{}",
                        diagnostic.source_ref, diagnostic.kind, diagnostic.detail
                    )
                })
                .collect::<Vec<_>>()
                .join("; ");
            WorkcellError::UnsatisfiedDemand(if detail.is_empty() {
                "no Workcell placement candidates were supplied".into()
            } else {
                format!("no Workcell can satisfy placement: {detail}")
            })
        })
    }
}

pub fn evaluate_placement(
    demand: &ExecutionDemand,
    sources: &[&dyn WorkcellDiscoverySource],
    policy: &PlacementPolicy,
    previous_workcell_ref: Option<&WorkcellRef>,
) -> Result<PlacementEvaluation> {
    demand.validate()?;
    let mut evaluation = PlacementEvaluation::default();

    for source in sources {
        if source.source_ref().trim().is_empty() {
            return Err(WorkcellError::InvalidDemand(
                "Workcell discovery source_ref must not be empty".into(),
            ));
        }
        if let Some(maximum) = policy.max_locality_cost {
            if source.locality_cost() > maximum {
                evaluation.diagnostics.push(PlacementDiagnostic {
                    source_ref: source.source_ref().into(),
                    workcell_ref: source.workcell_hint().cloned(),
                    kind: PlacementDiagnosticKind::PolicyRejected,
                    detail: format!(
                        "locality cost {} exceeds policy maximum {maximum}",
                        source.locality_cost()
                    ),
                });
                continue;
            }
        }
        let tags = source.policy_tags();
        if !policy.required_tags.is_subset(&tags) {
            let missing = policy
                .required_tags
                .difference(&tags)
                .cloned()
                .collect::<Vec<_>>()
                .join(",");
            evaluation.diagnostics.push(PlacementDiagnostic {
                source_ref: source.source_ref().into(),
                workcell_ref: source.workcell_hint().cloned(),
                kind: PlacementDiagnosticKind::PolicyRejected,
                detail: format!("missing required placement policy tags: {missing}"),
            });
            continue;
        }

        let discovery = match source.discover() {
            Ok(discovery) => discovery,
            Err(error) => {
                evaluation.diagnostics.push(PlacementDiagnostic {
                    source_ref: source.source_ref().into(),
                    workcell_ref: source.workcell_hint().cloned(),
                    kind: PlacementDiagnosticKind::TransportUnavailable,
                    detail: error.to_string(),
                });
                continue;
            }
        };
        if matches!(discovery.health, HealthState::Unavailable) {
            evaluation.diagnostics.push(PlacementDiagnostic {
                source_ref: source.source_ref().into(),
                workcell_ref: Some(discovery.workcell_ref.clone()),
                kind: PlacementDiagnosticKind::WorkcellUnavailable,
                detail: "Workcell discovery reports unavailable health".into(),
            });
            continue;
        }

        let capacity_headroom = match aggregate_capacity_headroom(demand, &discovery.capacity) {
            Ok(headroom) => headroom,
            Err(detail) if policy.require_declared_aggregate_capacity => {
                evaluation.diagnostics.push(PlacementDiagnostic {
                    source_ref: source.source_ref().into(),
                    workcell_ref: Some(discovery.workcell_ref.clone()),
                    kind: PlacementDiagnosticKind::CapacityRejected,
                    detail,
                });
                continue;
            }
            Err(_) => BTreeMap::new(),
        };

        let material_plan = plan(demand, &discovery)?;
        if material_plan.status == PlanStatus::Unsatisfiable {
            evaluation.diagnostics.push(PlacementDiagnostic {
                source_ref: source.source_ref().into(),
                workcell_ref: Some(discovery.workcell_ref.clone()),
                kind: PlacementDiagnosticKind::Unsatisfiable,
                detail: material_plan.explanation.join("; "),
            });
            continue;
        }

        let mut provider_refs = material_plan
            .planned_bindings
            .iter()
            .map(|binding| binding.provider_ref.to_string())
            .chain(
                material_plan
                    .planned_exposures
                    .iter()
                    .map(|exposure| exposure.provider_ref.to_string()),
            )
            .chain(
                material_plan
                    .planned_constraints
                    .iter()
                    .map(|constraint| constraint.provider_ref.to_string()),
            )
            .collect::<BTreeSet<_>>()
            .into_iter()
            .collect::<Vec<_>>();
        provider_refs.sort();
        let mut offer_refs = material_plan
            .planned_bindings
            .iter()
            .map(|binding| binding.offer_ref.to_string())
            .chain(
                material_plan
                    .planned_exposures
                    .iter()
                    .map(|exposure| exposure.offer_ref.to_string()),
            )
            .chain(
                material_plan
                    .planned_constraints
                    .iter()
                    .map(|constraint| constraint.offer_ref.to_string()),
            )
            .collect::<BTreeSet<_>>()
            .into_iter()
            .collect::<Vec<_>>();
        offer_refs.sort();
        let previous = previous_workcell_ref.cloned();
        let changed = previous
            .as_ref()
            .is_some_and(|previous| previous != &discovery.workcell_ref);
        evaluation.eligible.push(PlacementDecision {
            workcell_ref: discovery.workcell_ref.clone(),
            plan: material_plan,
            provenance: PlacementProvenance {
                source_ref: source.source_ref().into(),
                workcell_ref: discovery.workcell_ref,
                previous_workcell_ref: previous,
                placement_changed: changed,
                locality_cost: source.locality_cost(),
                capacity_headroom,
                provider_refs,
                offer_refs,
                transport: source.transport_provenance(),
            },
        });
    }

    evaluation.eligible.sort_by(|left, right| {
        compare_decisions(left, right, policy).then_with(|| left.workcell_ref.cmp(&right.workcell_ref))
    });
    Ok(evaluation)
}

pub fn select_placement(
    demand: &ExecutionDemand,
    sources: &[&dyn WorkcellDiscoverySource],
    policy: &PlacementPolicy,
    previous_workcell_ref: Option<&WorkcellRef>,
) -> Result<PlacementDecision> {
    evaluate_placement(demand, sources, policy, previous_workcell_ref)?.into_selected()
}

fn compare_decisions(
    left: &PlacementDecision,
    right: &PlacementDecision,
    policy: &PlacementPolicy,
) -> Ordering {
    let plan_order = plan_rank(right.plan.status.clone()).cmp(&plan_rank(left.plan.status.clone()));
    if plan_order != Ordering::Equal {
        return plan_order;
    }

    let capacity = total_headroom(&right.provenance.capacity_headroom)
        .cmp(&total_headroom(&left.provenance.capacity_headroom));
    let locality = left
        .provenance
        .locality_cost
        .cmp(&right.provenance.locality_cost);
    if policy.prefer_locality_over_capacity {
        locality.then(capacity)
    } else {
        capacity.then(locality)
    }
}

fn plan_rank(status: PlanStatus) -> u8 {
    match status {
        PlanStatus::Satisfiable => 2,
        PlanStatus::Degraded => 1,
        PlanStatus::Unsatisfiable => 0,
    }
}

fn total_headroom(headroom: &BTreeMap<String, u64>) -> u128 {
    headroom.values().map(|value| u128::from(*value)).sum()
}

fn aggregate_capacity_headroom(
    demand: &ExecutionDemand,
    capacity: &BTreeMap<String, Capacity>,
) -> std::result::Result<BTreeMap<String, u64>, String> {
    let mut headroom = BTreeMap::new();
    for requirement in &demand.resources {
        let Some(minimum) = requirement.minimum else {
            continue;
        };
        let available = capacity.get(&requirement.key).ok_or_else(|| {
            format!(
                "aggregate Workcell capacity does not declare `{}`",
                requirement.key
            )
        })?;
        if let (Some(required_unit), Some(available_unit)) = (&requirement.unit, &available.unit) {
            if !required_unit.eq_ignore_ascii_case(available_unit) {
                return Err(format!(
                    "aggregate capacity unit mismatch for `{}`: demand `{required_unit}`, Workcell `{available_unit}`",
                    requirement.key
                ));
            }
        }
        if available.amount < minimum {
            return Err(format!(
                "aggregate Workcell capacity for `{}` is {} but demand requires at least {minimum}",
                requirement.key, available.amount
            ));
        }
        headroom.insert(requirement.key.clone(), available.amount - minimum);
    }
    Ok(headroom)
}
