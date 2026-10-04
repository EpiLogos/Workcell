//! Real material/namespace cases. Mandatory selected Linux execution must not
//! treat unsupported kernel/owner input as a passing positive case.
#![cfg(target_os = "linux")]
use epilogos_workcell_runtime::{
    capture_bounded_process, RuntimeProjection, WriteBoundaryRequirements, WRITE_BOUNDARY_COVERAGE,
};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::os::unix::fs::{MetadataExt, PermissionsExt};
use std::{
    fs,
    path::{Path, PathBuf},
    process::{Command, Stdio},
    time::{Duration, SystemTime, UNIX_EPOCH},
};

// Replay coordinates describe current material ownership, never a build checkout.
// Expected image/source values come from the original qualified compiler and
// held-image receipts; this agreement check does not confer native authority.
fn replay_digest_input(name: &str) -> std::io::Result<String> {
    let invalid = || {
        std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "explicit replay requires the exact admitted source/image digest",
        )
    };
    let value = std::env::var(name).map_err(|_| invalid())?;
    if value.len() != 64 || !value.bytes().all(|value| value.is_ascii_hexdigit()) {
        return Err(invalid());
    }
    Ok(value)
}

fn replay_image(
    path: &Path,
    context: &Path,
    expected: &str,
    actual_self: bool,
    deadline: std::time::Instant,
) -> std::io::Result<()> {
    use std::io::Read;
    use std::os::unix::fs::OpenOptionsExt;
    let invalid = || {
        std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "explicit replay image does not match the admitted compiled owner basis",
        )
    };
    if !path.is_absolute() || !path.starts_with(context) || path.canonicalize()? != path {
        return Err(invalid());
    }
    let mut file = fs::OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK | libc::O_CLOEXEC)
        .open(path)?;
    let initial = file.metadata()?;
    let identity = |metadata: &fs::Metadata| {
        (
            metadata.dev(),
            metadata.ino(),
            metadata.len(),
            metadata.mode(),
            metadata.nlink(),
            metadata.uid(),
            metadata.mtime(),
            metadata.mtime_nsec(),
            metadata.ctime(),
            metadata.ctime_nsec(),
        )
    };
    let named = fs::symlink_metadata(path)?;
    const LIMIT: u64 = 1_073_741_824;
    if !initial.is_file()
        || !named.is_file()
        || initial.mode() & 0o111 == 0
        || initial.uid() != unsafe { libc::geteuid() }
        || initial.len() > LIMIT
        || identity(&initial) != identity(&named)
    {
        return Err(invalid());
    }
    let is_actual_self = || -> std::io::Result<bool> {
        let actual = fs::metadata("/proc/self/exe")?;
        Ok((actual.dev(), actual.ino()) == (initial.dev(), initial.ino()))
    };
    if actual_self && !is_actual_self()? {
        return Err(invalid());
    }
    let mut digest = Sha256::new();
    let mut buffer = [0_u8; 65_536];
    let mut observed = 0_u64;
    loop {
        if std::time::Instant::now() >= deadline {
            return Err(std::io::Error::new(
                std::io::ErrorKind::TimedOut,
                "explicit replay image observation deadline elapsed",
            ));
        }
        let count = file.read(&mut buffer)?;
        if count == 0 {
            break;
        }
        observed += count as u64;
        if observed > LIMIT {
            return Err(invalid());
        }
        digest.update(&buffer[..count]);
    }
    let named = fs::symlink_metadata(path)?;
    if std::time::Instant::now() >= deadline {
        return Err(std::io::Error::new(
            std::io::ErrorKind::TimedOut,
            "explicit replay image observation deadline elapsed",
        ));
    }
    if !named.is_file()
        || identity(&initial) != identity(&file.metadata()?)
        || identity(&initial) != identity(&named)
        || observed != initial.len()
        || path.canonicalize()? != path
        || format!("{:x}", digest.finalize()) != expected
        || (actual_self && !is_actual_self()?)
    {
        return Err(invalid());
    }
    Ok(())
}

