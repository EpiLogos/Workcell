use epilogos_workcell_runtime::*;
use std::{
    fs,
    path::PathBuf,
    process::Command,
    time::{SystemTime, UNIX_EPOCH},
};

fn root() -> PathBuf {
    let root = std::env::temp_dir().join(format!(
        "workcell-protected-boundary-{}-{}",
        std::process::id(),
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    fs::create_dir_all(root.join("Work/project/src")).unwrap();
    fs::create_dir_all(root.join("Control/user")).unwrap();
    fs::create_dir_all(root.join("Control/agents/task/T")).unwrap();
    fs::write(root.join("Control/user/policy.json"), "human source").unwrap();
    fs::write(root.join("Control/agents/task/now.json"), "NOW source").unwrap();
    root
}

fn requirements(root: &std::path::Path) -> WriteBoundaryRequirements {
    WriteBoundaryRequirements {
        policy_ref: "central:source:policy".into(),
        policy_revision: "exact-policy-revision".into(),
        authority_ref: "authority:task".into(),
        writable_paths: vec![
            root.join("Work/project/src"),
            root.join("Control/agents/task/T"),
        ],
        protected_paths: vec![
            root.join("Control/user/policy.json"),
            root.join("Control/agents/task/now.json"),
            root.join("future-protected.json"),
        ],
        required_coverage: WRITE_BOUNDARY_COVERAGE
            .iter()
            .map(|item| item.to_string())
            .collect(),
        expires_at_unix_ms: u64::MAX,
    }
}

#[test]
fn protected_file_beneath_writable_directory_is_not_silently_dropped() {
    let root = root();
    let mut request = requirements(&root);
    request
        .protected_paths
        .push(root.join("Work/project/src/human.txt"));
    let error = PreparedWriteBoundary::prepare(request, "exact-policy-revision").unwrap_err();
    assert!(error.to_string().contains("includes a protected path"));
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn exact_files_and_absent_paths_are_protected_in_native_execution() {
    let root = root();
    let request = requirements(&root);
    let caps = write_boundary_capabilities();
    if caps["supported"] != true {
        assert!(PreparedWriteBoundary::prepare(request, "exact-policy-revision").is_err());
        fs::remove_dir_all(root).unwrap();
        assert_ne!(
            std::env::var("WORKCELL_REQUIRE_LANDLOCK").ok().as_deref(),
            Some("1"),
            "required protected-file execution unavailable: {caps}"
        );
        return;
    }
    let boundary = PreparedWriteBoundary::prepare(request.clone(), "exact-policy-revision").unwrap();
    let inspection = boundary.inspect("exact-policy-revision").unwrap();
    assert_eq!(inspection["requirements"], request.as_json());
    assert_eq!(inspection["protected_objects"].as_array().unwrap().len(), 3);
    let mut command = Command::new("python3");
    command.args([
        "-c",
        r#"
import sys
from pathlib import Path
root = Path(sys.argv[1])
(root/'Work/project/src/result.txt').write_text('permitted source edit')
(root/'Control/agents/task/T/result.txt').write_text('permitted artifact')
for path in ['Control/user/policy.json', 'Control/agents/task/now.json', 'future-protected.json']:
    try:
        (root/path).write_text('must not write')
    except PermissionError:
        pass
    else:
        raise AssertionError('protection lost: ' + path)
assert (root/'Control/user/policy.json').read_text() == 'human source'
assert (root/'Control/agents/task/now.json').read_text() == 'NOW source'
print('PROTECTED_PATHS_EXECUTED: three denied writes and two permitted writes')
"#,
        root.to_str().unwrap(),
    ]);
    boundary
        .configure_command(&mut command, "exact-policy-revision")
        .unwrap();
    let output = command.output().unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    eprintln!("{}", String::from_utf8_lossy(&output.stdout));
    let original = root.join("Control/user/policy.json");
    fs::rename(&original, root.join("Control/user/previous.json")).unwrap();
    fs::write(&original, "changed source object").unwrap();
    assert!(boundary.revalidate("exact-policy-revision").is_err());
    fs::remove_dir_all(root).unwrap();
}
