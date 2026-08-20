use std::{collections::BTreeMap, process::Command};

use epilogos_workcell_sdk::{
    contract::WorkcellRef,
    fabric::{
        FabricPathOffer, FabricPathProvider, FabricPathState, NetworkSecurity, ReachabilityScope,
    },
    provider::{ProviderRef, Result, WorkcellError},
};
use serde_json::Value;

/// Exact upstream source inspected when this optional reference adapter was authored.
pub const TAILSCALE_SOURCE_REVISION: &str = "90ed0bcf4bc227a81e39e686dc52cf24f17b0c63";
pub const TAILSCALE_STATUS_SOURCE: &str = "tailscale/tailscale:ipn/ipnstate/ipnstate.go";
pub const TAILSCALE_STATUS_COMMAND: &str = "tailscale status --json";

/// Provider-local selector for one Tailscale peer.
///
/// Stable node IDs, MagicDNS names and Tailscale IPs are provider binding facts.
/// None is promoted into Workcell semantic identity.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TailscalePeerSelector {
    value: String,
}

impl TailscalePeerSelector {
    pub fn new(value: impl Into<String>) -> Result<Self> {
        let value = value.into();
        if value.trim().is_empty() {
            return Err(WorkcellError::InvalidDemand(
                "Tailscale peer selector must not be empty".into(),
            ));
        }
        Ok(Self { value })
    }

    pub fn as_str(&self) -> &str {
        &self.value
    }
}

/// Independently obtained policy/access evidence for the material relation.
///
/// `tailscale status` peer visibility is not treated as authorization proof.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum TailscalePolicyEvidence {
    Allowed { evidence: String },
    Denied { evidence: String },
    Unknown,
}

impl TailscalePolicyEvidence {
    fn blocked_state(&self) -> Option<FabricPathState> {
        match self {
            Self::Allowed { .. } => None,
            Self::Denied { .. } => Some(FabricPathState::Denied),
            Self::Unknown => Some(FabricPathState::Unavailable),
        }
    }

