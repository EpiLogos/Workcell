use std::{
    fs,
    path::PathBuf,
    time::{SystemTime, UNIX_EPOCH},
};

use epilogos_workcell_core::{
    DemandRef, ProviderRef, RetentionExpectation, WorkspaceAccess, WorkspaceMaterialRequest,
    WorkspaceMaterialSource, WorkspaceProvider,
};
use epilogos_workcell_workspace::DirectoryWorkspaceProvider;

fn temp_path(label: &str) -> PathBuf {
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    std::env::temp_dir().join(format!(
        "epilogos-workcell-{label}-{}-{nonce}",
        std::process::id()
    ))
}

#[test]
fn directory_workspace_reconstructs_from_persisted_allocation_after_restart() {
    let source = temp_path("workspace-restart-source");
    let root = temp_path("workspace-restart-root");
    fs::create_dir_all(&source).unwrap();
    fs::write(source.join("source.txt"), "stable\n").unwrap();

    let provider_ref = ProviderRef::new("provider:workspace-restart").unwrap();
    let request = WorkspaceMaterialRequest {
        demand_ref: DemandRef::new("demand:workspace-restart").unwrap(),
        source: None,
        material_source: Some(WorkspaceMaterialSource {
            locator: source.display().to_string(),
            provenance: Default::default(),
        }),
        revision: None,
        access: WorkspaceAccess::Writable,
        persistence: None,
        retention: RetentionExpectation::Release,
    };
    let mut first = DirectoryWorkspaceProvider::new(provider_ref.clone(), &root);
    let allocation = first.prepare_workspace(&request).unwrap();
    let path = PathBuf::from(allocation.properties.get("path").unwrap());
    assert!(allocation.provenance.contains_key("baseline_fingerprint"));
    drop(first);

    let mut restarted = DirectoryWorkspaceProvider::new(provider_ref, &root);
    let observation = restarted.observe_workspace(&allocation).unwrap();
    assert_eq!(
        observation.detail.get("dirty").map(String::as_str),
        Some("false")
    );
    restarted
        .release_workspace(&allocation, &RetentionExpectation::Release)
        .unwrap();
    assert!(!path.exists());

    let _ = fs::remove_dir_all(source);
    let _ = fs::remove_dir_all(root);
}
