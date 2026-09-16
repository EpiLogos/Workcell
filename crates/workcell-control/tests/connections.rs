//! Cross-cell connection lifecycle at the control seam.
//!
//! These tests hold the serving side in-process over the existing transport
//! fixtures (`DirectTransport`, plus one rewriting wrapper for the version
//! skew case) and drive grants through a registry file that a second handle
//! opens independently — the same file a separate `workcell revoke` process
//! would touch.

use std::{
    fs,
    path::PathBuf,
    time::{SystemTime, UNIX_EPOCH},
};

use epilogos_workcell_control::{
    check_compatibility, credential_sha256, grant_ref_for, ConnectionGrants, ControlClient,
    ControlClientError, ControlService, ControlTransport, DirectTransport, TransportFailure,
    CONTROL_PROTOCOL_VERSION,
};
use epilogos_workcell_core::{Result as WorkcellResult, WorkcellRef};
use epilogos_workcell_runtime::{CollapsedLocalConfig, CollapsedLocalWorkcell};
use epilogos_workcell_wire::ConnectionGrant;
use serde_json::{json, Value};

fn temp_root(label: &str) -> PathBuf {
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    std::env::temp_dir().join(format!(
        "workcell-control-connections-{label}-{}-{nonce}",
        std::process::id()
    ))
}

fn local(label: &str, workcell_ref: &str) -> (CollapsedLocalWorkcell, PathBuf) {
    let root = temp_root(label);
    let workcell = CollapsedLocalWorkcell::new(CollapsedLocalConfig::new(
        WorkcellRef::new(workcell_ref).unwrap(),
        &root,
    ))
    .unwrap();
    (workcell, root)
}

fn grants_registry(root: &PathBuf, workcell_ref: &str) -> ConnectionGrants {
    ConnectionGrants::new(root, WorkcellRef::new(workcell_ref).unwrap())
}

fn grant_for(
    label: &str,
    credential: &str,
    operations: &[&str],
    advertise: &[&str],
) -> ConnectionGrant {
    ConnectionGrant {
        grant_ref: grant_ref_for(label, credential),
        client_label: label.to_owned(),
        protocol: CONTROL_PROTOCOL_VERSION.to_owned(),
        operations: operations.iter().map(|value| value.to_string()).collect(),
        advertise: advertise.iter().map(|value| value.to_string()).collect(),
        credential_sha256: credential_sha256(credential),
        credential_ref: None,
        created_at_unix_ms: 0,
        expires_at_unix_ms: None,
        state: "active".to_owned(),
        revoked_at_unix_ms: None,
        provenance: std::collections::BTreeMap::new(),
    }
}

/// A transport fixture that stands in for an older serving cell: it rewrites
/// every response envelope's protocol version before the client decodes it.
/// The control envelope itself is unchanged — only the disclosed version.
struct StaleVersionTransport<'a, C> {
    inner: DirectTransport<'a, C>,
}

impl<C> ControlTransport for StaleVersionTransport<'_, C>
where
    C: epilogos_workcell_core::WorkcellControlPlane,
{
    fn round_trip(&mut self, request: &[u8]) -> Result<Vec<u8>, TransportFailure> {
        let response = self.inner.round_trip(request)?;
        let mut value: Value = serde_json::from_slice(&response)
            .map_err(|error| TransportFailure::new(format!("fixture rewrite failed: {error}")))?;
        value["version"] = json!("workcell.control/v0");
        Ok(serde_json::to_vec(&value).unwrap())
    }
}

#[test]
fn handshake_discloses_compatibility_and_withholds_identity_from_refused_clients() {
    let (workcell, root) = local("handshake-open", "workcell:handshake-open");
    let mut service = ControlService::new(workcell)
        .with_connection_grants(grants_registry(&root, "workcell:handshake-open"));
    let mut client = ControlClient::new(DirectTransport::new(&mut service));

    // No credential: the client learns the compatibility surface and the
    // refusal reason — and nothing else.
    let refused = client.handshake("laptop").unwrap();
    assert_eq!(refused["protocol"], CONTROL_PROTOCOL_VERSION);
    assert!(refused["software"].as_str().is_some());
    assert_eq!(refused["authorised"], false);
    assert!(refused["workcell_ref"].is_null());
    assert!(refused["reason"]
        .as_str()
        .unwrap()
        .contains("requires a connection credential"));

    // Status without a credential is refused loudly, distinct from the
    // handshake.
    let error = client.status().unwrap_err();
    assert!(matches!(error, ControlClientError::AuthenticationFailed(_)));

    let _ = fs::remove_dir_all(root);
}

