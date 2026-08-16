use std::{
    fs,
    path::PathBuf,
    time::{SystemTime, UNIX_EPOCH},
};

use epilogos_workcell_control::{
    codec, ControlClient, ControlClientError, ControlService, DirectTransport,
    LengthPrefixedTransport, UnavailableTransport,
};
use epilogos_workcell_core::{
    AffordanceRequirement, DemandRef, DesiredMaterialState, ExecutionDemand, WorkcellControlPlane,
    WorkcellError, WorkcellRef, WorldRef,
};
use epilogos_workcell_runtime::{CollapsedLocalConfig, CollapsedLocalWorkcell};
use epilogos_workcell_wire::decode_world;

fn temp_path(label: &str) -> PathBuf {
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    std::env::temp_dir().join(format!(
        "epilogos-workcell-control-{label}-{}-{nonce}",
        std::process::id()
    ))
}

fn local(label: &str) -> (CollapsedLocalWorkcell, PathBuf) {
    let root = temp_path(label);
    let workcell = CollapsedLocalWorkcell::new(CollapsedLocalConfig::new(
        WorkcellRef::new("workcell:control-test").unwrap(),
        &root,
    ))
    .unwrap();
    (workcell, root)
}

fn shell_demand(label: &str) -> ExecutionDemand {
    let mut demand = ExecutionDemand::new(DemandRef::new(format!("demand:{label}")).unwrap());
    demand
        .affordances
        .required
        .push(AffordanceRequirement::new("shell").unwrap());
    demand
}

#[test]
fn service_plan_is_semantically_identical_to_embedded_plan() {
    let demand = shell_demand("parity");
    let (embedded, embedded_root) = local("embedded-parity");
    let embedded_plan = embedded.plan(&demand).unwrap();

    let (remote, remote_root) = local("remote-parity");
    let mut service = ControlService::new(remote);
    let transport = DirectTransport::new(&mut service);
    let mut client = ControlClient::new(transport);
    let remote_plan = client.plan(&demand).unwrap();

    assert_eq!(remote_plan, codec::plan_value(&embedded_plan));

    let _ = fs::remove_dir_all(embedded_root);
    let _ = fs::remove_dir_all(remote_root);
}

#[test]
fn direct_and_length_prefixed_paths_carry_equivalent_calls() {
    let demand = shell_demand("transport-parity");
    let (direct_workcell, direct_root) = local("direct-path");
    let mut direct_service = ControlService::new(direct_workcell);
    let mut direct_client = ControlClient::new(DirectTransport::new(&mut direct_service));

    let (framed_workcell, framed_root) = local("framed-path");
    let mut framed_service = ControlService::new(framed_workcell);
    let mut framed_client = ControlClient::new(LengthPrefixedTransport::new(&mut framed_service));

    let direct_discovery = direct_client.discover().unwrap();
    let framed_discovery = framed_client.discover().unwrap();
    assert_eq!(
        direct_discovery["workcell_ref"],
        framed_discovery["workcell_ref"]
    );
    assert_eq!(direct_discovery["health"], framed_discovery["health"]);
    assert_eq!(
        direct_discovery["offers"].as_array().unwrap().len(),
        framed_discovery["offers"].as_array().unwrap().len()
    );
    assert_ne!(
        direct_discovery["offers"][0]["metadata"]["root"],
        framed_discovery["offers"][0]["metadata"]["root"]
    );
    assert_eq!(
        direct_client.plan(&demand).unwrap(),
        framed_client.plan(&demand).unwrap()
    );

    let _ = fs::remove_dir_all(direct_root);
    let _ = fs::remove_dir_all(framed_root);
}

