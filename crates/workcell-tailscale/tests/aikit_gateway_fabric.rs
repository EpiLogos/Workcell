use epilogos_workcell_sdk::{
    contract::{PlanStatus, RequirementNecessity, WorkcellRef},
    fabric::{
        evaluate_fabric, FabricPathOffer, FabricPathProvider, NetworkRelationship, NetworkSecurity,
        ReachabilityScope, RequiredNetworkRelationship,
    },
    provider::{ProviderRef, Result},
};
use epilogos_workcell_tailscale::{
    tailscale_path_from_status_json, TailscalePathConfig, TailscalePeerSelector,
    TailscalePolicyEvidence,
};

const DIRECT_STATUS: &str = r#"{
  "BackendState": "Running",
  "Peer": {
    "nodekey:gateway": {
      "ID": "node-stable-id-material-only",
      "HostName": "gateway-host",
      "DNSName": "gateway-host.example.ts.net.",
      "TailscaleIPs": ["100.64.0.42"],
      "CurAddr": "192.0.2.42:41641",
      "Relay": "",
      "PeerRelay": "",
      "Online": true,
      "Active": true
    }
  }
}"#;

struct FixturePathProvider {
    provider_ref: ProviderRef,
    path: FabricPathOffer,
}

impl FabricPathProvider for FixturePathProvider {
    fn provider_ref(&self) -> &ProviderRef {
        &self.provider_ref
    }

    fn paths(&self) -> Result<Vec<FabricPathOffer>> {
        Ok(vec![self.path.clone()])
    }
}

#[test]
fn stable_gateway_service_relationship_can_be_realised_by_tailscale_without_identity_collapse() {
    let source = WorkcellRef::new("workcell:desktop").unwrap();
    let destination = WorkcellRef::new("workcell:server").unwrap();
    let provider_ref = ProviderRef::new("provider:tailscale-reference").unwrap();
    let path = tailscale_path_from_status_json(
        &TailscalePathConfig {
            provider_ref: provider_ref.clone(),
            path_ref: "path:tailscale:gateway-host".into(),
            source_workcell: source.clone(),
            destination_workcell: destination.clone(),
            peer: TailscalePeerSelector::new("gateway-host.example.ts.net").unwrap(),
            endpoint: "gateway-host.example.ts.net:7778".into(),
            transport: Some("websocket".into()),
            policy: TailscalePolicyEvidence::Allowed {
                evidence: "fixture grant permits desktop → gateway service".into(),
            },
        },
        DIRECT_STATUS,
    )
    .unwrap();
    let provider = FixturePathProvider {
        provider_ref,
        path,
    };

    let relationship_ref = "network:agency-gateway/personal-world";
    let logical_service = "service:agency-gateway/personal-world";
    let relationship = RequiredNetworkRelationship::between_workcells(
        NetworkRelationship::new(relationship_ref, "surface:desktop", logical_service)
            .unwrap()
            .with_transport("websocket")
            .unwrap()
            .with_scope(ReachabilityScope::Private)
            .with_security(NetworkSecurity::AuthenticatedEncrypted),
        RequirementNecessity::Required,
        source,
        destination,
    );

    let plan = evaluate_fabric(&[relationship], &[&provider]).unwrap();
    assert_eq!(plan.status, PlanStatus::Satisfiable);
    assert_eq!(plan.bindings.len(), 1);
    let binding = &plan.bindings[0];
    assert_eq!(binding.relationship_ref, relationship_ref);
    assert_eq!(binding.transport.as_deref(), Some("websocket"));
    assert_eq!(binding.scope, ReachabilityScope::Private);
    assert_eq!(binding.security, NetworkSecurity::AuthenticatedEncrypted);
    assert_eq!(
        binding.endpoint.as_deref(),
        Some("gateway-host.example.ts.net:7778")
    );
    assert_eq!(binding.path_class.as_deref(), Some("tailscale-direct"));

    // Provider-native node/DNS/address evidence remains material path provenance;
    // it never replaces the caller-owned network relationship or gateway service ref.
    assert_eq!(
        binding
            .path_provenance
            .get("peer_stable_id")
            .map(String::as_str),
        Some("node-stable-id-material-only")
    );
    assert_ne!(binding.relationship_ref, "node-stable-id-material-only");
    assert_ne!(logical_service, "gateway-host.example.ts.net");
}