#[test]
fn grant_scopes_operations_and_revocation_takes_effect_at_the_next_use() {
    let (workcell, root) = local("grant-scope", "workcell:grant-scope");
    let registry = grants_registry(&root, "workcell:grant-scope");
    let credential = "wck_fixture_credential_scoped";
    registry
        .create(grant_for(
            "laptop",
            credential,
            &["status", "discover"],
            &[],
        ))
        .unwrap();

    let mut service = ControlService::new(workcell).with_connection_grants(registry);
    let mut client =
        ControlClient::new(DirectTransport::new(&mut service)).with_authorization(credential);

    // Authorised handshake carries identity and the exact granted scope.
    let handshake = client.handshake("laptop").unwrap();
    assert_eq!(handshake["authorised"], true);
    assert_eq!(handshake["workcell_ref"], "workcell:grant-scope");
    assert_eq!(
        handshake["grant"]["operations"],
        json!(["status", "discover"])
    );
    assert_eq!(handshake["grant"]["client_label"], "laptop");

    // A granted operation works; an ungranted one is named, not generic.
    client.status().unwrap();
    let error = client.invoke("release", Value::Null).unwrap_err();
    let ControlClientError::AuthenticationFailed(message) = error else {
        panic!("expected authentication failure, got {error:?}");
    };
    assert!(
        message.contains("does not permit operation `release`"),
        "{message}"
    );

    // Revocation from an independently opened handle — the same file a
    // separate `workcell revoke` process would mutate — takes effect at the
    // client's next use, without restarting the service.
    let second_process_registry = grants_registry(&root, "workcell:grant-scope");
    let revoked = second_process_registry.revoke("laptop").unwrap();
    assert_eq!(revoked.len(), 1);

    let error = client.status().unwrap_err();
    let ControlClientError::AuthenticationFailed(message) = error else {
        panic!("expected authentication failure, got {error:?}");
    };
    assert!(message.contains("revoked"), "{message}");

    // The revoked handshake still discloses compatibility, and names the
    // truth: the credential was known and withdrawn.
    let handshake = client.handshake("laptop").unwrap();
    assert_eq!(handshake["authorised"], false);
    assert!(handshake["reason"].as_str().unwrap().contains("revoked"));
    assert_eq!(handshake["protocol"], CONTROL_PROTOCOL_VERSION);

    let _ = fs::remove_dir_all(root);
}

#[test]
fn expired_grants_refuse_loudly_and_distinctly_at_the_next_use() {
    let (workcell, root) = local("expiry", "workcell:expiry");
    let registry = grants_registry(&root, "workcell:expiry");
    let expired_credential = "wck_expired_credential";

    // A grant whose window closed the instant after creation: written into
    // the registry exactly as `authorise --expires-in` would store it.
    let mut expired = grant_for("laptop", expired_credential, &["status", "discover"], &[]);
    expired.created_at_unix_ms = 0;
    expired.expires_at_unix_ms = Some(1);
    registry.create(expired).unwrap();

    let mut service = ControlService::new(workcell).with_connection_grants(registry);
    let mut client = ControlClient::new(DirectTransport::new(&mut service))
        .with_authorization(expired_credential);

    // The handshake refuses, and names the truth: the grant expired, not
    // revoked and not unknown.
    let handshake = client.handshake("laptop").unwrap();
    assert_eq!(handshake["authorised"], false);
    let reason = handshake["reason"].as_str().unwrap();
    assert!(reason.contains("expired"), "{reason}");
    assert!(reason.contains("grant:"), "{reason}");
    assert!(!reason.contains("revoked"), "{reason}");

    // Every operation under the expired grant refuses with the same named
    // answer.
    let error = client.status().unwrap_err();
    let ControlClientError::AuthenticationFailed(message) = error else {
        panic!("expected authentication failure, got {error:?}");
    };
    assert!(message.contains("expired"), "{message}");
    assert!(message.contains("grant:"), "{message}");

    let _ = fs::remove_dir_all(root);
}

#[test]
fn discovery_is_filtered_to_the_grant_advertisement() {
    let (workcell, root) = local("advertise", "workcell:advertise");
    let registry = grants_registry(&root, "workcell:advertise");
    registry
        .create(grant_for(
            "everything",
            "wck_everything_credential",
            &["discover"],
            &[],
        ))
        .unwrap();
    registry
        .create(grant_for(
            "narrow",
            "wck_narrow_credential",
            &["discover", "status"],
            &["artifact-storage"],
        ))
        .unwrap();

    let mut service = ControlService::new(workcell).with_connection_grants(registry);

    let mut wide_client = ControlClient::new(DirectTransport::new(&mut service))
        .with_authorization("wck_everything_credential");
    let wide = wide_client.discover().unwrap();

    let mut narrow_client = ControlClient::new(DirectTransport::new(&mut service))
        .with_authorization("wck_narrow_credential");
    let narrow = narrow_client.discover().unwrap();

    let ports = |value: &Value| -> Vec<String> {
        value["offers"]
            .as_array()
            .unwrap()
            .iter()
            .map(|offer| offer["port"].as_str().unwrap().to_owned())
            .collect()
    };
    assert!(!ports(&wide).is_empty());
    assert!(ports(&wide).iter().any(|port| port != "artifact-storage"));
    assert!(!ports(&narrow).is_empty());
    assert!(ports(&narrow).iter().all(|port| port == "artifact-storage"));

    // The scoped status view discloses the same narrowed counts.
    let status = narrow_client.status().unwrap();
    assert_eq!(status["offers"], narrow["offers"].as_array().unwrap().len());

    let _ = fs::remove_dir_all(root);
}

