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
        diff_provider_inventory, verify_provider_port, verify_sdk_contract_version,
        verify_sdk_contract_version_against, ExecutionFault, FaultingExecutionProvider,
    },
    WORKCELL_SDK_VERSION,
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
fn conformance_admits_a_compatible_sdk_contract_version() {
    // Positive control: this runtime's own contract version admits itself.
    let own = verify_sdk_contract_version(WORKCELL_SDK_VERSION).unwrap();
    assert_eq!(own.declared_version, WORKCELL_SDK_VERSION);
    assert_eq!(own.runtime_version, WORKCELL_SDK_VERSION);
    assert!(own.summary().contains("compatible"));

    // Zero-padded equivalence: a provider declaring the short form (`1.0`)
    // names the same contract as a runtime that writes `1.0.0`. A comparator
    // that compares strings instead of zero-padded numbers refuses this
    // honest envelope — the defect class the suite's owner fixed elsewhere
    // in the org — so the admission must prove the padding itself.
    let short = verify_sdk_contract_version_against("1.0.0", "1.0").unwrap();
    assert_eq!(short.declared_version, "1.0");
    assert_eq!(short.runtime_version, "1.0.0");
    let long = verify_sdk_contract_version_against("1.0", "1.0.0").unwrap();
    assert_eq!(long.declared_version, "1.0.0");

    // An older provider inside the same major admits: the runtime is the
    // newer end of the contract, and a provider built against an older SDK
    // uses only surfaces the runtime still carries.
    verify_sdk_contract_version_against("1.2.0", "1.1.9").unwrap();
}

#[test]
fn conformance_rejects_an_incompatible_sdk_contract_version_by_name() {
    // A major bump is a contract break: refused whatever the minor says,
    // with a result that names both versions and the incompatibility.
    let error = verify_sdk_contract_version_against("1.0.0", "2.0.0").unwrap_err();
    let message = format!("{error}");
    assert!(
        message.contains("2.0.0") && message.contains("1.0.0") && message.contains("incompatible"),
        "a major-version mismatch must be refused by name, got: {message}"
    );

    // A provider built against a NEWER SDK than the runtime may rely on
    // surfaces the runtime does not carry — refused by name too.
    let error = verify_sdk_contract_version_against("1.0.0", "1.1.0").unwrap_err();
    assert!(
        format!("{error}").contains("incompatible"),
        "a newer-declared minor must be refused, got: {error}"
    );

    // Numeric discipline: `1.0.10` is NEWER than `1.0.9`. A lexicographic
    // comparator orders it older and would wrongly admit it; the gate must
    // refuse it as the incompatible newer contract it is.
    let error = verify_sdk_contract_version_against("1.0.9", "1.0.10").unwrap_err();
    let message = format!("{error}");
    assert!(
        message.contains("1.0.10") && message.contains("1.0.9") && message.contains("incompatible"),
        "an un-padded comparator would admit `1.0.10` against `1.0.9`; got: {message}"
    );

    // A version that cannot be declared at all is its own named refusal,
    // never a silent admission.
    for garbage in ["", "abc", "1.x.0", "1.0.0.1", "1..0"] {
        let error = verify_sdk_contract_version_against("1.0.0", garbage).unwrap_err();
        let message = format!("{error}");
        assert!(
            message.contains("not a parsable") && message.contains("major.minor.patch"),
            "garbage version `{garbage}` must carry its own named refusal, got: {message}"
        );
    }
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