    fn provenance(&self) -> (&'static str, Option<&str>) {
        match self {
            Self::Allowed { evidence } => ("allowed", Some(evidence)),
            Self::Denied { evidence } => ("denied", Some(evidence)),
            Self::Unknown => ("unverified", None),
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TailscalePathConfig {
    pub provider_ref: ProviderRef,
    pub path_ref: String,
    pub source_workcell: WorkcellRef,
    pub destination_workcell: WorkcellRef,
    pub peer: TailscalePeerSelector,
    /// Target-native endpoint reached through the tailnet.
    pub endpoint: String,
    /// Application/material transport property, where relevant.
    pub transport: Option<String>,
    pub policy: TailscalePolicyEvidence,
}

impl TailscalePathConfig {
    pub fn validate(&self) -> Result<()> {
        if self.path_ref.trim().is_empty() {
            return Err(WorkcellError::InvalidDemand(
                "Tailscale path_ref must not be empty".into(),
            ));
        }
        if self.endpoint.trim().is_empty() {
            return Err(WorkcellError::InvalidDemand(
                "Tailscale material endpoint must not be empty".into(),
            ));
        }
        if self
            .transport
            .as_deref()
            .is_some_and(|transport| transport.trim().is_empty())
        {
            return Err(WorkcellError::InvalidDemand(
                "Tailscale transport must not be empty when supplied".into(),
            ));
        }
        Ok(())
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TailscalePeerObservation {
    pub backend_state: String,
    pub stable_id: String,
    pub hostname: String,
    pub dns_name: String,
    pub tailscale_ips: Vec<String>,
    pub online: bool,
    pub active: bool,
    pub current_address: Option<String>,
    pub derp_relay: Option<String>,
    pub peer_relay: Option<String>,
}

/// Parse the small source-pinned portion of `tailscale status --json` needed by
/// Workcell fabric conformance. Missing fields are treated conservatively.
pub fn parse_tailscale_status(
    status_json: &str,
    selector: &TailscalePeerSelector,
) -> Result<TailscalePeerObservation> {
    let value: Value = serde_json::from_str(status_json).map_err(|error| {
        WorkcellError::OperationFailed(format!("parse Tailscale status JSON: {error}"))
    })?;
    let backend_state = value
        .get("BackendState")
        .and_then(Value::as_str)
        .unwrap_or("unknown")
        .to_owned();
    let peers = value
        .get("Peer")
        .and_then(Value::as_object)
        .ok_or_else(|| {
            WorkcellError::Unavailable(
                "Tailscale status did not expose a Peer object at the pinned seam".into(),
            )
        })?;

    let needle = selector.as_str().trim_end_matches('.');
    for peer in peers.values() {
        let stable_id = string_field(peer, "ID");
        let hostname = string_field(peer, "HostName");
        let dns_name = string_field(peer, "DNSName");
        let tailscale_ips = string_array_field(peer, "TailscaleIPs");
        let matches = stable_id == selector.as_str()
            || hostname == selector.as_str()
            || dns_name.trim_end_matches('.') == needle
            || tailscale_ips.iter().any(|ip| ip == selector.as_str());
        if !matches {
            continue;
        }

        return Ok(TailscalePeerObservation {
            backend_state,
            stable_id,
            hostname,
            dns_name,
            tailscale_ips,
            online: bool_field(peer, "Online"),
            active: bool_field(peer, "Active"),
            current_address: optional_string_field(peer, "CurAddr"),
            derp_relay: optional_string_field(peer, "Relay"),
            peer_relay: optional_string_field(peer, "PeerRelay"),
        });
    }

    Err(WorkcellError::Unavailable(format!(
        "Tailscale peer selector `{}` was not present in status",
        selector.as_str()
    )))
}

pub fn tailscale_path_from_status_json(
    config: &TailscalePathConfig,
    status_json: &str,
) -> Result<FabricPathOffer> {
    config.validate()?;
    let observation = parse_tailscale_status(status_json, &config.peer)?;
    Ok(path_from_observation(config, &observation))
}

/// Optional external-style reference adapter over an installed Tailscale CLI.
///
/// It depends only on `epilogos-workcell-sdk` plus Tailscale's public command
/// output. Installation, authentication, Grants, Serve/Funnel and Tailscale SSH
/// remain target/deployment responsibilities.
pub struct TailscaleFabricProvider {
    config: TailscalePathConfig,
    program: String,
}

impl TailscaleFabricProvider {
    pub fn new(config: TailscalePathConfig) -> Result<Self> {
        config.validate()?;
        Ok(Self {
            config,
            program: "tailscale".into(),
        })
    }

    pub fn with_program(mut self, program: impl Into<String>) -> Result<Self> {
        let program = program.into();
        if program.trim().is_empty() {
            return Err(WorkcellError::InvalidDemand(
                "Tailscale command program must not be empty".into(),
            ));
        }
        self.program = program;
        Ok(self)
    }

    fn unavailable_path(&self, detail: impl Into<String>) -> FabricPathOffer {
        let mut provenance = base_provenance(&self.config);
        provenance.insert("observation_error".into(), detail.into());
        FabricPathOffer {
            provider_ref: self.config.provider_ref.clone(),
            path_ref: self.config.path_ref.clone(),
            source_workcell: self.config.source_workcell.clone(),
            destination_workcell: self.config.destination_workcell.clone(),
            transport: self.config.transport.clone(),
            scope: ReachabilityScope::Private,
            security: NetworkSecurity::AuthenticatedEncrypted,
            state: FabricPathState::Unavailable,
            endpoint: Some(self.config.endpoint.clone()),
            path_class: Some("tailscale-unavailable".into()),
            provenance,
        }
    }
}

impl FabricPathProvider for TailscaleFabricProvider {
    fn provider_ref(&self) -> &ProviderRef {
        &self.config.provider_ref
    }

    fn paths(&self) -> Result<Vec<FabricPathOffer>> {
        let output = match Command::new(&self.program)
            .args(["status", "--json"])
            .output()
        {
            Ok(output) => output,
            Err(error) => {
                return Ok(vec![self.unavailable_path(format!(
                    "execute `{}` status --json: {error}",
                    self.program
                ))]);
            }
        };
        if !output.status.success() {
            return Ok(vec![self.unavailable_path(format!(
                "`{}` status --json exited {}",
                self.program, output.status
            ))]);
        }
        let status_json = String::from_utf8(output.stdout).map_err(|error| {
            WorkcellError::OperationFailed(format!("decode Tailscale status output: {error}"))
        })?;
        match tailscale_path_from_status_json(&self.config, &status_json) {
            Ok(path) => Ok(vec![path]),
            Err(error) => Ok(vec![self.unavailable_path(error.to_string())]),
        }
    }
}

fn path_from_observation(
    config: &TailscalePathConfig,
    observation: &TailscalePeerObservation,
) -> FabricPathOffer {
    let mut provenance = base_provenance(config);
    provenance.insert("backend_state".into(), observation.backend_state.clone());
    provenance.insert("peer_stable_id".into(), observation.stable_id.clone());
    provenance.insert("peer_hostname".into(), observation.hostname.clone());
    provenance.insert("peer_dns_name".into(), observation.dns_name.clone());
    provenance.insert("peer_online".into(), observation.online.to_string());
    provenance.insert("peer_active".into(), observation.active.to_string());
    provenance.insert(
        "peer_tailscale_ips".into(),
        observation.tailscale_ips.join(","),
    );
    if let Some(address) = &observation.current_address {
        provenance.insert("current_direct_address".into(), address.clone());
    }
    if let Some(relay) = &observation.peer_relay {
        provenance.insert("peer_relay".into(), relay.clone());
    }
    if let Some(relay) = &observation.derp_relay {
        provenance.insert("derp_relay".into(), relay.clone());
    }
    let (policy_result, policy_evidence) = config.policy.provenance();
    provenance.insert("policy_result".into(), policy_result.into());
    if let Some(evidence) = policy_evidence {
        provenance.insert("policy_evidence".into(), evidence.into());
    }

    let (state, path_class) = if observation.backend_state != "Running" || !observation.online {
        (FabricPathState::Unavailable, "tailscale-offline")
    } else if let Some(state) = config.policy.blocked_state() {
        let path_class = if matches!(&config.policy, TailscalePolicyEvidence::Denied { .. }) {
            "tailscale-policy-denied"
        } else {
            "tailscale-policy-unverified"
        };
        (state, path_class)
    } else if observation.current_address.is_some() {
        (FabricPathState::Reachable, "tailscale-direct")
    } else if observation.peer_relay.is_some() {
        (FabricPathState::Reachable, "tailscale-peer-relay")
    } else if observation.derp_relay.is_some() {
        (FabricPathState::Reachable, "tailscale-derp-relay")
    } else {
        (FabricPathState::Degraded, "tailscale-tailnet-visible")
    };

    FabricPathOffer {
        provider_ref: config.provider_ref.clone(),
        path_ref: config.path_ref.clone(),
        source_workcell: config.source_workcell.clone(),
        destination_workcell: config.destination_workcell.clone(),
        transport: config.transport.clone(),
        scope: ReachabilityScope::Private,
        security: NetworkSecurity::AuthenticatedEncrypted,
        state,
        endpoint: Some(config.endpoint.clone()),
        path_class: Some(path_class.into()),
        provenance,
    }
}

fn base_provenance(config: &TailscalePathConfig) -> BTreeMap<String, String> {
    BTreeMap::from([
        ("implementation".into(), "tailscale-reference".into()),
        ("source_revision".into(), TAILSCALE_SOURCE_REVISION.into()),
        ("status_source".into(), TAILSCALE_STATUS_SOURCE.into()),
        ("status_command".into(), TAILSCALE_STATUS_COMMAND.into()),
        ("peer_selector".into(), config.peer.as_str().into()),
    ])
}

fn string_field(value: &Value, key: &str) -> String {
    value
        .get(key)
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_owned()
}

fn optional_string_field(value: &Value, key: &str) -> Option<String> {
    let value = string_field(value, key);
    (!value.is_empty()).then_some(value)
}

fn bool_field(value: &Value, key: &str) -> bool {
    value.get(key).and_then(Value::as_bool).unwrap_or(false)
}

fn string_array_field(value: &Value, key: &str) -> Vec<String> {
    value
        .get(key)
        .and_then(Value::as_array)
        .map(|values| {
            values
                .iter()
                .filter_map(Value::as_str)
                .map(str::to_owned)
                .collect()
        })
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use epilogos_workcell_sdk::{
        contract::{PlanStatus, RequirementNecessity},
        fabric::{
            evaluate_fabric, require_fabric_plan, NetworkRelationship, RequiredNetworkRelationship,
        },
    };

    use super::*;

    const DIRECT_STATUS: &str = r#"{
      "BackendState": "Running",
      "Peer": {
        "nodekey:peer": {
          "ID": "peer-stable-id",
          "HostName": "server",
          "DNSName": "server.example.ts.net.",
          "TailscaleIPs": ["100.64.0.20", "fd7a:115c:a1e0::20"],
          "CurAddr": "192.0.2.50:41641",
          "Relay": "",
          "PeerRelay": "",
          "Online": true,
          "Active": true
        }
      }
    }"#;

    fn config(policy: TailscalePolicyEvidence) -> TailscalePathConfig {
        TailscalePathConfig {
            provider_ref: ProviderRef::new("provider:tailscale-reference").unwrap(),
            path_ref: "path:tailscale-server".into(),
            source_workcell: WorkcellRef::new("workcell:client").unwrap(),
            destination_workcell: WorkcellRef::new("workcell:server").unwrap(),
            peer: TailscalePeerSelector::new("server.example.ts.net").unwrap(),
            endpoint: "server.example.ts.net:7777".into(),
            transport: Some("tcp".into()),
            policy,
        }
    }

    fn relationship() -> RequiredNetworkRelationship {
        RequiredNetworkRelationship {
            relationship: NetworkRelationship::new(
                "relationship:workcell-control",
                "workcell:client/control",
                "workcell:server/control",
            )
            .unwrap()
            .with_transport("tcp")
            .unwrap()
            .with_scope(ReachabilityScope::Private)
            .with_security(NetworkSecurity::AuthenticatedEncrypted),
            necessity: RequirementNecessity::Required,
            source_workcell: WorkcellRef::new("workcell:client").unwrap(),
            destination_workcell: WorkcellRef::new("workcell:server").unwrap(),
        }
    }

    struct Fixture(FabricPathOffer);

    impl FabricPathProvider for Fixture {
        fn provider_ref(&self) -> &ProviderRef {
            &self.0.provider_ref
        }

        fn paths(&self) -> Result<Vec<FabricPathOffer>> {
            Ok(vec![self.0.clone()])
        }
    }

    #[test]
    fn direct_path_keeps_provider_addresses_out_of_relationship_identity() {
        let path = tailscale_path_from_status_json(
            &config(TailscalePolicyEvidence::Allowed {
                evidence: "probe:control-service-authenticated".into(),
            }),
            DIRECT_STATUS,
        )
        .unwrap();
        assert_eq!(path.state, FabricPathState::Reachable);
        assert_eq!(path.path_class.as_deref(), Some("tailscale-direct"));
        assert_eq!(
            path.provenance
                .get("current_direct_address")
                .map(String::as_str),
            Some("192.0.2.50:41641")
        );

        let provider = Fixture(path);
        let plan =
            require_fabric_plan(evaluate_fabric(&[relationship()], &[&provider]).unwrap()).unwrap();
        assert_eq!(plan.status, PlanStatus::Satisfiable);
        assert_eq!(
            plan.bindings[0].relationship_ref,
            "relationship:workcell-control"
        );
        assert_eq!(
            plan.bindings[0].provenance["peer_stable_id"],
            "peer-stable-id"
        );
    }

    #[test]
    fn direct_peer_relay_and_derp_change_path_class_not_binding_identity() {
        let direct = tailscale_path_from_status_json(
            &config(TailscalePolicyEvidence::Allowed {
                evidence: "probe:allowed".into(),
            }),
            DIRECT_STATUS,
        )
        .unwrap();
        let peer_relay_json = DIRECT_STATUS
            .replace("\"CurAddr\": \"192.0.2.50:41641\"", "\"CurAddr\": \"\"")
            .replace("\"PeerRelay\": \"\"", "\"PeerRelay\": \"100.64.0.7:1:23\"");
        let peer_relay = tailscale_path_from_status_json(
            &config(TailscalePolicyEvidence::Allowed {
                evidence: "probe:allowed".into(),
            }),
            &peer_relay_json,
        )
        .unwrap();
        let derp_json = DIRECT_STATUS
            .replace("\"CurAddr\": \"192.0.2.50:41641\"", "\"CurAddr\": \"\"")
            .replace("\"Relay\": \"\"", "\"Relay\": \"lhr\"");
        let derp = tailscale_path_from_status_json(
            &config(TailscalePolicyEvidence::Allowed {
                evidence: "probe:allowed".into(),
            }),
            &derp_json,
        )
        .unwrap();

        assert_eq!(direct.path_class.as_deref(), Some("tailscale-direct"));
        assert_eq!(
            peer_relay.path_class.as_deref(),
            Some("tailscale-peer-relay")
        );
        assert_eq!(derp.path_class.as_deref(), Some("tailscale-derp-relay"));
        assert_eq!(direct.path_ref, peer_relay.path_ref);
        assert_eq!(peer_relay.path_ref, derp.path_ref);
        assert_eq!(direct.endpoint, derp.endpoint);
    }

    #[test]
    fn unknown_or_denied_policy_is_never_promoted_to_reachable() {
        let unknown = tailscale_path_from_status_json(
            &config(TailscalePolicyEvidence::Unknown),
            DIRECT_STATUS,
        )
        .unwrap();
        assert_eq!(unknown.state, FabricPathState::Unavailable);
        assert_eq!(unknown.provenance["policy_result"], "unverified");

        let denied = tailscale_path_from_status_json(
            &config(TailscalePolicyEvidence::Denied {
                evidence: "grant-probe:denied".into(),
            }),
            DIRECT_STATUS,
        )
        .unwrap();
        assert_eq!(denied.state, FabricPathState::Denied);
        assert_eq!(denied.provenance["policy_result"], "denied");
    }

    #[test]
    fn offline_peer_is_unavailable_even_with_prior_allowed_policy() {
        let offline_json = DIRECT_STATUS.replace("\"Online\": true", "\"Online\": false");
        let offline = tailscale_path_from_status_json(
            &config(TailscalePolicyEvidence::Allowed {
                evidence: "historic-probe:allowed".into(),
            }),
            &offline_json,
        )
        .unwrap();
        assert_eq!(offline.state, FabricPathState::Unavailable);
        assert_eq!(offline.path_class.as_deref(), Some("tailscale-offline"));
    }
}
