#![cfg(target_os = "macos")]

use epilogos_workcell_runtime::{capture_bounded_process, WriteBoundaryRequirements};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::{
    fs,
    path::{Path, PathBuf},
    process::{Command, Stdio},
    time::{Duration, SystemTime, UNIX_EPOCH},
};

fn persist_capture(root: &Path, label: &str, stdout: &[u8], stderr: &[u8], observation: &Value) {
    // Attempt every receipt before reporting a persistence failure. These are
    // controlled test-owned bytes; no provider or credential is invoked here.
    let writes = [
        (
            "stdout",
            fs::write(root.join(format!("{label}.stdout")), stdout),
        ),
        (
            "stderr",
            fs::write(root.join(format!("{label}.stderr")), stderr),
        ),
        (
            "observation",
            fs::write(
                root.join(format!("{label}.observation.json")),
                observation.to_string(),
            ),
        ),
    ];
    for (kind, result) in writes {
        result.unwrap_or_else(|error| panic!("cannot retain {label} {kind}: {error}"));
    }
}

fn capture(
    root: &Path,
    label: &str,
    args: &[String],
) -> epilogos_workcell_runtime::BoundedProcessOutput {
    let mut command = Command::new(env!("CARGO_BIN_EXE_workcell-write-boundary"));
    command
        .args(args)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    match capture_bounded_process(command, Duration::from_secs(10), 65_536) {
        Ok(output) => {
            persist_capture(
                root,
                label,
                &output.stdout,
                &output.stderr,
                &json!({
                    "schema":"workcell.native-runtime-admission-capture/v1",
                    "exit_code":output.status.code(), "status_success":output.status.success(),
                    "timed_out":output.timed_out, "output_complete":output.output_complete,
                    "output_truncated":output.output_truncated,
                    "stdout_bytes":output.stdout.len(), "stderr_bytes":output.stderr.len()
                }),
            );
            assert!(!output.timed_out && output.output_complete && !output.output_truncated);
            output
        }
        Err(failure) => {
            persist_capture(
                root,
                label,
                failure.stdout(),
                failure.stderr(),
                &failure.observation(),
            );
            panic!("{label} bounded capture failed after retaining actual observation: {failure}");
        }
    }
}

#[test]
fn real_ordinary_exec_survives_and_unsupported_runtime_refuses_before_body() {
    let base = PathBuf::from(
        std::env::var_os("WORKCELL_TEST_ARTIFACT_ROOT")
            .expect("native Mac gate requires an admitted artifact root"),
    );
    assert!(base.is_absolute() && base.is_dir());
    let base = fs::canonicalize(base).unwrap();
    let root = base.join(format!(
        "native-runtime-admission-{}-{}",
        std::process::id(),
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    fs::create_dir(&root).unwrap();
    let input = root.join("original-input");
    let task = root.join("task");
    let protected_source = root.join("protected-source");
    fs::create_dir(&input).unwrap();
    fs::create_dir(&task).unwrap();
    fs::create_dir(&protected_source).unwrap();
    let protected_marker = protected_source.join("source.txt");
    fs::write(&protected_marker, b"OWNED_PROTECTED_SOURCE").unwrap();
    fs::write(input.join("auth.json"), b"OWNED_NONSECRET_IMMUTABLE_INPUT").unwrap();
    let raw = json!({"schema":"workcell.write-boundary/v1", "policy_ref":"test-owner:actual-runtime-admission",
        "policy_revision":"native-runtime-admission-1", "authority_ref":"test-owner:isolated-native-exec",
        "writable_paths":[task], "protected_paths":[protected_source], "required_coverage":["file-content"],
        "expires_at_unix_ms":SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_millis() as u64 + 60_000});
    let requirements = root.join("requirements.json");
    fs::write(&requirements, raw.to_string()).unwrap();
    let digest = WriteBoundaryRequirements::from_json(&raw.to_string())
        .unwrap()
        .digest();
    let caps = capture(&root, "capabilities", &["capabilities".into()]);
    assert!(caps.status.success());
    let capabilities: Value = serde_json::from_slice(&caps.stdout).unwrap();
    assert_eq!(
        capabilities["runtime_projection"]["schema"],
        "workcell.runtime-projection-capabilities/v1"
    );
    assert_eq!(capabilities["runtime_projection"]["implemented"], false);
    let ordinary_marker = task.join("ordinary-marker");
    let prefix = [
        requirements.display().to_string(),
        "native-runtime-admission-1".into(),
        digest.clone(),
    ];
    let mut ordinary = vec!["exec".into()];
    ordinary.extend(prefix.clone());
    ordinary.extend([
        "--".into(),
        "/bin/sh".into(),
        "-c".into(),
        r#"printf owned-ordinary-exec > "$1" || exit 90; if (printf forbidden-source-write > "$2") 2>/dev/null; then exit 91; fi"#.into(),
        "native-owned-body".into(),
        ordinary_marker.display().to_string(),
        protected_marker.display().to_string(),
    ]);
    let positive = capture(&root, "ordinary", &ordinary);
    assert!(positive.status.success());
    assert_eq!(fs::read(ordinary_marker).unwrap(), b"owned-ordinary-exec");
    assert_eq!(
        fs::read(&protected_marker).unwrap(),
        b"OWNED_PROTECTED_SOURCE"
    );
    let projection = json!({"schema":"workcell.runtime-projection/v1", "requested_input_root":input,
        "input_root":input, "runtime_root":task.join("native-runtime"), "immutable_members":["auth.json"],
        "mutable_directories":["tmp"], "mutable_files":["history.jsonl"], "boundary_digest":digest});
    let projection_text = projection.to_string();
    let projection_file = root.join("projection.json");
    fs::write(&projection_file, &projection_text).unwrap();
    let rejected_marker = task.join("rejected-marker");
    let mut selected = vec!["exec-runtime".into()];
    selected.extend(prefix);
    selected.extend([
        projection_file.display().to_string(),
        format!("sha256:{:x}", Sha256::digest(projection_text.as_bytes())),
        "--".into(),
        "/bin/sh".into(),
        "-c".into(),
        r#"printf forbidden-selected-body > "$1""#.into(),
        "native-owned-body".into(),
        rejected_marker.display().to_string(),
    ]);
    let rejected = capture(&root, "selected", &selected);
    assert_eq!(rejected.status.code(), Some(2));
    let failure: Value = serde_json::from_slice(&rejected.stderr).unwrap();
    assert_eq!(failure["executed"], false);
    assert_eq!(failure["runtime_projection"]["phase"], "platform");
    assert_eq!(
        failure["runtime_projection"]["material_setup_started"],
        false
    );
    assert!(!rejected_marker.exists());
    assert!(!task.join("native-runtime").exists());
    assert_eq!(
        fs::read(protected_marker).unwrap(),
        b"OWNED_PROTECTED_SOURCE"
    );
    assert_eq!(
        fs::read(input.join("auth.json")).unwrap(),
        b"OWNED_NONSECRET_IMMUTABLE_INPUT"
    );
}
