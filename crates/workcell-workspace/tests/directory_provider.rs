mod common;

use std::{
    fs,
    path::{Path, PathBuf},
    time::{SystemTime, UNIX_EPOCH},
};

use epilogos_workcell_core::{
    DemandRef, ExternalRef, HealthState, ProviderRef, ReleaseDisposition, RetentionExpectation,
    WorkspaceAccess, WorkspaceMaterialRequest, WorkspaceMaterialSource, WorkspaceProvider,
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

fn source_tree() -> PathBuf {
    let source = temp_path("directory-source");
    fs::create_dir_all(source.join("nested")).unwrap();
    fs::write(source.join("hello.txt"), "source\n").unwrap();
    fs::write(source.join("nested/value.txt"), "nested\n").unwrap();
    source
}

fn request(source: &Path, access: WorkspaceAccess) -> WorkspaceMaterialRequest {
    WorkspaceMaterialRequest {
        demand_ref: DemandRef::new("demand:directory-workspace").unwrap(),
        source: Some(ExternalRef::new("client:source:directory-fixture").unwrap()),
        material_source: Some(WorkspaceMaterialSource {
            locator: source.display().to_string(),
            provenance: Default::default(),
        }),
        revision: Some("fixture-revision".into()),
        access,
        persistence: None,
        retention: RetentionExpectation::Release,
    }
}

#[test]
fn directory_provider_tracks_dirty_retention_and_rematerialisation() {
    let source = source_tree();
    let root = temp_path("directory-root");
    let mut provider = DirectoryWorkspaceProvider::new(
        ProviderRef::new("provider:directory-test").unwrap(),
        &root,
    );
    let request = request(&source, WorkspaceAccess::Writable);
    let allocation = common::assert_workspace_provider_basics(&mut provider, &request);

    assert_eq!(
        allocation.provenance.get("source_ref").unwrap(),
        "client:source:directory-fixture"
    );
    assert_eq!(
        allocation.provenance.get("source_revision").unwrap(),
        "fixture-revision"
    );
    let path = PathBuf::from(allocation.properties.get("path").unwrap());
    assert_eq!(
        fs::read_to_string(path.join("hello.txt")).unwrap(),
        "source\n"
    );

    let repeated = provider.prepare_workspace(&request).unwrap();
    assert_eq!(repeated.material_ref, allocation.material_ref);

    fs::write(path.join("hello.txt"), "dirty\n").unwrap();
    assert_eq!(
        provider
            .observe_workspace(&allocation)
            .unwrap()
            .detail
            .get("dirty")
            .unwrap(),
        "true"
    );
    assert!(provider
        .release_workspace(&allocation, &RetentionExpectation::Release)
        .is_err());

    fs::write(path.join("hello.txt"), "source\n").unwrap();
    assert_eq!(
        provider
            .observe_workspace(&allocation)
            .unwrap()
            .detail
            .get("dirty")
            .unwrap(),
        "false"
    );
    let preserved = provider
        .release_workspace(&allocation, &RetentionExpectation::Preserve)
        .unwrap();
    assert_eq!(preserved.disposition, ReleaseDisposition::Preserved);
    assert!(path.exists());
    assert_eq!(
        provider.observe_workspace(&allocation).unwrap().health,
        HealthState::Healthy
    );

    let released = provider
        .release_workspace(&allocation, &RetentionExpectation::Release)
        .unwrap();
    assert_eq!(released.disposition, ReleaseDisposition::Released);
    assert!(released.changed);
    assert!(!path.exists());

    let rematerialised = provider.prepare_workspace(&request).unwrap();
    assert_eq!(rematerialised.material_ref, allocation.material_ref);
    assert_eq!(
        rematerialised.provenance.get("source_ref").unwrap(),
        "client:source:directory-fixture"
    );
    provider
        .release_workspace(&rematerialised, &RetentionExpectation::Release)
        .unwrap();

    let _ = fs::remove_dir_all(&root);
    let _ = fs::remove_dir_all(&source);
}

#[test]
fn directory_provider_enforces_readonly_and_reports_missing_material() {
    let source = source_tree();
    let root = temp_path("directory-readonly-root");
    let mut provider = DirectoryWorkspaceProvider::new(
        ProviderRef::new("provider:directory-readonly").unwrap(),
        &root,
    );
    let readonly_request = request(&source, WorkspaceAccess::ReadOnly);
    let allocation = common::assert_workspace_provider_basics(&mut provider, &readonly_request);
    let path = PathBuf::from(allocation.properties.get("path").unwrap());

    assert!(fs::metadata(path.join("hello.txt"))
        .unwrap()
        .permissions()
        .readonly());
    assert!(fs::metadata(&path).unwrap().permissions().readonly());
    #[cfg(unix)]
    {
        assert!(fs::write(path.join("hello.txt"), "should fail\n").is_err());
        assert!(fs::write(path.join("new.txt"), "should fail\n").is_err());
    }

    provider
        .release_workspace(&allocation, &RetentionExpectation::Release)
        .unwrap();
    assert!(!path.exists());

    let missing = temp_path("directory-missing-source");
    let mut missing_request = request(&missing, WorkspaceAccess::Writable);
    missing_request.demand_ref = DemandRef::new("demand:directory-missing").unwrap();
    assert!(provider.prepare_workspace(&missing_request).is_err());

    let writable_request = request(&source, WorkspaceAccess::Writable);
    let allocation = provider.prepare_workspace(&writable_request).unwrap();
    let path = PathBuf::from(allocation.properties.get("path").unwrap());
    fs::remove_dir_all(&path).unwrap();
    let observation = provider.observe_workspace(&allocation).unwrap();
    assert_eq!(observation.health, HealthState::Unavailable);
    assert_eq!(observation.detail.get("exists").unwrap(), "false");
    let released = provider
        .release_workspace(&allocation, &RetentionExpectation::Release)
        .unwrap();
    assert!(!released.changed);

    let _ = fs::remove_dir_all(&root);
    let _ = fs::remove_dir_all(&source);
}
