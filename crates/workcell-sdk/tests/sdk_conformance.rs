use std::collections::BTreeMap;

use epilogos_workcell_sdk::{
    client::{ControlClient, ControlClientError, UnavailableTransport},
    contract::{
        DemandRef, ExecutionDemand, ExternalRef, ResourceRequirement, RetentionExpectation,
    },
    provider::{
        Availability, ExecutionMaterialRequest, ExecutionProvider, HealthState, OfferRef,
        OperationalOffer, ProviderOperation, ProviderPort, ProviderPortKind, ProviderRef,
    },
    testkit::{
        diff_provider_inventory, verify_provider_port, ExecutionFault, FaultingExecutionProvider,
    },
};

struct ExternalStyleProvider {
    provider_ref: ProviderRef,
    offered_ref: ProviderRef,
}

impl ExternalStyleProvider {
    fn valid() -> Self {
        let provider_ref = ProviderRef::new("provider:example/external").unwrap();
        Self {
            offered_ref: provider_ref.clone(),
            provider_ref,
        }
    }
}

impl ProviderPort for ExternalStyleProvider {
    fn provider_ref(&self) -> &ProviderRef {
        &self.provider_ref
    }

    fn port_kind(&self) -> ProviderPortKind {
        ProviderPortKind::Execution
    }

    fn offers(&self) -> epilogos_workcell_sdk::provider::Result<Vec<OperationalOffer>> {
        Ok(vec![OperationalOffer {
            offer_ref: OfferRef::new("offer:example/external-execution").unwrap(),
            provider_ref: self.offered_ref.clone(),
            port: "execution".into(),
            affordances: vec!["shell".into()],
            connections: vec!["internet".into()],
            exposures: Vec::new(),
            isolation_trust: Vec::new(),
            availability: Availability::Available,
            health: HealthState::Healthy,
            capacity: BTreeMap::new(),
            metadata: BTreeMap::new(),
        }])
    }
}

#[test]
fn external_provider_can_conform_using_only_the_sdk_facade() {
    let provider = ExternalStyleProvider::valid();
    let report = verify_provider_port(&provider).unwrap();
    assert_eq!(report.provider_ref.as_str(), "provider:example/external");
    assert_eq!(report.port, ProviderPortKind::Execution);
    assert_eq!(report.offer_count, 1);
    assert_eq!(report.available_offers, 1);
    assert!(report.summary().contains("1 available"));
}

#[test]
fn conformance_rejects_provider_identity_drift() {
    let mut provider = ExternalStyleProvider::valid();
    provider.offered_ref = ProviderRef::new("provider:other").unwrap();
    assert!(verify_provider_port(&provider).is_err());
}

#[test]
fn provider_removal_and_replacement_are_inventory_changes_not_identity_rewrites() {
    let original = verify_provider_port(&ExternalStyleProvider::valid()).unwrap();
    let replacement =
        FaultingExecutionProvider::new(ProviderRef::new("provider:example/replacement").unwrap());
    let replacement = verify_provider_port(&replacement).unwrap();

    let delta = diff_provider_inventory(&[original], &[replacement]);
    assert_eq!(delta.removed[0].as_str(), "provider:example/external");
    assert_eq!(delta.added[0].as_str(), "provider:example/replacement");
    assert!(delta.retained.is_empty());
}

#[test]
fn public_fault_fixture_covers_degraded_offer_and_partial_lifecycle_failure() {
    let degraded =
        FaultingExecutionProvider::new(ProviderRef::new("provider:fixture/degraded").unwrap())
            .with_availability(Availability::Degraded, HealthState::Degraded);
    let report = verify_provider_port(&degraded).unwrap();
    assert_eq!(report.degraded_offers, 1);

    let mut partial =
        FaultingExecutionProvider::new(ProviderRef::new("provider:fixture/partial").unwrap())
            .with_fault(ExecutionFault::Execute);
    let request = ExecutionMaterialRequest {
        demand_ref: DemandRef::new("demand:sdk-fault").unwrap(),
        affordances: vec!["shell".into()],
        resources: Vec::new(),
        connectivity: Vec::new(),
        isolation_trust: None,
        retention: RetentionExpectation::Release,
    };
    let allocation = partial.prepare_execution(&request).unwrap();
    let operation = ProviderOperation {
        key: "fixture-operation".into(),
        parameters: BTreeMap::new(),
    };
    assert!(partial.execute_operation(&allocation, &operation).is_err());
    assert_eq!(
        partial.observe_execution(&allocation).unwrap().health,
        HealthState::Healthy
    );
    assert!(
        partial
            .release_execution(&allocation, &RetentionExpectation::Release)
            .unwrap()
            .changed
    );
}

#[test]
fn client_sdk_preserves_transport_unavailability_as_a_distinct_failure() {
    let mut client = ControlClient::new(UnavailableTransport);
    assert!(matches!(
        client.status(),
        Err(ControlClientError::TransportUnavailable(_))
    ));
}

fn model_serving_demand(engine: &str, placement: &str) -> ExecutionDemand {
    let mut demand =
        ExecutionDemand::new(DemandRef::new(format!("demand:model:{engine}")).unwrap())
            .with_subject(
                "model",
                ExternalRef::new("model:qwen2.5-coder-32b").unwrap(),
            )
            .with_subject("variant", ExternalRef::new("variant:q4-k-m").unwrap());
    demand.resources.push(ResourceRequirement {
        key: "accelerator".into(),
        minimum: Some(1),
        unit: Some("device".into()),
    });
    demand
        .extensions
        .insert("inference-engine".into(), engine.into());
    demand
        .extensions
        .insert("placement".into(), placement.into());
    demand
}

#[test]
fn model_serving_conformance_uses_ordinary_opaque_material_demands() {
    let ollama = model_serving_demand("ollama", "local");
    let llama_cpp = model_serving_demand("llama.cpp", "local");
    let vllm = model_serving_demand("vllm", "remote");

    for demand in [&ollama, &llama_cpp, &vllm] {
        demand.validate().unwrap();
        assert_eq!(demand.subjects["model"].as_str(), "model:qwen2.5-coder-32b");
        assert_eq!(demand.resources[0].key, "accelerator");
    }

    assert_eq!(ollama.extensions["placement"], "local");
    assert_eq!(vllm.extensions["placement"], "remote");
    assert_eq!(llama_cpp.extensions["inference-engine"], "llama.cpp");

    // There is no Workcell-owned ModelServer/LocalModelProvider identity in
    // this proof: model/variant remain opaque caller refs and engine/placement
    // are provider-materialisation facts on an ordinary ExecutionDemand.
}