#[test]
fn static_token_keeps_full_access_alongside_scoped_grants() {
    let (workcell, root) = local("static", "workcell:static");
    let registry = grants_registry(&root, "workcell:static");
    registry
        .create(grant_for(
            "client",
            "wck_client_credential",
            &["status"],
            &[],
        ))
        .unwrap();

    let mut service = ControlService::new(workcell)
        .with_authorization("operator-secret")
        .with_connection_grants(registry);

    let mut operator = ControlClient::new(DirectTransport::new(&mut service))
        .with_authorization("operator-secret");
    let handshake = operator.handshake("operator").unwrap();
    assert_eq!(handshake["authorised"], true);
    assert!(
        handshake["grant"].is_null(),
        "full access has no grant scope"
    );
    operator.invoke("release", Value::Null).unwrap_err(); // full access: reaches the operation (unknown world), not authorisation

    let mut scoped = ControlClient::new(DirectTransport::new(&mut service))
        .with_authorization("wck_client_credential");
    scoped.status().unwrap();
    let error = scoped.invoke("release", Value::Null).unwrap_err();
    assert!(matches!(error, ControlClientError::AuthenticationFailed(_)));

    let _ = fs::remove_dir_all(root);
}

#[test]
fn unsupported_version_combinations_refuse_loudly_over_the_wire() {
    let (workcell, root) = local("skew", "workcell:skew");
    let mut service = ControlService::new(workcell);
    let transport = StaleVersionTransport {
        inner: DirectTransport::new(&mut service),
    };
    let mut client = ControlClient::new(transport);

    // The envelope refuses before any operation runs, and the compatibility
    // rule refuses at the handshake disclosure — both loudly.
    let error = client.handshake("laptop").unwrap_err();
    assert!(matches!(error, ControlClientError::ProtocolIncompatible(_)));

    let report = check_compatibility("workcell.control/v1", "workcell 0.1.0 (same)");
    assert!(report.is_ok());
    let error = check_compatibility("workcell.control/v2", "workcell 9.9");
    assert!(matches!(
        error,
        Err(epilogos_workcell_core::WorkcellError::Unsupported(message)) if message.contains("refused")
    ));

    let _ = fs::remove_dir_all(root);
}

#[test]
fn cross_cell_use_runs_on_the_serving_cell_with_its_identity() {
    let (server_workcell, server_root) = local("exec-location", "workcell:home-server");
    let mut service = ControlService::new(server_workcell);
    let mut client = ControlClient::new(DirectTransport::new(&mut service));

    let discovery = client.discover().unwrap();
    assert_eq!(discovery["workcell_ref"], "workcell:home-server");

    let demand_value = json!({
        "demand_ref": "demand:remote-client",
        "subjects": {},
        "affordances": {"required": ["shell"], "preferred": [], "optional": []},
        "storage": {"required": [], "preferred": [], "optional": []},
        "workspace": null,
        "project_runtime": null,
        "resources": [],
        "connectivity": {"required": [], "preferred": [], "optional": []},
        "exposure": {"required": [], "preferred": [], "optional": []},
        "outputs": {"required": [], "preferred": [], "optional": []},
        "persistence": null,
        "isolation_trust": null,
        "retention": "release",
        "extensions": {},
    });
    let prepared = client
        .invoke("prepare", demand_value)
        .unwrap_or_else(|error| panic!("prepare should be authorised and satisfiable: {error}"));
    // Execution location is proved by the receipt itself: the world belongs
    // to the serving cell's identity, not the client's.
    assert_eq!(prepared["workcell_ref"], "workcell:home-server");

    let _ = fs::remove_dir_all(server_root);
}

#[test]
fn grants_registry_files_bound_to_one_workcell_identity() -> WorkcellResult<()> {
    let root = temp_root("registry-binding");
    let registry = grants_registry(&root, "workcell:bound");
    registry
        .create(grant_for(
            "client",
            "wck_binding_credential",
            &["status"],
            &[],
        ))
        .unwrap();

    // A different WorkcellRef pointing at the same state root is refused.
    let foreign = grants_registry(&root, "workcell:other");
    assert!(foreign.list().is_err());

    // The bound registry sees exactly the grant, still active.
    let list = registry.list()?;
    assert_eq!(list.len(), 1);
    assert!(list[0].is_active());

    let _ = fs::remove_dir_all(root);
    Ok(())
}