fn admitted_artifact_root() -> std::io::Result<PathBuf> {
    use std::io::Read;
    use std::os::unix::fs::{MetadataExt, OpenOptionsExt};
    let invalid = |message| std::io::Error::new(std::io::ErrorKind::InvalidInput, message);
    let input = |name| {
        std::env::var_os(name).ok_or_else(|| {
            invalid(format!(
                "required artifact admission input {name} is absent"
            ))
        })
    };
    let selected = PathBuf::from(input("WORKCELL_TEST_ARTIFACT_ROOT")?);
    let source = PathBuf::from(input("WORKCELL_TEST_ARTIFACT_OWNER_SOURCE")?);
    if !selected.is_absolute() || !source.is_absolute() {
        return Err(invalid(
            "artifact placement and owner source must be absolute".into(),
        ));
    }
    let base = selected.canonicalize()?;
    let base_metadata = fs::metadata(&base)?;
    if !base_metadata.is_dir() || base_metadata.uid() != unsafe { libc::geteuid() } {
        return Err(invalid(
            "artifact root must be an existing coordinator-owned directory".into(),
        ));
    }
    let owner_ref = std::env::var("WORKCELL_TEST_ARTIFACT_OWNER_REF")
        .map_err(|_| invalid("artifact owner reference is missing or not UTF-8".into()))?;
    let admission = std::env::var("WORKCELL_TEST_ARTIFACT_ADMISSION")
        .map_err(|_| invalid("artifact admission kind is missing or not UTF-8".into()))?;
    let mut file = fs::OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK | libc::O_CLOEXEC)
        .open(&source)?;
    let initial = file.metadata()?;
    if !initial.is_file() || initial.nlink() != 1 || initial.len() > 65_536 {
        return Err(invalid(
            "artifact owner source must be a bounded regular single-link file".into(),
        ));
    }
    let mut bytes = Vec::new();
    (&mut file).take(65_537).read_to_end(&mut bytes)?;
    if bytes.len() > 65_536 {
        return Err(invalid(
            "artifact owner source exceeded the admission byte limit".into(),
        ));
    }
    let identity = |m: &fs::Metadata| {
        (
            m.dev(),
            m.ino(),
            m.len(),
            m.mtime(),
            m.mtime_nsec(),
            m.ctime(),
            m.ctime_nsec(),
        )
    };
    let named = fs::symlink_metadata(&source)?;
    if !named.is_file()
        || identity(&initial) != identity(&file.metadata()?)
        || identity(&initial) != identity(&named)
        || source.canonicalize()? != source
    {
        return Err(invalid(
            "artifact owner source changed or is not the selected canonical regular source".into(),
        ));
    }
    let text = std::str::from_utf8(&bytes)
        .map_err(|error| std::io::Error::new(std::io::ErrorKind::InvalidData, error))?;
    let explicit_context = std::env::var_os("WORKCELL_TEST_CONTEXT_ROOT");
    const REPLAY_INPUTS: [&str; 5] = [
        "WORKCELL_TEST_REPLAY_SOURCE_REF",
        "WORKCELL_TEST_REPLAY_TEST_SOURCE_SHA256",
        "WORKCELL_TEST_REPLAY_LOCK_SHA256",
        "WORKCELL_TEST_REPLAY_TEST_IMAGE_SHA256",
        "WORKCELL_TEST_REPLAY_OWNER_IMAGE_SHA256",
    ];
    if explicit_context.is_none()
        && REPLAY_INPUTS
            .iter()
            .any(|name| std::env::var_os(name).is_some())
    {
        return Err(invalid(
            "explicit replay has no actual owner context".into(),
        ));
    }
    let (repository, context_witness) = if let Some(context) = explicit_context {
        if admission == "hosted-runner" {
            return Err(invalid(
                "hosted staging cannot substitute an explicit replay owner context".into(),
            ));
        }
        let context = PathBuf::from(context);
        if !context.is_absolute() || context.canonicalize()? != context {
            return Err(invalid(
                "explicit replay context must be an existing canonical absolute owner".into(),
            ));
        }
        let held = fs::OpenOptions::new()
            .read(true)
            .custom_flags(libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_NONBLOCK | libc::O_CLOEXEC)
            .open(&context)?;
        let initial = held.metadata()?;
        let named = fs::symlink_metadata(&context)?;
        if !initial.is_dir()
            || !named.is_dir()
            || (initial.dev(), initial.ino()) != (named.dev(), named.ino())
        {
            return Err(invalid(
                "explicit replay owner affiliation is unavailable".into(),
            ));
        }
        (context, Some((held, initial)))
    } else {
        (
            PathBuf::from(env!("CARGO_MANIFEST_DIR"))
                .parent()
                .and_then(Path::parent)
                .ok_or_else(|| invalid("missing compiled product repository".into()))?
                .canonicalize()?,
            None,
        )
    };
    let mut central_context = None;
    for ancestor in repository.ancestors() {
        match fs::metadata(ancestor.join("Control/user/native-action-authority.json")) {
            Ok(metadata) if metadata.is_file() => {
                central_context = Some(ancestor.to_path_buf());
                break;
            }
            Ok(_) => {
                return Err(invalid(
                    "compiled Central context marker is not a regular file".into(),
                ))
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(error),
        }
    }
    match admission.as_str() {
        "central-clearing" => {
            let record: serde_json::Value = serde_json::from_slice(&bytes)
                .map_err(|error| std::io::Error::new(std::io::ErrorKind::InvalidData, error))?;
            let clearing = source
                .parent()
                .ok_or_else(|| invalid("missing clearing owner".into()))?;
            let central = clearing
                .ancestors()
                .nth(5)
                .ok_or_else(|| invalid("missing Central root owner".into()))?;
            let id = clearing
                .file_name()
                .and_then(|value| value.to_str())
                .ok_or_else(|| invalid("native clearing identity is not UTF-8".into()))?;
            let expected = central
                .join("Control/agents/now/clearings")
                .join(id)
                .join("now.json");
            let source_ref =
                format!("central:source:control:root:Control/agents/now/clearings/{id}/now.json");
            if context_witness.is_some() && repository != central {
                return Err(invalid(
                    "explicit replay context is not the selected actual clearing World".into(),
                ));
            }
            // An explicit replay already holds the exact native clearing World
            // above; private grant files do not establish portable World identity.
            if context_witness.is_none() && central_context.as_deref() != Some(central) {
                return Err(invalid(
                    "clearing source is outside the actual compiled Central working context".into(),
                ));
            }
            if source != expected
                || owner_ref != format!("central:now:control:root:{id}")
                || record["schema"] != "central.now-clearing/v1"
                || record["now_ref"] != owner_ref
                || record["source_ref"] != source_ref
                || record["scope_ref"] != "control:root"
                || !record["policy_revision_at_allocation"]
                    .as_str()
                    .is_some_and(|value| !value.is_empty())
                || !record["participant_refs"]
                    .as_array()
                    .is_some_and(|values| !values.is_empty())
            {
                return Err(invalid(
                    "selected owner source does not establish the actual root clearing allocation"
                        .into(),
                ));
            }
            if !base.starts_with(clearing.join("T").canonicalize()?) {
                return Err(invalid(
                    "artifact root is outside the selected actual clearing T".into(),
                ));
            }
        }
        "product-scratch" => {
            let record: serde_json::Value = serde_json::from_slice(&bytes)
                .map_err(|error| std::io::Error::new(std::io::ErrorKind::InvalidData, error))?;
            let project = source
                .parent()
                .and_then(Path::parent)
                .ok_or_else(|| invalid("missing declared product owner".into()))?;
            let id = record["project_id"]
                .as_str()
                .ok_or_else(|| invalid("declared product has no native project identity".into()))?;
            let actual_product_context = match &central_context {
                Some(central) => project.parent() == Some(central.join("Work").as_path()),
                None => project == repository,
            };
            if context_witness.is_some()
                && project != repository
                && central_context.as_deref() != Some(repository.as_path())
            {
                return Err(invalid(
                    "explicit context is not its actual Project or World".into(),
                ));
            }
            if !actual_product_context {
                return Err(invalid(
                    "product scratch source is outside the actual compiled product owner context"
                        .into(),
                ));
            }
            if record["schema"] != "central.project/v1"
                || id.is_empty()
                || record["human_source"] != "ProjectCentral/user"
                || owner_ref != format!("project:{id}")
                || source != project.join("ProjectCentral/project.json")
                || !base.starts_with(project.join("ProjectCentral/now/tmp").canonicalize()?)
            {
                return Err(invalid(
                    "artifact root does not match the selected actual product scratch owner".into(),
                ));
            }
            // This is authored product scratch, not proof of a Run allocation.
        }
        "hosted-runner" => {
            let workspace = PathBuf::from(input("GITHUB_WORKSPACE")?).canonicalize()?;
            if std::env::var("GITHUB_ACTIONS").as_deref() != Ok("true")
                || std::env::var("CI").as_deref() != Ok("true")
                || !std::env::var("GITHUB_RUN_ID").is_ok_and(|value| !value.is_empty())
                || !std::env::var("GITHUB_REPOSITORY").is_ok_and(|value| !value.is_empty())
                || workspace != repository
                || base != workspace.join("evidence/native-processes").canonicalize()?
                || source != workspace.join("evidence/source-commit.txt")
                || text.trim() != owner_ref
                || owner_ref.len() != 40
                || !owner_ref.bytes().all(|value| value.is_ascii_hexdigit())
            {
                return Err(invalid("artifact root is not the explicitly admitted source-pinned hosted staging exception".into()));
            }
            // CI staging is not a native Project, World, Run or NOW identity.
        }
        _ => return Err(invalid("unrecognised artifact admission kind".into())),
    }
    if let Some((held, context_initial)) = context_witness {
        let expected_source = std::env::var("WORKCELL_TEST_REPLAY_SOURCE_REF")
            .map_err(|_| invalid("explicit replay source association is absent".into()))?;
        let compiled_source = option_env!("WORKCELL_TEST_COMPILED_SOURCE_REF")
            .ok_or_else(|| invalid("image lacks compiled source association".into()))?;
        if expected_source.len() != 40
            || !expected_source
                .bytes()
                .all(|value| value.is_ascii_hexdigit())
            || expected_source != compiled_source
        {
            return Err(invalid("explicit replay source association differs".into()));
        }
        let expected_test_source = replay_digest_input("WORKCELL_TEST_REPLAY_TEST_SOURCE_SHA256")?;
        let expected_lock = replay_digest_input("WORKCELL_TEST_REPLAY_LOCK_SHA256")?;
        if format!(
            "{:x}",
            Sha256::digest(include_bytes!("runtime_projection.rs"))
        ) != expected_test_source
            || format!(
                "{:x}",
                Sha256::digest(include_bytes!("../../../Cargo.lock"))
            ) != expected_lock
        {
            return Err(invalid(
                "explicit replay compiled source/lock basis differs".into(),
            ));
        }
        let expected_test = replay_digest_input("WORKCELL_TEST_REPLAY_TEST_IMAGE_SHA256")?;
        let expected_owner = replay_digest_input("WORKCELL_TEST_REPLAY_OWNER_IMAGE_SHA256")?;
        let deadline = std::time::Instant::now() + Duration::from_secs(30);
        replay_image(
            &std::env::current_exe()?,
            &repository,
            &expected_test,
            true,
            deadline,
        )?;
        let owner = PathBuf::from(input("WORKCELL_RUNTIME_PROJECTION_BIN")?);
        replay_image(&owner, &repository, &expected_owner, false, deadline)?;
        // Image observation may take time. Requalify the SAME held owner source
        // before any fixture allocation; changed/missing source never falls back.
        let owner_named = fs::symlink_metadata(&source)?;
        if !owner_named.is_file()
            || owner_named.nlink() != 1
            || identity(&initial) != identity(&file.metadata()?)
            || identity(&initial) != identity(&owner_named)
            || source.canonicalize()? != source
        {
            return Err(invalid("actual replay owner source changed".into()));
        }
        let named = fs::symlink_metadata(&repository)?;
        let current = held.metadata()?;
        if !named.is_dir()
            || (context_initial.dev(), context_initial.ino()) != (named.dev(), named.ino())
            || (context_initial.dev(), context_initial.ino()) != (current.dev(), current.ino())
            || repository.canonicalize()? != repository
        {
            return Err(invalid("explicit replay owner affiliation changed".into()));
        }
    }
    Ok(base)
}

