use std::collections::{BTreeMap, BTreeSet};

use epilogos_workcell_core::{
    AffordanceRequirement, Availability, Capacity, DemandRef, Discovery, ExecutionDemand,
    ExternalRef, HealthState, OfferRef, OperationalOffer, ProviderRef, ResourceRequirement,
    WorkcellError, WorkcellRef,
};
use epilogos_workcell_placement::{
    evaluate_placement, select_placement, PlacementDiagnosticKind, PlacementPolicy,
    WorkcellDiscoverySource,
};

#[derive(Clone)]
struct FakeSource {
    source_ref: String,
    hint: WorkcellRef,
    locality_cost: u32,
    tags: BTreeSet<String>,
    discovery: Discovery,
    transport_available: bool,
    transport_kind: String,
}

impl FakeSource {
    fn new(
        source_ref: &str,
        workcell_ref: &str,
        provider_ref: &str,
        offer_ref: &str,
        cpu: u64,
        locality_cost: u32,
    ) -> Self {
        let workcell_ref = WorkcellRef::new(workcell_ref).unwrap();
        let capacity = BTreeMap::from([(
            "cpu".into(),
            Capacity {
                amount: cpu,
                unit: Some("cores".into()),
            },
        )]);
        Self {
            source_ref: source_ref.into(),
            hint: workcell_ref.clone(),
            locality_cost,
            tags: BTreeSet::new(),
            discovery: Discovery {
                workcell_ref,
                health: HealthState::Healthy,
                capacity: capacity.clone(),
                offers: vec![OperationalOffer {
                    offer_ref: OfferRef::new(offer_ref).unwrap(),
                    provider_ref: ProviderRef::new(provider_ref).unwrap(),
                    port: "execution".into(),
                    affordances: vec!["shell".into()],
                    connections: vec![],
                    exposures: vec![],
                    isolation_trust: vec![],
                    availability: Availability::Available,
                    health: HealthState::Healthy,
                    capacity,
                    metadata: BTreeMap::new(),
                }],
            },
            transport_available: true,
            transport_kind: "fixture".into(),
        }
    }

    fn with_tag(mut self, tag: &str) -> Self {
        self.tags.insert(tag.into());
        self
    }

    fn unavailable_transport(mut self) -> Self {
        self.transport_available = false;
        self
    }

    fn unavailable_workcell(mut self) -> Self {
        self.discovery.health = HealthState::Unavailable;
        self
    }
}

impl WorkcellDiscoverySource for FakeSource {
    fn source_ref(&self) -> &str {
        &self.source_ref
    }

    fn workcell_hint(&self) -> Option<&WorkcellRef> {
        Some(&self.hint)
    }

    fn locality_cost(&self) -> u32 {
        self.locality_cost
    }

    fn policy_tags(&self) -> BTreeSet<String> {
        self.tags.clone()
    }

    fn transport_provenance(&self) -> BTreeMap<String, String> {
        BTreeMap::from([
            ("transport_source".into(), self.source_ref.clone()),
            ("transport_kind".into(), self.transport_kind.clone()),
        ])
    }

    fn discover(&self) -> epilogos_workcell_core::Result<Discovery> {
        if !self.transport_available {
            return Err(WorkcellError::Unavailable(format!(
                "transport `{}` cannot reach `{}`",
                self.source_ref, self.hint
            )));
        }
        Ok(self.discovery.clone())
    }
}

fn semantic_demand() -> ExecutionDemand {
    let mut demand = ExecutionDemand::new(DemandRef::new("demand:multi-workcell").unwrap());
    demand.subjects.insert(
        "project".into(),
        ExternalRef::new("factory:project:factory").unwrap(),
    );
    demand
        .subjects
        .insert("run".into(), ExternalRef::new("factory:run:r-16").unwrap());
    demand.subjects.insert(
        "candidate".into(),
        ExternalRef::new("factory:candidate:c-16").unwrap(),
    );
    demand.subjects.insert(
        "agent".into(),
        ExternalRef::new("factory:agent:builder").unwrap(),
    );
    demand
        .affordances
        .required
        .push(AffordanceRequirement::new("shell").unwrap());
    demand.resources.push(ResourceRequirement {
        key: "cpu".into(),
        minimum: Some(2),
        unit: Some("cores".into()),
    });
    demand
}

#[test]
fn capacity_drives_placement_when_policy_does_not_prefer_locality() {
    let demand = semantic_demand();
    let smaller = FakeSource::new(
        "source:small",
        "workcell:small",
        "provider:small",
        "offer:small",
        4,
        1,
    );
    let larger = FakeSource::new(
        "source:large",
        "workcell:large",
        "provider:large",
        "offer:large",
        16,
        5,
    );
    let sources: [&dyn WorkcellDiscoverySource; 2] = [&smaller, &larger];
    let policy = PlacementPolicy {
        require_declared_aggregate_capacity: true,
        ..PlacementPolicy::default()
    };

    let selected = select_placement(&demand, &sources, &policy, None).unwrap();
    assert_eq!(selected.workcell_ref.as_str(), "workcell:large");
    assert_eq!(selected.provenance.capacity_headroom.get("cpu"), Some(&14));
    assert_eq!(
        selected.provenance.provider_refs,
        vec!["provider:large".to_string()]
    );
    assert_eq!(
        demand.subjects.get("candidate").map(|item| item.as_str()),
        Some("factory:candidate:c-16")
    );
}

