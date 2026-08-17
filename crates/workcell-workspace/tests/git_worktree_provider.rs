mod common;

use std::{
    fs,
    path::{Path, PathBuf},
    process::Command,
    time::{SystemTime, UNIX_EPOCH},
};

use epilogos_workcell_core::{
    Availability, DemandRef, ExternalRef, HealthState, ProviderPort, ProviderRef,
    ReleaseDisposition, RetentionExpectation, WorkspaceAccess, WorkspaceMaterialRequest,
    WorkspaceMaterialSource, WorkspaceProvider,
};
use epilogos_workcell_workspace::GitWorktreeWorkspaceProvider;

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

fn git(repository: &Path, args: &[&str]) -> String {
    let output = Command::new("git")
        .arg("-C")
        .arg(repository)
        .args(args)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "git {:?} failed: {}",
        args,
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8_lossy(&output.stdout).trim().to_owned()
}

fn repository() -> (PathBuf, String) {
    let path = temp_path("git-source");
    fs::create_dir_all(&path).unwrap();
    git(&path, &["init"]);
    git(&path, &["config", "user.email", "workcell@example.invalid"]);
    git(&path, &["config", "user.name", "Workcell Fixture"]);
    git(&path, &["config", "commit.gpgsign", "false"]);
    fs::write(path.join("hello.txt"), "committed\n").unwrap();
    git(&path, &["add", "hello.txt"]);
    git(&path, &["commit", "-m", "fixture"]);
    let commit = git(&path, &["rev-parse", "HEAD"]);
    (path, commit)
}

fn request(repository: &Path, revision: &str) -> WorkspaceMaterialRequest {
    request_with_access(repository, revision, WorkspaceAccess::Writable)
}

fn request_with_access(
    repository: &Path,
    revision: &str,
    access: WorkspaceAccess,
) -> WorkspaceMaterialRequest {
    WorkspaceMaterialRequest {
        demand_ref: DemandRef::new("demand:git-worktree").unwrap(),
        source: Some(ExternalRef::new("client:source:git-fixture").unwrap()),
        material_source: Some(WorkspaceMaterialSource {
            locator: repository.display().to_string(),
            provenance: Default::default(),
        }),
        revision: Some(revision.into()),
        access,
        persistence: None,
        retention: RetentionExpectation::Release,
    }
}

#[test]
fn git_provider_materialises_exact_revision_and_tracks_dirty_state() {
    let (repository, commit) = repository();
    fs::write(repository.join("hello.txt"), "uncommitted source change\n").unwrap();
    let root = temp_path("git-worktree-root");
    let mut provider = GitWorktreeWorkspaceProvider::new(
        ProviderRef::new("provider:git-worktree-test").unwrap(),
        &root,
    );

    assert_eq!(
        provider.offers().unwrap()[0].availability,
        Availability::Available
    );
    let req = request(&repository, &commit);
    let allocation = common::assert_workspace_provider_basics(&mut provider, &req);
    assert_eq!(allocation.provenance.get("source_dirty").unwrap(), "true");
    assert_eq!(
        allocation.provenance.get("source_ref").unwrap(),
        "client:source:git-fixture"
    );
    assert_eq!(allocation.provenance.get("source_commit").unwrap(), &commit);

    let path = PathBuf::from(allocation.properties.get("path").unwrap());
    assert_eq!(
        fs::read_to_string(path.join("hello.txt")).unwrap(),
        "committed\n"
    );
    assert_eq!(
        provider
            .observe_workspace(&allocation)
            .unwrap()
            .detail
            .get("dirty")
            .unwrap(),
        "false"
    );

    fs::write(path.join("hello.txt"), "worktree change\n").unwrap();
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

    git(&path, &["reset", "--hard", "HEAD"]);
    let released = provider
        .release_workspace(&allocation, &RetentionExpectation::Release)
        .unwrap();
    assert_eq!(released.disposition, ReleaseDisposition::Released);

    let rematerialised = provider.prepare_workspace(&req).unwrap();
    assert_eq!(allocation.material_ref, rematerialised.material_ref);
    provider
        .release_workspace(&rematerialised, &RetentionExpectation::Release)
        .unwrap();

    let _ = fs::remove_dir_all(&root);
    let _ = fs::remove_dir_all(&repository);
}

#[test]
fn git_provider_reports_stale_revision_and_deleted_worktree() {
    let (repository, commit) = repository();
    let root = temp_path("git-worktree-stale-root");
    let mut provider = GitWorktreeWorkspaceProvider::new(
        ProviderRef::new("provider:git-worktree-stale").unwrap(),
        &root,
    );

    assert!(provider
        .prepare_workspace(&request(&repository, "revision-that-does-not-exist"))
        .is_err());

    let allocation = provider
        .prepare_workspace(&request(&repository, &commit))
        .unwrap();
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
    let _ = fs::remove_dir_all(&repository);
}

#[test]
fn git_provider_enforces_readonly_workspace_and_can_release_it() {
    let (repository, commit) = repository();
    let root = temp_path("git-worktree-readonly-root");
    let mut provider = GitWorktreeWorkspaceProvider::new(
        ProviderRef::new("provider:git-worktree-readonly").unwrap(),
        &root,
    );
    let request = request_with_access(&repository, &commit, WorkspaceAccess::ReadOnly);
    let allocation = common::assert_workspace_provider_basics(&mut provider, &request);
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

    let released = provider
        .release_workspace(&allocation, &RetentionExpectation::Release)
        .unwrap();
    assert_eq!(released.disposition, ReleaseDisposition::Released);
    assert!(!path.exists());

    let _ = fs::remove_dir_all(&root);
    let _ = fs::remove_dir_all(&repository);
}
