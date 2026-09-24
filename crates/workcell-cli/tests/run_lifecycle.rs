//! The one-run-per-rung acceptance, local rung (spec acceptance 1–3):
//! start a worktree run → the branch law materialises `aikit/<slug>` →
//! collect shows the branch tip as deliverable → a dirty release is refused
//! into run `blocked` (never deletion) → a clean release closes the run as
//! `success`. Every `workcell` invocation below is a separate process, so
//! the record and the git worktree journal surviving "restart" is proven by
//! construction at each step.

use std::{
    fs,
    path::{Path, PathBuf},
    process::{Command, Output},
    time::{SystemTime, UNIX_EPOCH},
};

use serde_json::Value;

fn binary() -> &'static str {
    env!("CARGO_BIN_EXE_workcell")
}

fn temp_path(label: &str) -> PathBuf {
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    std::env::temp_dir().join(format!(
        "epilogos-workcell-run-lifecycle-{label}-{}-{nonce}",
        std::process::id()
    ))
}

fn run(args: &[String]) -> Output {
    Command::new(binary()).args(args).output().unwrap()
}

fn stdout_json(output: &Output) -> Value {
    serde_json::from_slice(&output.stdout).unwrap_or_else(|error| {
        panic!(
            "invalid JSON stdout: {error}\nstdout={}\nstderr={}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        )
    })
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

fn fixture_repository() -> PathBuf {
    let repository = temp_path("repo");
    fs::create_dir_all(&repository).unwrap();
    git(&repository, &["init", "--template="]);
    git(
        &repository,
        &["config", "user.email", "runs@example.invalid"],
    );
    git(&repository, &["config", "user.name", "Run Fixture"]);
    git(&repository, &["config", "commit.gpgsign", "false"]);
    fs::write(repository.join("hello.txt"), "committed\n").unwrap();
    git(&repository, &["add", "hello.txt"]);
    git(&repository, &["commit", "-m", "fixture"]);
    repository
}

fn workcell(state_root: &Path, extra: &[&str], args: &[String]) -> Vec<String> {
    let mut all = vec![
        "--state-root".to_owned(),
        state_root.display().to_string(),
        "--json".to_owned(),
    ];
    all.extend(extra.iter().map(|value| value.to_string()));
    all.extend(args.iter().cloned());
    all
}

#[test]
fn a_worktree_run_lives_end_to_end_on_the_branch_law_with_dirty_refusal() {
    let root = temp_path("lifecycle");
    let state_root = root.join("state");
    let repository = fixture_repository();
    let repo_arg = repository.display().to_string();
    let slug = "lifecycle-1";

    // 1. Start: the demand carries the branch law, the run materialises a
    //    worktree on `aikit/<slug>`.
    let args = workcell(
        &state_root,
        &["--workspace-source", &repo_arg],
        &[
            "run".into(),
            "start".into(),
            "--run".into(),
            slug.into(),
            "--extension".into(),
            "branch_law=aikit".into(),
            "--extension".into(),
            format!("run_slug={slug}"),
            "--workspace".into(),
            "writable".into(),
        ],
    );
    let output = run(&args);
    assert!(output.status.success(), "start failed");
    let started = stdout_json(&output);
    assert_eq!(started["run"]["execution_status"], "running");
    assert_eq!(started["run"]["rung"], "local");
    assert!(started["run"]["canonical_run_ref"].is_null());

    // 2. The branch law: the worktree materialised on `aikit/<slug>`, not a
    //    detached checkout, and the journal knows it.
    assert!(
        git(&repository, &["branch", "--list", &format!("aikit/{slug}")])
            .contains(&format!("aikit/{slug}")),
        "the run branch must exist in the source repository"
    );
    let worktree_path = started["run"]["material_refs"]
        .as_array()
        .unwrap()
        .iter()
        .find_map(|material| {
            let reference = material.as_str()?;
            reference.strip_prefix("workspace:git-worktree:")
        })
        .map(|key| state_root.join("workspaces").join(key))
        .expect("a git-worktree material ref");
    assert!(worktree_path.exists(), "worktree must be material");
    let journal: Value = serde_json::from_str(
        &fs::read_to_string(state_root.join("workspaces").join("git-worktrees.json")).unwrap(),
    )
    .unwrap();
    assert_eq!(journal["schema"], "workcell.git-worktrees/v1");
    assert_eq!(journal["worktrees"].as_array().unwrap().len(), 1);
    assert_eq!(
        journal["worktrees"][0]["branch"],
        format!("aikit/{slug}"),
        "the journal records the branch-law worktree"
    );

    // 3. The resident's work: commit on the run branch inside the worktree.
    fs::write(worktree_path.join("work.txt"), "resident output\n").unwrap();
    git(&worktree_path, &["add", "work.txt"]);
    git(&worktree_path, &["commit", "-m", "resident work"]);
    let branch_tip = git(&worktree_path, &["rev-parse", "HEAD"]);

    // 4. Collect: the branch tip is the deliverable, status `returned`.
    let args = workcell(
        &state_root,
        &[],
        &["run".into(), "collect".into(), "--run".into(), slug.into()],
    );
    let output = run(&args);
    assert!(output.status.success(), "collect failed");
    let collected = stdout_json(&output);
    assert_eq!(collected["execution_status"], "returned");
    assert_eq!(
        collected["deliverable"]["branch"]["name"],
        format!("aikit/{slug}")
    );
    assert_eq!(collected["deliverable"]["branch"]["commit"], branch_tip);
    assert_eq!(collected["deliverable"]["branch"]["pushed"], false);

    // 5. Dirty release is refused into `blocked` — and the worktree stays.
    fs::write(worktree_path.join("dirty.txt"), "uncommitted\n").unwrap();
    let args = workcell(
        &state_root,
        &[],
        &["run".into(), "release".into(), "--run".into(), slug.into()],
    );
    let output = run(&args);
    assert!(!output.status.success(), "a dirty release must fail");
    let refused = stdout_json(&output);
    assert_eq!(refused["run"]["execution_status"], "blocked");
    assert!(
        refused["run"]["status_reason"]
            .as_str()
            .unwrap_or("")
            .contains("release refused"),
        "the refusal must name its reason"
    );
    assert!(worktree_path.exists(), "a blocked release never deletes");

    // 6. Cross-process restart: a fresh invocation still sees the record.
    let args = workcell(
        &state_root,
        &[],
        &["run".into(), "show".into(), "--run".into(), slug.into()],
    );
    let shown = stdout_json(&run(&args));
    assert_eq!(shown["run"]["run_slug"], slug);
    assert_eq!(shown["run"]["execution_status"], "blocked");

    // 7. Clean the worktree, release again: `success`, material gone.
    git(&worktree_path, &["clean", "-fdq"]);
    let args = workcell(
        &state_root,
        &[],
        &["run".into(), "release".into(), "--run".into(), slug.into()],
    );
    let output = run(&args);
    assert!(output.status.success(), "clean release failed");
    let released = stdout_json(&output);
    assert_eq!(released["run"]["execution_status"], "success");
    assert_eq!(released["run"]["release_disposition"], "released");
    assert!(!worktree_path.exists(), "released material is gone");
    let journal: Value = serde_json::from_str(
        &fs::read_to_string(state_root.join("workspaces").join("git-worktrees.json")).unwrap(),
    )
    .unwrap();
    assert_eq!(
        journal["worktrees"].as_array().unwrap().len(),
        0,
        "the journal must not claim released worktrees"
    );

    let _ = fs::remove_dir_all(root);
}

#[test]
fn scope_names_the_worktree_and_degrades_honestly_without_a_write_adapter() {
    let root = temp_path("scope");
    let state_root = root.join("state");
    let repository = fixture_repository();
    let repo_arg = repository.display().to_string();
    let slug = "scope-1";

    let args = workcell(
        &state_root,
        &["--workspace-source", &repo_arg],
        &[
            "run".into(),
            "start".into(),
            "--run".into(),
            slug.into(),
            "--extension".into(),
            "branch_law=aikit".into(),
            "--workspace".into(),
            "writable".into(),
        ],
    );
    run(&args);

    let args = workcell(
        &state_root,
        &[],
        &[
            "run".into(),
            "scope".into(),
            "--run".into(),
            slug.into(),
            "--policy-revision".into(),
            "test-policy-1".into(),
        ],
    );
    let output = run(&args);
    assert!(output.status.success(), "scope failed");
    let scoped = stdout_json(&output);
    assert_eq!(scoped["scope"]["schema"], "workcell.prepared-run-scope/v1");
    assert_eq!(scoped["scope"]["run_slug"], slug);
    assert!(
        scoped["scope"]["worktree_path"]
            .as_str()
            .unwrap()
            .contains("workspaces"),
        "the scope names the materialised worktree"
    );
    assert!(scoped["scope"]["demand_digest"]
        .as_str()
        .unwrap()
        .starts_with("sha256:"));

    // On a platform with no material write adapter the boundary is null with
    // a named degradation — never a weaker stand-in. On a platform with one
    // it is the exact prepared object with its digest.
    let boundary = &scoped["scope"]["prepared_write_boundary"];
    if boundary.is_null() {
        let degradation = &scoped["scope"]["degradations"][0];
        assert_eq!(degradation["subject_ref"], "prepared_write_boundary");
        assert_eq!(degradation["state"], "unavailable");
    } else {
        assert_eq!(boundary["schema"], "workcell.prepared-write-boundary/v1");
        assert_eq!(boundary["state"], "prepared-not-executed");
    }

    let _ = fs::remove_dir_all(root);
}