struct Fixture {
    root: PathBuf,
}
impl Fixture {
    fn new() -> Self {
        assert_ne!(
            unsafe { libc::geteuid() },
            0,
            "actual unprivileged native projection prerequisite"
        );
        let base = admitted_artifact_root()
            .expect("actual existing owner allocation/source admission is required");
        let root = base.join(format!(
            "runtime-projection-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir(&root).unwrap();
        for name in ["input", "task"] {
            fs::create_dir(root.join(name)).unwrap();
        }
        fs::write(
            root.join("input/auth.txt"),
            b"CONTROLLED_INPUT_NOT_A_CREDENTIAL",
        )
        .unwrap();
        fs::write(
            root.join("input/config.txt"),
            b"CONTROLLED_CONFIG_UNCHANGED",
        )
        .unwrap();
        fs::create_dir(root.join("input/sessions")).unwrap();
        fs::write(
            root.join("input/sessions/history.jsonl"),
            b"controlled-old-history\n",
        )
        .unwrap();
        Self { root }
    }
    fn requirements(&self) -> WriteBoundaryRequirements {
        WriteBoundaryRequirements {
            policy_ref: "controlled-material-policy".into(),
            policy_revision: "controlled-revision".into(),
            authority_ref: "controlled-material-test".into(),
            writable_paths: vec![self.root.join("task")],
            protected_paths: vec![],
            required_coverage: WRITE_BOUNDARY_COVERAGE
                .iter()
                .map(|s| s.to_string())
                .collect(),
            expires_at_unix_ms: u64::MAX,
        }
    }
    fn projection(&self) -> Value {
        json!({"schema":"workcell.runtime-projection/v1","requested_input_root":self.root.join("input"),"input_root":self.root.join("input"),"runtime_root":self.root.join("task/native-runtime"),
            "immutable_members":["auth.txt","config.txt"],"mutable_directories":["sessions","log","tmp"],"mutable_files":["installation_id"],"boundary_digest":self.requirements().digest()})
    }
    fn execute(
        &self,
        label: &str,
        script: &str,
    ) -> epilogos_workcell_runtime::BoundedProcessOutput {
        self.execute_projection(label, script, self.projection())
    }
    fn execute_projection(
        &self,
        label: &str,
        script: &str,
        selected: Value,
    ) -> epilogos_workcell_runtime::BoundedProcessOutput {
        let binary = PathBuf::from(
            std::env::var_os("WORKCELL_RUNTIME_PROJECTION_BIN")
                .expect("same-source built native owner executable is mandatory"),
        );
        assert!(binary.is_absolute() && binary.is_file());
        let requirements = self.root.join(format!("{label}-requirements.json"));
        let projection = self.root.join(format!("{label}-projection.json"));
        fs::write(&requirements, self.requirements().as_json().to_string()).unwrap();
        let raw = selected.to_string();
        fs::write(&projection, &raw).unwrap();
        let mut command = Command::new(&binary);
        command
            .args([
                "exec-runtime",
                requirements.to_str().unwrap(),
                "controlled-revision",
                &self.requirements().digest(),
                projection.to_str().unwrap(),
                &format!("sha256:{:x}", Sha256::digest(raw.as_bytes())),
                "--",
                "python3",
                "-c",
                script,
            ])
            .arg(&self.root)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        let output = match capture_bounded_process(command, Duration::from_secs(20), 1_048_576) {
            Ok(output) => output,
            Err(failure) => {
                fs::write(self.root.join(format!("{label}.stdout")), failure.stdout()).unwrap();
                fs::write(self.root.join(format!("{label}.stderr")), failure.stderr()).unwrap();
                fs::write(self.root.join(format!("{label}-outcome.json")),json!({
                    "failure":failure.observation(),"error":failure.to_string(),"binary":binary,
                    "projection_path":projection,"projection_sha256":format!("sha256:{:x}",Sha256::digest(raw.as_bytes()))
                }).to_string()).unwrap();
                panic!("same native finite process owner capture failed; retained raw evidence: {failure}");
            }
        };
        fs::write(self.root.join(format!("{label}.stdout")), &output.stdout).unwrap();
        fs::write(self.root.join(format!("{label}.stderr")), &output.stderr).unwrap();
        fs::write(self.root.join(format!("{label}-outcome.json")),json!({"status":output.status.code(),"timed_out":output.timed_out,"output_complete":output.output_complete,"output_truncated":output.output_truncated,"binary":binary}).to_string()).unwrap();
        assert!(
            !output.timed_out && output.output_complete && !output.output_truncated,
            "actual complete bounded outcome required"
        );
        output
    }
    fn retained(&self) {
        assert_eq!(
            fs::read(self.root.join("input/auth.txt")).unwrap(),
            b"CONTROLLED_INPUT_NOT_A_CREDENTIAL"
        );
        assert_eq!(
            fs::read(self.root.join("input/config.txt")).unwrap(),
            b"CONTROLLED_CONFIG_UNCHANGED"
        );
        assert_eq!(
            fs::read(self.root.join("input/sessions/history.jsonl")).unwrap(),
            b"controlled-old-history\n"
        );
    }
}
// Owned artifacts remain under the admitted owner root for raw qualification;
// these tests do not sweep old or failed native runtime material.

#[test]
#[ignore = "requires actual unprivileged Linux namespace/overlay/Landlock and same-source native owner"]
fn actual_original_input_is_readonly_and_task_history_reentry_is_durable() {
    let f = Fixture::new();
    let initial = fs::metadata(f.root.join("input/auth.txt")).unwrap();
    let script = r#"import os,sys,json,errno
from pathlib import Path
r=Path(sys.argv[1]); h=r/'input'; t=r/'task'; denied=[]
def refuse(name,fn):
    try: fn()
    except OSError as e:
        assert e.errno in (errno.EACCES,errno.EPERM,errno.EROFS);denied.append(name)
    else: raise AssertionError('escaped readonly input: '+name)
refuse('auth',lambda:(h/'auth.txt').write_text('changed'))
refuse('config',lambda:(h/'config.txt').write_text('changed'))
refuse('new-input',lambda:(h/'unknown-input').write_text('changed'))
refuse('skeleton',lambda:next((t/'native-runtime').glob('view-*/readonly-members')).joinpath('auth.txt').write_text('changed'))
assert (h/'sessions/history.jsonl').read_text().startswith('controlled-old-history\n')
with (h/'sessions/history.jsonl').open('a') as s: s.write('controlled-continuation\n')
(h/'log/native.log').write_text('actual native material');(h/'tmp/native.tmp').write_text('actual tmp material')
p=h/'installation_id'
if not p.read_text():p.write_text('controlled-runtime-id')
os.chmod(p,0o644)
assert p.read_text()=='controlled-runtime-id'
status=Path('/proc/self/status').read_text()
for line in status.splitlines():
    if line.startswith(('CapEff:','CapPrm:','CapAmb:','CapBnd:')):assert int(line.split()[1],16)==0
assert os.getuid()==int(os.environ['EXPECTED_NATIVE_UID'])
print(json.dumps({'denied':denied,'history_lines':len((h/'sessions/history.jsonl').read_text().splitlines()),'runtime_id':p.read_text()}))
"#;
    // Exact uid is nonsecret material context, not a fabricated Session.
    // Supply it in the real script rather than mutate process-wide test env.
    let script = script.replace(
        "int(os.environ['EXPECTED_NATIVE_UID'])",
        &unsafe { libc::geteuid() }.to_string(),
    );
    let first = f.execute("first", &script);
    assert!(
        first.status.success(),
        "{}",
        String::from_utf8_lossy(&first.stderr)
    );
    let first: Value = serde_json::from_slice(&first.stdout).unwrap();
    assert_eq!(first["history_lines"], 2);
    assert_eq!(first["denied"].as_array().unwrap().len(), 4);
    let runtime_id = fs::read(
        f.root
            .join("task/native-runtime/files/installation_id/upper/installation_id"),
    )
    .unwrap();
    let second = f.execute("reentry", &script);
    assert!(
        second.status.success(),
        "{}",
        String::from_utf8_lossy(&second.stderr)
    );
    let second: Value = serde_json::from_slice(&second.stdout).unwrap();
    assert_eq!(second["history_lines"], 3);
    assert_eq!(second["runtime_id"], first["runtime_id"]);
    assert_eq!(
        fs::read(
            f.root
                .join("task/native-runtime/files/installation_id/upper/installation_id")
        )
        .unwrap(),
        runtime_id
    );
    f.retained();
    let final_metadata = fs::metadata(f.root.join("input/auth.txt")).unwrap();
    assert_eq!(
        (
            final_metadata.dev(),
            final_metadata.ino(),
            final_metadata.mtime(),
            final_metadata.mtime_nsec()
        ),
        (
            initial.dev(),
            initial.ino(),
            initial.mtime(),
            initial.mtime_nsec()
        )
    );
}

#[test]
#[ignore = "requires actual native owner and admitted Linux filesystem fixture"]
fn actual_redirected_runtime_never_writes_the_foreign_directory() {
    let f = Fixture::new();
    let foreign = f.root.join("foreign");
    fs::create_dir(&foreign).unwrap();
    fs::write(foreign.join("retained"), b"FOREIGN_UNCHANGED").unwrap();
    std::os::unix::fs::symlink(&foreign, f.root.join("task/native-runtime")).unwrap();
    let out=f.execute("redirect", "from pathlib import Path;import sys;Path(sys.argv[1]).joinpath('task/body').write_text('ran')");
    assert!(!out.status.success());
    let error: Value = serde_json::from_slice(&out.stderr).unwrap();
    assert_eq!(error["runtime_projection"]["phase"], "material-setup");
    assert_eq!(
        error["runtime_projection"]["cause"]["raw_os_error"],
        libc::ENOTDIR
    );
    assert_eq!(
        fs::read(foreign.join("retained")).unwrap(),
        b"FOREIGN_UNCHANGED"
    );
    assert_eq!(fs::read_dir(foreign).unwrap().count(), 1);
    assert_eq!(
        fs::symlink_metadata(f.root.join("task/body"))
            .unwrap_err()
            .kind(),
        std::io::ErrorKind::NotFound
    );
    f.retained();
}

#[test]
#[ignore = "requires actual native owner and admitted Linux filesystem fixture"]
fn actual_immutable_input_redirect_refuses_before_material_or_body() {
    let f = Fixture::new();
    fs::remove_file(f.root.join("input/auth.txt")).unwrap();
    let foreign = f.root.join("foreign-auth");
    fs::write(&foreign, b"FOREIGN_NOT_A_CREDENTIAL").unwrap();
    std::os::unix::fs::symlink(&foreign, f.root.join("input/auth.txt")).unwrap();
    let out=f.execute("input-redirect","from pathlib import Path;import sys;Path(sys.argv[1]).joinpath('task/body').write_text('ran')");
    assert!(!out.status.success());
    let error: Value = serde_json::from_slice(&out.stderr).unwrap();
    assert_eq!(error["runtime_projection"]["phase"], "origin-admission");
    assert_eq!(error["runtime_projection"]["material_setup_started"], false);
    assert_eq!(fs::read(&foreign).unwrap(), b"FOREIGN_NOT_A_CREDENTIAL");
    assert_eq!(fs::read_dir(f.root.join("task")).unwrap().count(), 0);
}

#[test]
#[ignore = "requires actual native owner and nonroot readonly filesystem admission"]
fn actual_readonly_runtime_file_retains_native_permission_cause() {
    let f = Fixture::new();
    let files = f
        .root
        .join("task/native-runtime/files/installation_id/upper");
    for p in [
        f.root.join("task/native-runtime"),
        f.root.join("task/native-runtime/files"),
        f.root.join("task/native-runtime/files/installation_id"),
        files.clone(),
    ] {
        fs::create_dir(&p).unwrap();
        fs::set_permissions(&p, fs::Permissions::from_mode(0o700)).unwrap();
    }
    let retained = files.join("installation_id");
    fs::write(&retained, b"retained-runtime-id").unwrap();
    fs::set_permissions(&retained, fs::Permissions::from_mode(0o444)).unwrap();
    let before = fs::metadata(&retained).unwrap();
    let out=f.execute("readonly","from pathlib import Path;import sys;Path(sys.argv[1]).joinpath('task/body').write_text('ran')");
    assert!(!out.status.success());
    let error: Value = serde_json::from_slice(&out.stderr).unwrap();
    assert_eq!(error["runtime_projection"]["phase"], "material-setup");
    assert_eq!(
        error["runtime_projection"]["cause"]["kind"],
        "PermissionDenied"
    );
    assert_eq!(
        error["runtime_projection"]["cause"]["raw_os_error"],
        libc::EACCES
    );
    assert_eq!(error["runtime_projection"]["executed"], false);
    assert_eq!(fs::read(&retained).unwrap(), b"retained-runtime-id");
    let after = fs::metadata(&retained).unwrap();
    assert_eq!(
        (after.dev(), after.ino(), after.mode()),
        (before.dev(), before.ino(), before.mode())
    );
    f.retained();
    // Keep the actual readonly refusal material in the admitted evidence root.
}

#[test]
#[ignore = "requires admitted actual Linux filesystem; no provider or fabricated errno"]
fn actual_projection_fifo_hardlink_and_digest_refuse_without_blocking_read() {
    let f = Fixture::new();
    let fifo = f.root.join("fifo");
    let native = std::ffi::CString::new(fifo.to_str().unwrap()).unwrap();
    assert_eq!(unsafe { libc::mkfifo(native.as_ptr(), 0o600) }, 0);
    let start = std::time::Instant::now();
    let error = RuntimeProjection::read(&fifo, "sha256:not-a-native-digest").unwrap_err();
    assert!(start.elapsed() < Duration::from_secs(1));
    assert_eq!(error.as_json()["phase"], "request");
    let path = f.root.join("request.json");
    let raw = f.projection().to_string();
    fs::write(&path, &raw).unwrap();
    let digest = format!("sha256:{:x}", Sha256::digest(raw.as_bytes()));
    fs::hard_link(&path, f.root.join("alias.json")).unwrap();
    assert!(RuntimeProjection::read(&path, &digest).is_err());
    fs::remove_file(f.root.join("alias.json")).unwrap();
    assert!(RuntimeProjection::read(&path, "sha256:wrong").is_err());
    assert_eq!(
        RuntimeProjection::read(&path, &digest).unwrap().input_root,
        f.root.join("input")
    );
    let mut conflict = f.projection();
    conflict["mutable_files"] = json!(["auth.txt"]);
    assert!(RuntimeProjection::from_json(&conflict.to_string()).is_err());
    f.retained();
}

#[test]
#[ignore = "requires actual unprivileged Linux namespace/overlay/Landlock and same-source native owner"]
fn actual_unchanged_requested_input_alias_preserves_original_origin() {
    let f = Fixture::new();
    let requested = f.root.join("provider-home");
    std::os::unix::fs::symlink(f.root.join("input"), &requested).unwrap();
    let mut selected = f.projection();
    selected["requested_input_root"] = json!(&requested);
    let before = fs::metadata(f.root.join("input/auth.txt")).unwrap();
    let out=f.execute_projection("unchanged-alias",r#"from pathlib import Path
import sys,errno,json
r=Path(sys.argv[1]);h=r/'provider-home'
assert h.resolve()==r/'input'
assert (h/'auth.txt').read_bytes()==b'CONTROLLED_INPUT_NOT_A_CREDENTIAL'
try:(h/'auth.txt').write_bytes(b'changed')
except OSError as e:assert e.errno in (errno.EACCES,errno.EPERM,errno.EROFS)
else:raise AssertionError('original input became writable')
with (h/'sessions/history.jsonl').open('a') as s:s.write('alias-continuation\n')
print(json.dumps({'original_route_retained':True,'history_lines':len((h/'sessions/history.jsonl').read_text().splitlines())}))
"#,selected);
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let body: Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(body["original_route_retained"], true);
    assert_eq!(body["history_lines"], 2);
    let after = fs::metadata(f.root.join("input/auth.txt")).unwrap();
    assert_eq!(
        (
            before.dev(),
            before.ino(),
            before.mode(),
            before.mtime(),
            before.mtime_nsec()
        ),
        (
            after.dev(),
            after.ino(),
            after.mode(),
            after.mtime(),
            after.mtime_nsec()
        )
    );
    assert_eq!(fs::read_link(&requested).unwrap(), f.root.join("input"));
    f.retained();
}

#[test]
#[ignore = "requires same-source native owner and admitted actual Linux filesystem fixture"]
fn actual_requested_input_alias_retarget_refuses_before_setup() {
    let f = Fixture::new();
    let requested = f.root.join("provider-home");
    let foreign = f.root.join("foreign-home");
    fs::create_dir(&foreign).unwrap();
    fs::write(foreign.join("auth.txt"), b"FOREIGN_INPUT_UNCHANGED").unwrap();
    std::os::unix::fs::symlink(f.root.join("input"), &requested).unwrap();
    let mut selected = f.projection();
    selected["requested_input_root"] = json!(&requested);
    // Retain the real original object A and the exact selected request while
    // redirecting only its original invocation alias to distinct actual B.
    let held = fs::File::open(f.root.join("input")).unwrap();
    let original = held.metadata().unwrap();
    let before = fs::metadata(foreign.join("auth.txt")).unwrap();
    fs::remove_file(&requested).unwrap();
    std::os::unix::fs::symlink(&foreign, &requested).unwrap();
    assert_eq!(requested.canonicalize().unwrap(), foreign);
    let out=f.execute_projection("retargeted-alias","from pathlib import Path;import sys;Path(sys.argv[1]).joinpath('task/body').write_text('ran')",selected);
    assert!(!out.status.success());
    let error: Value = serde_json::from_slice(&out.stderr).unwrap();
    assert_eq!(error["runtime_projection"]["phase"], "origin-admission");
    assert_eq!(error["runtime_projection"]["cause"]["kind"], "InvalidInput");
    assert_eq!(error["runtime_projection"]["material_setup_started"], false);
    assert_eq!(error["runtime_projection"]["executed"], false);
    assert_eq!(fs::read_dir(f.root.join("task")).unwrap().count(), 0);
    f.retained();
    let named = fs::metadata(f.root.join("input")).unwrap();
    assert_eq!((original.dev(), original.ino()), (named.dev(), named.ino()));
    let after = fs::metadata(foreign.join("auth.txt")).unwrap();
    assert_eq!(
        (
            before.dev(),
            before.ino(),
            before.mode(),
            before.mtime(),
            before.mtime_nsec()
        ),
        (
            after.dev(),
            after.ino(),
            after.mode(),
            after.mtime(),
            after.mtime_nsec()
        )
    );
    assert_eq!(
        fs::read(foreign.join("auth.txt")).unwrap(),
        b"FOREIGN_INPUT_UNCHANGED"
    );
    assert_eq!(fs::read_dir(&foreign).unwrap().count(), 1);
}

#[test]
#[ignore = "requires same-source native owner and admitted actual Linux filesystem fixture"]
fn actual_missing_requested_input_retains_resolution_cause() {
    let f = Fixture::new();
    let requested = f.root.join("provider-home");
    std::os::unix::fs::symlink(f.root.join("input"), &requested).unwrap();
    let mut selected = f.projection();
    selected["requested_input_root"] = json!(&requested);
    fs::remove_file(&requested).unwrap();
    assert_eq!(
        requested.canonicalize().unwrap_err().raw_os_error(),
        Some(libc::ENOENT)
    );
    let out=f.execute_projection("missing-alias","from pathlib import Path;import sys;Path(sys.argv[1]).joinpath('task/body').write_text('ran')",selected);
    assert!(!out.status.success());
    let error: Value = serde_json::from_slice(&out.stderr).unwrap();
    assert_eq!(error["runtime_projection"]["phase"], "origin-admission");
    assert_eq!(error["runtime_projection"]["cause"]["kind"], "NotFound");
    assert_eq!(
        error["runtime_projection"]["cause"]["raw_os_error"],
        libc::ENOENT
    );
    assert_eq!(error["runtime_projection"]["material_setup_started"], false);
    assert_eq!(error["runtime_projection"]["executed"], false);
    assert_eq!(fs::read_dir(f.root.join("task")).unwrap().count(), 0);
    f.retained();
}
