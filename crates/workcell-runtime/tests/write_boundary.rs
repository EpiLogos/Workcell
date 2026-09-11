use epilogos_workcell_runtime::*;
use std::{
    fs,
    path::{Path, PathBuf},
    process::Command,
    time::{SystemTime, UNIX_EPOCH},
};
fn root() -> PathBuf {
    let p = std::env::temp_dir().join(format!(
        "workcell-boundary-{}-{}",
        std::process::id(),
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    fs::create_dir(&p).unwrap();
    p
}
fn requirements(root: &Path) -> WriteBoundaryRequirements {
    WriteBoundaryRequirements {
        policy_ref: "source:policy".into(),
        policy_revision: "revision:1".into(),
        authority_ref: "authority:1".into(),
        writable_paths: vec![root.join("Work/NOW"), root.join("Work/project")],
        protected_paths: vec![root.join("Work")],
        required_coverage: WRITE_BOUNDARY_COVERAGE
            .iter()
            .map(|s| s.to_string())
            .collect(),
        expires_at_unix_ms: u64::MAX,
    }
}
#[test]
fn stale_expired_or_unsupported_requirements_fail_before_any_program() {
    let root = root();
    let mut r = requirements(&root);
    assert!(r.validate("revision:2").is_err());
    r.expires_at_unix_ms = 1;
    assert!(r.validate("revision:1").is_err());
    r.expires_at_unix_ms = u64::MAX;
    r.required_coverage
        .push("metadata-chmod-chown-xattr-time".into());
    assert!(r.validate("revision:1").is_err());
    let bad = r.as_json().to_string();
    assert!(WriteBoundaryRequirements::from_json(&bad).is_err());
    fs::remove_dir_all(root).unwrap();
}
#[test]
fn native_kernel_write_canaries_or_explicit_unsupported_refusal() {
    let root = root();
    fs::create_dir_all(root.join("Work/NOW")).unwrap();
    fs::create_dir(root.join("Work/project")).unwrap();
    fs::write(root.join("Work/human"), "protected").unwrap();
    let r = requirements(&root);
    let caps = write_boundary_capabilities();
    if caps["supported"] != true {
        assert!(PreparedWriteBoundary::prepare(r, "revision:1").is_err());
        eprintln!("WRITE_BOUNDARY_UNSUPPORTED: {caps}; positive enforcement NOT executed");
        fs::remove_dir_all(root).unwrap();
        assert_ne!(
            std::env::var("WORKCELL_REQUIRE_LANDLOCK").ok().as_deref(),
            Some("1"),
            "required positive Landlock campaign did not run"
        );
        return;
    }
    let boundary = PreparedWriteBoundary::prepare(r, "revision:1").unwrap();
    let mut command = Command::new("python3");
    command.args(["-c",r#"
import os,sys,subprocess,json
from pathlib import Path
r=Path(sys.argv[1]); n=r/'Work/NOW'; h=r/'Work/human'; denied=[]
(n/'permitted').write_text('ok'); (r/'Work/project/source').write_text('engineering is permitted')
def deny(name, action):
    try: action()
    except PermissionError: denied.append(name)
    except OSError as e:
        if e.errno == 18: denied.append(name)
        else: raise
    else: raise AssertionError('write escaped: '+name)
deny('root-create',lambda:(r/'Work/scratch').mkdir())
deny('content',lambda:h.write_text('overwrite'))
deny('truncate',lambda:os.truncate(h,0))
deny('remove',lambda:h.unlink())
deny('rename',lambda:os.rename(h,n/'stolen'))
deny('link',lambda:os.link(h,n/'alias'))
deny('cwd-traversal',lambda:(n/'../escape').write_text('escape'))
os.symlink(h,n/'symlink'); deny('symlink',lambda:(n/'symlink').write_text('escape'))
p=subprocess.run(['/bin/sh','-c','printf escape > "$1"','child',str(r/'Work/child-escape')],capture_output=True)
assert p.returncode != 0; denied.append('descendant')
assert h.read_text()=='protected'; print(json.dumps({'executed_denials':denied,'allowed_writes':2}))
"#, root.to_str().unwrap()]);
    boundary
        .configure_command(&mut command, "revision:1")
        .unwrap();
    let output = command.output().unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let report: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(report["executed_denials"].as_array().unwrap().len(), 9);
    assert_eq!(
        fs::read_to_string(root.join("Work/human")).unwrap(),
        "protected"
    );
    eprintln!("WRITE_BOUNDARY_EXECUTED: {report}");
    fs::rename(root.join("Work/NOW"), root.join("Work/old-NOW")).unwrap();
    fs::create_dir(root.join("Work/NOW")).unwrap();
    let mut command = Command::new("true");
    assert!(boundary
        .configure_command(&mut command, "revision:1")
        .is_err());
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn protected_source_files_retain_identity_without_broadening_writable_grants() {
    let root = root();
    fs::create_dir_all(root.join("Work/NOW")).unwrap();
    fs::create_dir(root.join("Work/project")).unwrap();
    let source = root.join("now.json");
    fs::write(&source, "retained NOW source").unwrap();
    let mut r = requirements(&root);
    r.protected_paths.push(source.clone());
    let caps = write_boundary_capabilities();
    if caps["supported"] != true {
        let error = PreparedWriteBoundary::prepare(r, "revision:1").unwrap_err();
        assert!(
            !error.to_string().contains("existing directory"),
            "valid protected source file was refused before platform eligibility: {error}"
        );
        eprintln!("PROTECTED_SOURCE_PLATFORM_UNAVAILABLE: {caps}; no execution claimed");
        fs::remove_dir_all(root).unwrap();
        assert_ne!(
            std::env::var("WORKCELL_REQUIRE_LANDLOCK").ok().as_deref(),
            Some("1")
        );
        return;
    }
    let boundary = PreparedWriteBoundary::prepare(r, "revision:1").unwrap();
    let inspected = boundary.inspect("revision:1").unwrap();
    assert!(inspected["protected_objects"]
        .as_array()
        .unwrap()
        .iter()
        .any(|p| p["path"].as_str() == source.to_str() && p["identity"].as_str().is_some()));
    let mut command = Command::new("python3");
    command.args(["-c", "import sys; from pathlib import Path; p=Path(sys.argv[1]);\ntry: p.write_text('escape')\nexcept PermissionError: print('protected-source-denied')\nelse: raise AssertionError('protected source overwritten')", source.to_str().unwrap()]);
    boundary
        .configure_command(&mut command, "revision:1")
        .unwrap();
    let output = command.output().unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(fs::read_to_string(&source).unwrap(), "retained NOW source");
    fs::rename(&source, root.join("retained-now.json")).unwrap();
    fs::write(&source, "replacement").unwrap();
    assert!(boundary.inspect("revision:1").is_err());
    eprintln!("PROTECTED_SOURCE_EXECUTED: denial and identity-drift refusal");
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn protected_file_under_writable_directory_is_not_silently_dropped() {
    let root = root();
    fs::create_dir_all(root.join("Work/NOW")).unwrap();
    fs::create_dir(root.join("Work/project")).unwrap();
    let source = root.join("Work/NOW/protected.json");
    fs::write(&source, "retained").unwrap();
    let mut r = requirements(&root);
    r.protected_paths.push(source.clone());
    let error = PreparedWriteBoundary::prepare(r, "revision:1").unwrap_err();
    assert!(
        error
            .to_string()
            .contains("writable directory includes a protected object"),
        "{error}"
    );
    assert_eq!(fs::read_to_string(source).unwrap(), "retained");
    fs::remove_dir_all(root).unwrap();
}
