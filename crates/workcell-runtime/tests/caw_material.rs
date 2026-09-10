use epilogos_workcell_core::*;
use epilogos_workcell_runtime::*;
use std::{
    fs,
    path::{Path, PathBuf},
    time::{SystemTime, UNIX_EPOCH},
};
fn root(label: &str) -> PathBuf {
    let p = std::env::temp_dir().join(format!(
        "workcell-caw-{label}-{}-{}",
        std::process::id(),
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    fs::create_dir_all(&p).unwrap();
    p
}
fn config(root: &Path, now: &Path) -> CollapsedLocalConfig {
    CollapsedLocalConfig::new(
        WorkcellRef::new("workcell:caw").unwrap(),
        root.join("state"),
    )
    .with_directory_storage(DirectoryStorage::new("now:task", now).unwrap())
}
fn storage_demand() -> ExecutionDemand {
    let mut d = ExecutionDemand::new(DemandRef::new("demand:caw").unwrap());
    let mut s = StorageRequirement::new("now:task").unwrap();
    s.sharing = StorageSharing::Shared;
    d.storage.required.push(s);
    for (key, reference) in [
        ("agent", "agent:opaque"),
        ("session", "session:opaque"),
        ("source", "source:now"),
        ("attempt", "attempt:opaque"),
        ("policy", "source:policy/revision"),
    ] {
        d.subjects
            .insert(key.into(), ExternalRef::new(reference).unwrap());
    }
    d
}
#[test]
fn now_directory_is_attached_not_minted_copied_or_deleted() {
    let root = root("storage");
    let now = root.join("NOW");
    let wrong = root.join("other");
    fs::create_dir_all(&now).unwrap();
    fs::create_dir_all(&wrong).unwrap();
    fs::write(now.join("authored.md"), "unaltered authored bytes").unwrap();
    let make = || {
        config(&root, &now)
            .with_directory_storage(DirectoryStorage::new("aaa:wrong", &wrong).unwrap())
    };
    let mut host = CollapsedLocalWorkcell::new(make()).unwrap();
    let demand = storage_demand();
    let world = host.prepare(&demand).unwrap();
    assert_eq!(world.subjects, demand.subjects);
    let binding = &world.binding_graph.bindings[0];
    assert_eq!(binding.properties["path"], now.display().to_string());
    fs::write(now.join("permitted-output"), "task bytes").unwrap();
    drop(host);
    let mut restarted = CollapsedLocalWorkcell::new(make()).unwrap();
    restarted.register_world(world.clone()).unwrap();
    assert_eq!(
        restarted.observe(&world.world_ref).unwrap().observations[0].state,
        HealthState::Healthy
    );
    let recovered = restarted.recover(&world.world_ref).unwrap();
    assert_eq!(recovered.world_ref, world.world_ref);
    assert_eq!(recovered.subjects, world.subjects);
    restarted.release(&world.world_ref).unwrap();
    assert_eq!(
        fs::read_to_string(now.join("authored.md")).unwrap(),
        "unaltered authored bytes"
    );
    assert!(now.join("permitted-output").is_file());
    fs::remove_dir_all(root).unwrap();
}
#[test]
fn unknown_readonly_exclusive_capacity_and_path_drift_are_not_promised() {
    let root = root("refusal");
    let now = root.join("NOW");
    fs::create_dir(&now).unwrap();
    let mut host = CollapsedLocalWorkcell::new(config(&root, &now)).unwrap();
    for variant in 0..4 {
        let mut d = storage_demand();
        match variant {
            0 => d.storage.required[0].logical_ref = "undeclared".into(),
            1 => d.storage.required[0].access = StorageAccess::ReadOnly,
            2 => d.storage.required[0].sharing = StorageSharing::Exclusive,
            _ => {
                d.storage.required[0].minimum_capacity = Some(10);
                d.storage.required[0].unit = Some("bytes".into());
            }
        }
        assert_eq!(host.plan(&d).unwrap().status, PlanStatus::Unsatisfiable);
        assert!(host.prepare(&d).is_err());
    }
    let world = host.prepare(&storage_demand()).unwrap();
    fs::rename(&now, root.join("old-NOW")).unwrap();
    fs::create_dir(&now).unwrap();
    assert!(host
        .observe(&world.world_ref)
        .unwrap()
        .observations
        .iter()
        .all(|o| o.state != HealthState::Healthy));
    assert!(host.recover(&world.world_ref).is_err());
    host.release(&world.world_ref).unwrap();
    assert!(root.join("old-NOW").is_dir());
    fs::remove_dir_all(root).unwrap();
}
#[cfg(unix)]
#[test]
fn actual_managed_child_recovery_changes_material_not_subjects_and_release_stops_it() {
    use std::{net::TcpListener, process::Command, thread, time::Duration};
    let root = root("service");
    let now = root.join("NOW");
    fs::create_dir(&now).unwrap();
    let port = TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port();
    let service=ManagedHostService::new("gateway:opaque",format!("tcp://127.0.0.1:{port}"),"python3").unwrap()
        .with_arg("-c").with_arg("import socket,time,sys; s=socket.socket(); s.setsockopt(socket.SOL_SOCKET,socket.SO_REUSEADDR,1); s.bind(('127.0.0.1',int(sys.argv[1]))); s.listen(); time.sleep(60)").with_arg(port.to_string())
        .with_tcp_readiness(TcpEndpointProbe::new("127.0.0.1",port).unwrap());
    let mut host = CollapsedLocalWorkcell::new(
        config(&root, &now)
            .with_persistent_host_lifetime()
            .with_managed_service(service),
    )
    .unwrap();
    let mut demand = storage_demand();
    demand
        .connectivity
        .required
        .push(LogicalConnectionRequirement::new("gateway:opaque").unwrap());
    let world = host.prepare(&demand).unwrap();
    let old = world
        .binding_graph
        .bindings
        .iter()
        .find(|b| b.port == ProviderPortKind::Service)
        .unwrap();
    let old_pid = old.properties["pid"].clone();
    assert!(Command::new("kill")
        .args(["-KILL", &old_pid])
        .status()
        .unwrap()
        .success());
    thread::sleep(Duration::from_millis(25));
    let recovered = host.recover(&world.world_ref).unwrap();
    assert_ne!(recovered.world_ref, world.world_ref);
    assert_eq!(recovered.subjects, world.subjects);
    let new = recovered
        .binding_graph
        .bindings
        .iter()
        .find(|b| b.port == ProviderPortKind::Service)
        .unwrap();
    assert_ne!(new.material_ref, old.material_ref);
    assert_ne!(new.properties["pid"], old_pid);
    assert!(host.release(&world.world_ref).is_err());
    assert!(now.is_dir());
    assert_eq!(
        host.observe(&recovered.world_ref)
            .unwrap()
            .observations
            .iter()
            .filter(|o| o.state == HealthState::Healthy)
            .count(),
        2
    );
    host.release(&recovered.world_ref).unwrap();
    assert!(std::net::TcpStream::connect(("127.0.0.1", port)).is_err());
    assert!(now.is_dir());
    drop(host);
    fs::remove_dir_all(root).unwrap();
}