#[test]
fn authentication_protocol_transport_and_remote_failures_remain_distinct() {
    let (auth_workcell, auth_root) = local("auth");
    let mut auth_service = ControlService::new(auth_workcell).with_authorization("secret");
    let mut unauthenticated = ControlClient::new(DirectTransport::new(&mut auth_service));
    assert!(matches!(
        unauthenticated.discover(),
        Err(ControlClientError::AuthenticationFailed(_))
    ));
    drop(unauthenticated);
    let mut authenticated =
        ControlClient::new(DirectTransport::new(&mut auth_service)).with_authorization("secret");
    assert!(authenticated.discover().is_ok());

    let (version_workcell, version_root) = local("version");
    let mut version_service = ControlService::new(version_workcell);
    let mut incompatible = ControlClient::new(DirectTransport::new(&mut version_service))
        .with_protocol_version("workcell.control/v99");
    assert!(matches!(
        incompatible.discover(),
        Err(ControlClientError::ProtocolIncompatible(_))
    ));

    let mut unavailable = ControlClient::new(UnavailableTransport);
    assert!(matches!(
        unavailable.discover(),
        Err(ControlClientError::TransportUnavailable(_))
    ));

    let (remote_failure_workcell, remote_failure_root) = local("remote-failure");
    let mut remote_failure_service = ControlService::new(remote_failure_workcell);
    let mut remote_failure = ControlClient::new(DirectTransport::new(&mut remote_failure_service));
    let missing_world = WorldRef::new("world:missing").unwrap();
    assert!(matches!(
        remote_failure.observe(&missing_world),
        Err(ControlClientError::Remote(WorkcellError::NotFound(_)))
    ));

    let _ = fs::remove_dir_all(auth_root);
    let _ = fs::remove_dir_all(version_root);
    let _ = fs::remove_dir_all(remote_failure_root);
}

#[test]
fn service_hosts_the_same_complete_operation_surface() {
    let demand = shell_demand("complete-surface");
    let (workcell, root) = local("complete-surface");
    let mut service = ControlService::new(workcell);
    let mut client = ControlClient::new(DirectTransport::new(&mut service));

    assert_eq!(client.status().unwrap()["health"], "healthy");
    assert!(
        client.discover().unwrap()["offers"]
            .as_array()
            .unwrap()
            .len()
            >= 3
    );
    assert_eq!(client.plan(&demand).unwrap()["status"], "satisfiable");

    let prepared = client.prepare(&demand).unwrap();
    let world = decode_world(&serde_json::to_string(&prepared).unwrap()).unwrap();
    assert_eq!(world.demand_ref, demand.demand_ref);

    let observed = client.observe(&world.world_ref).unwrap();
    assert_eq!(observed["world_ref"], world.world_ref.as_str());
    assert!(observed["observations"]
        .as_array()
        .unwrap()
        .iter()
        .any(|item| item["logical_ref"] == "affordance:shell"));

    let exposed = client.expose(&world.world_ref).unwrap();
    assert_eq!(exposed["world_ref"], world.world_ref.as_str());

    let collected = client.collect(&world.world_ref).unwrap();
    assert_eq!(collected["world_ref"], world.world_ref.as_str());

    let reconciled = client
        .reconcile(&[DesiredMaterialState {
            logical_ref: "affordance:shell".into(),
            desired: "present".into(),
        }])
        .unwrap();
    let deltas = reconciled["deltas"].as_array().unwrap();
    assert!(deltas
        .iter()
        .any(|delta| delta["logical_ref"] == "affordance:shell" && delta["desired"] == "present"));

    let released = client.release(&world.world_ref).unwrap();
    assert_eq!(released["world_ref"], world.world_ref.as_str());
    assert_eq!(released["disposition"], "released");

    let _ = fs::remove_dir_all(root);
}

#[test]
fn restart_and_reconnect_preserve_material_world_identity() {
    let demand = shell_demand("restart");
    let (workcell, root) = local("restart");
    let mut first_service = ControlService::new(workcell);
    let mut first_client = ControlClient::new(DirectTransport::new(&mut first_service));
    let prepared = first_client.prepare(&demand).unwrap();
    let world = decode_world(&serde_json::to_string(&prepared).unwrap()).unwrap();
    let world_ref = world.world_ref.clone();
    drop(first_client);
    drop(first_service);

    let mut restarted = CollapsedLocalWorkcell::new(CollapsedLocalConfig::new(
        WorkcellRef::new("workcell:control-test").unwrap(),
        &root,
    ))
    .unwrap();
    restarted.register_world(world).unwrap();
    let mut second_service = ControlService::new(restarted);
    let mut second_client = ControlClient::new(DirectTransport::new(&mut second_service));

    let observed = second_client.observe(&world_ref).unwrap();
    assert_eq!(observed["world_ref"], world_ref.as_str());
    assert!(observed["observations"]
        .as_array()
        .unwrap()
        .iter()
        .any(|item| item["logical_ref"] == "affordance:shell"));
    assert_eq!(
        second_client.release(&world_ref).unwrap()["world_ref"],
        world_ref.as_str()
    );

    let _ = fs::remove_dir_all(root);
}