#[test]
fn locality_and_policy_can_outweigh_capacity_without_host_names() {
    let demand = semantic_demand();
    let near = FakeSource::new(
        "source:near",
        "workcell:near",
        "provider:near",
        "offer:near",
        4,
        0,
    )
    .with_tag("trusted-zone");
    let far = FakeSource::new(
        "source:far",
        "workcell:far",
        "provider:far",
        "offer:far",
        64,
        20,
    )
    .with_tag("trusted-zone");
    let sources: [&dyn WorkcellDiscoverySource; 2] = [&near, &far];
    let policy = PlacementPolicy {
        required_tags: BTreeSet::from(["trusted-zone".into()]),
        max_locality_cost: Some(30),
        prefer_locality_over_capacity: true,
        require_declared_aggregate_capacity: true,
    };
    let selected = select_placement(&demand, &sources, &policy, None).unwrap();
    assert_eq!(selected.workcell_ref.as_str(), "workcell:near");
    assert_eq!(selected.provenance.locality_cost, 0);
}

#[test]
fn remote_workcell_loss_is_structured_and_does_not_mutate_semantic_refs() {
    let demand = semantic_demand();
    let semantic_before = demand.subjects.clone();
    let local = FakeSource::new(
        "source:local",
        "workcell:local",
        "provider:local",
        "offer:local",
        4,
        0,
    );
    let remote = FakeSource::new(
        "source:remote",
        "workcell:remote",
        "provider:remote",
        "offer:remote",
        32,
        10,
    )
    .unavailable_transport();
    let sources: [&dyn WorkcellDiscoverySource; 2] = [&local, &remote];

    let evaluation =
        evaluate_placement(&demand, &sources, &PlacementPolicy::default(), None).unwrap();
    assert_eq!(
        evaluation.selected().unwrap().workcell_ref.as_str(),
        "workcell:local"
    );
    let diagnostic = evaluation
        .diagnostics
        .iter()
        .find(|diagnostic| diagnostic.source_ref == "source:remote")
        .unwrap();
    assert_eq!(
        diagnostic.kind,
        PlacementDiagnosticKind::TransportUnavailable
    );
    assert_eq!(
        diagnostic.workcell_ref.as_ref().map(|item| item.as_str()),
        Some("workcell:remote")
    );
    assert_eq!(demand.subjects, semantic_before);
}

#[test]
fn discovered_unavailable_workcell_is_distinct_from_transport_loss() {
    let demand = semantic_demand();
    let remote = FakeSource::new(
        "source:remote-health",
        "workcell:remote-health",
        "provider:remote-health",
        "offer:remote-health",
        32,
        10,
    )
    .unavailable_workcell();
    let sources: [&dyn WorkcellDiscoverySource; 1] = [&remote];
    let evaluation =
        evaluate_placement(&demand, &sources, &PlacementPolicy::default(), None).unwrap();
    assert!(evaluation.eligible.is_empty());
    assert_eq!(
        evaluation.diagnostics[0].kind,
        PlacementDiagnosticKind::WorkcellUnavailable
    );
}

#[test]
fn re_placement_substitutes_workcell_and_provider_but_retains_semantic_subjects_and_provenance() {
    let demand = semantic_demand();
    let semantic_before = demand.subjects.clone();
    let first_source = FakeSource::new(
        "source:first",
        "workcell:first",
        "provider:first",
        "offer:first",
        8,
        0,
    );
    let first_sources: [&dyn WorkcellDiscoverySource; 1] = [&first_source];
    let first =
        select_placement(&demand, &first_sources, &PlacementPolicy::default(), None).unwrap();
    assert_eq!(first.workcell_ref.as_str(), "workcell:first");

    let lost_first = first_source.clone().unavailable_transport();
    let replacement = FakeSource::new(
        "source:replacement",
        "workcell:replacement",
        "provider:replacement",
        "offer:replacement",
        12,
        4,
    );
    let replacement_sources: [&dyn WorkcellDiscoverySource; 2] = [&lost_first, &replacement];
    let second = select_placement(
        &demand,
        &replacement_sources,
        &PlacementPolicy::default(),
        Some(&first.workcell_ref),
    )
    .unwrap();

    assert_eq!(second.workcell_ref.as_str(), "workcell:replacement");
    assert!(second.provenance.placement_changed);
    assert_eq!(
        second
            .provenance
            .previous_workcell_ref
            .as_ref()
            .map(|item| item.as_str()),
        Some("workcell:first")
    );
    assert_eq!(
        second.provenance.provider_refs,
        vec!["provider:replacement".to_string()]
    );
    assert_eq!(
        second
            .provenance
            .transport
            .get("transport_source")
            .map(String::as_str),
        Some("source:replacement")
    );
    assert_eq!(demand.subjects, semantic_before);
}

#[test]
fn placement_source_contract_contains_no_transport_or_cluster_ontology() {
    let source = include_str!("../src/lib.rs").to_ascii_lowercase();
    assert!(!source.contains("kubernetes"));
    assert!(!source.contains("cluster"));
    assert!(!source.contains("ssh"));
    assert!(!source.contains("http://"));
    assert!(!source.contains("hostname"));
    assert!(!source.contains("password"));
    assert!(!source.contains("bearer"));
}
