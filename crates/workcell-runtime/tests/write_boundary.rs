use epilogos_workcell_runtime::*;
use std::{
    fs,
    path::{Path, PathBuf},
    time::{SystemTime, UNIX_EPOCH},
};
fn root() -> PathBuf {
    use std::sync::atomic::{AtomicU64, Ordering};
    // Timestamps alone can collide when parallel test threads in this
    // process read the clock together; a monotonic counter keeps each
    // caller's temp root distinct (the same discipline expose_collect uses).
    static UNIQUE: AtomicU64 = AtomicU64::new(0);
    let unique = UNIQUE.fetch_add(1, Ordering::Relaxed);
    let p = std::env::temp_dir().join(format!(
        "workcell-boundary-{}-{}-{}",
        std::process::id(),
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos(),
        unique
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
    let mut command = boundary.command("python3", "revision:1").unwrap();
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
    assert!(boundary.command("true", "revision:1").is_err());
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
    let canonical_source = fs::canonicalize(&source).unwrap();
    assert!(inspected["protected_objects"]
        .as_array()
        .unwrap()
        .iter()
        .any(
            |p| p["path"].as_str() == canonical_source.to_str() && p["identity"].as_str().is_some()
        ));
    let mut command = boundary.command("python3", "revision:1").unwrap();
    command.args(["-c", "import sys; from pathlib import Path; p=Path(sys.argv[1]);\ntry: p.write_text('escape')\nexcept PermissionError: print('protected-source-denied')\nelse: raise AssertionError('protected source overwritten')", source.to_str().unwrap()]);
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

#[test]
fn missing_protected_path_under_writable_directory_is_not_silently_dropped() {
    let root = fs::canonicalize(root()).unwrap();
    fs::create_dir_all(root.join("Work/NOW")).unwrap();
    fs::create_dir(root.join("Work/project")).unwrap();
    let source = root.join("Work/NOW/future-source.json");
    let mut r = requirements(&root);
    r.protected_paths.push(source.clone());
    let error = PreparedWriteBoundary::prepare(r, "revision:1").unwrap_err();
    assert!(
        error
            .to_string()
            .contains("writable directory includes a protected object"),
        "{error}"
    );
    assert!(!source.exists());
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn missing_protected_path_requires_new_resolution_when_any_suffix_appears() {
    let root = fs::canonicalize(root()).unwrap();
    fs::create_dir_all(root.join("Work/NOW")).unwrap();
    fs::create_dir(root.join("Work/project")).unwrap();
    fs::create_dir(root.join("Control")).unwrap();
    let source = root.join("Control/ProjectCentral/source.json");
    let mut r = requirements(&root);
    r.protected_paths.push(source.clone());
    let caps = write_boundary_capabilities();
    if caps["supported"] != true {
        assert!(PreparedWriteBoundary::prepare(r, "revision:1").is_err());
        fs::remove_dir_all(root).unwrap();
        return;
    }
    let boundary = PreparedWriteBoundary::prepare(r, "revision:1").unwrap();
    let inspected = boundary.inspect("revision:1").unwrap();
    let object = inspected["protected_objects"]
        .as_array()
        .unwrap()
        .iter()
        .find(|object| object["path"].as_str() == source.to_str())
        .unwrap();
    assert_eq!(object["presence"], "missing");
    assert!(object["identity"].is_null());
    fs::create_dir(root.join("Control/ProjectCentral")).unwrap();
    assert!(boundary.inspect("revision:1").is_err());
    fs::remove_dir_all(root).unwrap();
}

#[cfg(target_os = "macos")]
#[test]
fn macos_native_escaping_root_identity_fds_and_environment() {
    use std::os::fd::AsRawFd;
    let root = root();
    fs::create_dir_all(root.join("Work/NOW")).unwrap();
    fs::create_dir(root.join("Work/project")).unwrap();
    let unusual = root.join("Work/project/quoted\"\\(allow default)λ");
    fs::create_dir(&unusual).unwrap();
    let human = root.join("Work/human");
    fs::write(&human, "protected").unwrap();
    let file = fs::OpenOptions::new().append(true).open(&human).unwrap();
    let fd = file.as_raw_fd();
    assert_eq!(unsafe { libc::fcntl(fd, libc::F_SETFD, 0) }, 0);
    let mut requirement = requirements(&root);
    requirement.writable_paths.push(unusual.clone());
    let boundary = PreparedWriteBoundary::prepare(requirement, "revision:1").unwrap();
    let mut command = boundary.command("python3", "revision:1").unwrap();
    command
        .args([
            "-B",
            "-c",
            r#"
import os,sys,json
from pathlib import Path
root=Path(sys.argv[1]); unusual=Path(sys.argv[2]); fd=int(sys.argv[3])
(unusual/'permitted').write_text('quoted-path')
def deny(fn):
    try: fn()
    except OSError as e:
        assert e.errno in (1,13), str(e)
    else: raise AssertionError('denial did not occur')
deny(lambda:(root/'escaped').write_text('profile injection'))
deny(lambda:unusual.rename(unusual.with_name('moved')))
deny(lambda:unusual.rmdir())
try: os.write(fd,b'inherited descriptor escape')
except OSError as e: assert e.errno==9, str(e)
else: raise AssertionError('inherited descriptor survived exec')
print(json.dumps({'escaped_path_denied':True,'grant_root_denied':True,'inherited_fd_closed':True}))
"#,
        ])
        .arg(&root)
        .arg(&unusual)
        .arg(fd.to_string());
    let output = command.output().unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(fs::read_to_string(&human).unwrap(), "protected");
    assert_eq!(
        fs::read_to_string(unusual.join("permitted")).unwrap(),
        "quoted-path"
    );
    let mut environment = boundary.command("/usr/bin/env", "revision:1").unwrap();
    environment
        .env_clear()
        .env("BOUNDARY_ONLY", "exact-value")
        .current_dir(&unusual);
    let output = environment.output().unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(
        String::from_utf8(output.stdout).unwrap(),
        "BOUNDARY_ONLY=exact-value\n"
    );
    // Held-out setter path: overriding stdout after protected construction must
    // not reopen the descriptor bypass, even though ordinary CLI callers pipe it.
    let denied_output = fs::OpenOptions::new().append(true).open(&human).unwrap();
    let mut redirected = boundary.command("/bin/sh", "revision:1").unwrap();
    redirected
        .args(["-c", "printf forbidden-stdio"])
        .stdout(denied_output);
    assert!(
        redirected.output().is_err(),
        "regular stdout reached the worker"
    );
    assert_eq!(fs::read_to_string(&human).unwrap(), "protected");
    // A replaced native directory fails before creating a protected Command.
    fs::rename(&unusual, root.join("retained-original")).unwrap();
    fs::create_dir(&unusual).unwrap();
    assert!(boundary.command("/usr/bin/true", "revision:1").is_err());
    eprintln!(
        "MACOS_BOUNDARY_EXECUTED: escaping, root mutation, inherited fd, late stdio override, env_clear, identity drift"
    );
    drop(file);
    fs::remove_dir_all(root).unwrap();
}

#[cfg(target_os = "macos")]
#[test]
fn macos_missing_protection_denies_creation_and_detects_external_appearance() {
    let root = fs::canonicalize(root()).unwrap();
    fs::create_dir_all(root.join("Work/NOW")).unwrap();
    fs::create_dir(root.join("Work/project")).unwrap();
    fs::create_dir(root.join("Control")).unwrap();
    let protected = root.join("Control/.central");
    let mut requirement = requirements(&root);
    requirement.protected_paths.push(protected.clone());
    let boundary = PreparedWriteBoundary::prepare(requirement, "revision:1").unwrap();
    let mut command = boundary.command("python3", "revision:1").unwrap();
    command
        .args([
            "-B",
            "-c",
            "import sys\nfrom pathlib import Path\np=Path(sys.argv[1])\ntry: p.mkdir()\nexcept OSError as e: assert e.errno in (1,13), str(e)\nelse: raise AssertionError('missing protected path was created')",
        ])
        .arg(&protected);
    let output = command.output().unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(!protected.exists());
    fs::create_dir(&protected).unwrap();
    assert!(boundary.command("/usr/bin/true", "revision:1").is_err());
    eprintln!("MACOS_MISSING_PROTECTION_EXECUTED: creation denied and appearance drift refused");
    fs::remove_dir_all(root).unwrap();
}

#[cfg(target_os = "macos")]
#[test]
fn macos_unrepresentable_profile_path_fails_closed() {
    let root = root();
    fs::create_dir_all(root.join("Work/NOW")).unwrap();
    fs::create_dir(root.join("Work/project")).unwrap();
    let control = root.join("Work/project/line\nbreak");
    fs::create_dir(&control).unwrap();
    let mut requirement = requirements(&root);
    requirement.writable_paths.push(control);
    assert!(PreparedWriteBoundary::prepare(requirement, "revision:1")
        .unwrap_err()
        .to_string()
        .contains("control characters"));
    fs::remove_dir_all(root).unwrap();
}
