#![cfg(unix)]

use epilogos_workcell_runtime::run_bounded_process;
use serde_json::{json, Value};
use std::{
    fs,
    path::PathBuf,
    process::{Command, Stdio},
    sync::atomic::{AtomicU64, Ordering},
    thread,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

static SEQUENCE: AtomicU64 = AtomicU64::new(0);

struct Fixture {
    root: PathBuf,
    requirements: PathBuf,
    writer_verified: std::cell::Cell<bool>,
}

impl Fixture {
    fn new(label: &str) -> Self {
        let base = PathBuf::from(std::env::var_os("WORKCELL_TEST_ARTIFACT_ROOT")
            .expect("native CLI capture gate requires an allocated artifact directory"));
        assert!(base.is_absolute() && base.is_dir());
        let base = std::fs::canonicalize(base).expect("native artifact root must resolve physically");
        let manifest_dir = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        for ancestor in manifest_dir.ancestors() {
            let clearings = ancestor.join("Control/agents/now/clearings");
            if clearings.is_dir() {
                let relative = base.strip_prefix(std::fs::canonicalize(clearings).unwrap())
                    .expect("Central native evidence must remain below an allocated clearing");
                let parts: Vec<_> = relative.components().collect();
                assert!(parts.len() >= 3 && parts[1].as_os_str() == "T",
                    "Central native evidence must be below an existing clearing T");
                break;
            }
        }
        let root = base.join(format!("{label}-{}-{}", std::process::id(),
            SEQUENCE.fetch_add(1, Ordering::Relaxed)));
        fs::create_dir(&root).unwrap();
        let root = fs::canonicalize(root).unwrap();
        let requirements = root.join("requirements.json");
        let value = json!({
            "schema":"workcell.write-boundary/v1",
            "policy_ref":"test-owner:actual-capture-policy",
            "policy_revision":"actual-capture-1",
            "authority_ref":"test-owner:isolated-native-capture",
            "writable_paths":[root],
            "protected_paths":[],
            "required_coverage":["file-content","file-creation","descendant-processes"],
            "expires_at_unix_ms":SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_millis() as u64 + 30_000
        });
        fs::write(&requirements, serde_json::to_vec(&value).unwrap()).unwrap();
        Self { root, requirements, writer_verified: std::cell::Cell::new(false) }
    }

    fn call(&self, program_args: &[String]) -> Value {
        let mut command = Command::new(env!("CARGO_BIN_EXE_workcell-write-boundary"));
        command.arg("run").arg(&self.requirements).arg("actual-capture-1")
            .arg("3000").arg("--").arg("python3").args(program_args)
            .stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::piped());
        let output = run_bounded_process(command, Duration::from_secs(7), 262144).unwrap();
        fs::write(self.root.join("cli.stdout"), &output.stdout).unwrap();
        fs::write(self.root.join("cli.stderr"), &output.stderr).unwrap();
        fs::write(self.root.join("cli-capture.json"), serde_json::to_vec(&json!({
            "status":output.status.code(),"timed_out":output.timed_out,
            "output_complete":output.output_complete,"output_truncated":output.output_truncated
        })).unwrap()).unwrap();
        assert!(!output.timed_out && output.output_complete && !output.output_truncated,
            "outer native CLI capture uncertain; retained raw evidence");
        assert_eq!(output.status.code(), Some(1), "actual incomplete or truncated command cannot return CLI success");
        let result: Value = serde_json::from_slice(&output.stdout).unwrap();
        assert_eq!(result["schema"], "workcell.write-boundary-result/v1");
        assert_eq!(result["executed"], true, "native boundary must actually execute; unavailable is not a positive gate");
        assert_eq!(result["exit_code"], 0);
        assert_eq!(result["timed_out"], false);
        assert_eq!(result["ok"], false);
        assert_eq!(result["automatic_retry"], false);
        result
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        if let Err(error) = fs::write(self.root.join("release"), b"release owned writer") {
            eprintln!("actual writer cleanup release failed: {error}; fixture {}", self.root.display());
            if !thread::panicking() {
                panic!("actual writer cleanup uncertain; retained evidence");
            }
        }
        if self.root.join("writer.json").exists() && !self.writer_verified.get() {
            let uncertainty = json!({"release_attempted":true,"physical_retirement_verified":false,
                "original_panic":thread::panicking(),"standing":"native writer cleanup unverified; evidence retained"});
            if let Err(error) = fs::write(self.root.join("cleanup-uncertainty.json"), uncertainty.to_string()) {
                eprintln!("cleanup uncertainty persistence failed: {error}");
                if !thread::panicking() { panic!("actual cleanup evidence unavailable"); }
            }
        }
    }
}

fn isolate(test: &str) -> bool {
    if std::env::var("WORKCELL_CLI_CAPTURE_ISOLATED").as_deref() == Ok(test) {
        #[cfg(target_os = "linux")]
        {
            // Test-only process ancestry: adopt and reap only this fixture's
            // actual escaped writer, without changing the shared test harness.
            extern "C" { fn prctl(option: i32, ...) -> i32; }
            assert_eq!(unsafe { prctl(36, 1i32, 0i32, 0i32, 0i32) }, 0);
        }
        return true;
    }
    let mut command = Command::new(std::env::current_exe().unwrap());
    command.args(["--exact", test, "--nocapture", "--test-threads=1"])
        .env("WORKCELL_CLI_CAPTURE_ISOLATED", test)
        .stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::piped());
    let result = run_bounded_process(command, Duration::from_secs(14), 65536).unwrap();
    assert!(result.status.success() && !result.timed_out
        && result.output_complete && !result.output_truncated
        && String::from_utf8_lossy(&result.stdout).contains("1 passed; 0 failed; 0 ignored;"),
        "real isolated CLI gate failed: stdout={} stderr={}",
        String::from_utf8_lossy(&result.stdout), String::from_utf8_lossy(&result.stderr));
    false
}

#[test]
fn actual_zero_exit_truncated_capture_is_not_cli_success() {
    let fixture = Fixture::new("actual-cli-truncation");
    let script = fixture.root.join("write.py");
    fs::write(&script, "import os,pathlib,sys\npathlib.Path(sys.argv[1]).write_bytes(b'actual-effect')\nos.write(1,b'x'*70000)\n").unwrap();
    let result = fixture.call(&[
        "-S".into(), script.display().to_string(),
        fixture.root.join("effect").display().to_string()
    ]);
    assert_eq!(result["output_complete"], true);
    assert_eq!(result["output_truncated"], true);
    assert_eq!(result["stdout"].as_str().unwrap().len(), 65536);
    assert_eq!(fs::read(fixture.root.join("effect")).unwrap(), b"actual-effect");
}

#[test]
fn actual_zero_exit_escaped_pipe_is_unverified_and_read_ends_close() {
    if !isolate("actual_zero_exit_escaped_pipe_is_unverified_and_read_ends_close") {
        return;
    }
    let fixture = Fixture::new("actual-cli-held-pipe");
    let script = fixture.root.join("writer.py");
    fs::write(&script, include_str!("../../workcell-runtime/tests/fixtures/bounded_pipe_writer.py")).unwrap();
    let result = fixture.call(&[
        "-S".into(), script.display().to_string(), fixture.root.display().to_string(), "1".into()
    ]);
    assert_eq!(result["output_complete"], false);
    assert_eq!(result["output_truncated"], false);
    assert_eq!(result["stdout"], "out-prefix\n");
    assert_eq!(result["stderr"], "err-prefix\n");
    fs::write(fixture.root.join("release"), b"actual caller releases its writer").unwrap();
    let deadline = Instant::now() + Duration::from_secs(3);
    let report_path = fixture.root.join("writer-result.json");
    while !report_path.exists() && Instant::now() < deadline {
        thread::sleep(Duration::from_millis(5));
    }
    let report: Value = serde_json::from_slice(&fs::read(report_path).unwrap()).unwrap();
    assert_eq!(report["released"], true);
    assert_eq!(report["writes"]["1"]["epipe"], true);
    assert_eq!(report["writes"]["2"]["epipe"], true);
    let pid = report["pid"].as_i64().unwrap() as i32;
    loop {
        #[cfg(target_os = "linux")]
        {
            extern "C" { fn waitpid(pid: i32, status: *mut i32, options: i32) -> i32; }
            let mut status = 0;
            let waited = unsafe { waitpid(pid, &mut status, 1) };
            if waited == pid {
                assert_eq!(status, 0, "exact adopted native writer must exit successfully");
            } else if waited < 0 {
                assert_eq!(std::io::Error::last_os_error().raw_os_error(), Some(10));
            }
        }
        let mut query = Command::new("/bin/ps");
        query.args(["-p", &pid.to_string(), "-o", "pid=,uid=,lstart=,stat="])
            .env("LC_ALL", "C")
            .stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::piped());
        let observed = run_bounded_process(query, Duration::from_secs(1), 4096).unwrap();
        fs::write(fixture.root.join("last-ps.stdout"), &observed.stdout).unwrap();
        fs::write(fixture.root.join("last-ps.stderr"), &observed.stderr).unwrap();
        assert!(!observed.timed_out && observed.output_complete && !observed.output_truncated);
        if observed.status.code() == Some(1) && observed.stdout.is_empty() && observed.stderr.is_empty() {
            break;
        }
        assert!(Instant::now() < deadline, "actual writer still present including zombie; no retirement claim");
        thread::sleep(Duration::from_millis(5));
    }
    fixture.writer_verified.set(true);
}

#[test]
fn actual_deadline_retains_native_partial_bytes_and_reap_facts() {
    let fixture=Fixture::new("actual-cli-deadline-prefix");
    let script=fixture.root.join("deadline.py");
    fs::write(&script,"import os,time\nos.write(1,b'actual-out\\x00prefix')\nos.write(2,b'actual-err\\xffprefix')\ntime.sleep(20)\n").unwrap();
    let mut command=Command::new(env!("CARGO_BIN_EXE_workcell-write-boundary"));
    command.arg("run").arg(&fixture.requirements).arg("actual-capture-1")
        .arg("1500").arg("--").arg("python3").arg("-S").arg(&script)
        .stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::piped());
    let outer=run_bounded_process(command,Duration::from_secs(5),262144).unwrap();
    fs::write(fixture.root.join("cli.stdout"),&outer.stdout).unwrap();
    fs::write(fixture.root.join("cli.stderr"),&outer.stderr).unwrap();
    assert!(outer.output_complete&&!outer.output_truncated&&!outer.timed_out);
    assert_eq!(outer.status.code(),Some(1));
    let result:Value=serde_json::from_slice(&outer.stdout).unwrap();
    assert_eq!(result["ok"],false);assert_eq!(result["executed"],true);
    assert_eq!(result["timed_out"],true);
    assert_eq!(result["capture_failure"]["kind"],"DeadlineElapsed");
    assert_eq!(result["capture_failure"]["status_observed"],true);
    assert_eq!(result["capture_failure"]["reaped_by_owner"],true);
    assert_eq!(result["capture_failure"]["termination_request_accepted"],true);
    assert_eq!(result["stdout_bytes"],json!(b"actual-out\0prefix".as_slice()));
    assert_eq!(result["stderr_bytes"],json!(b"actual-err\xffprefix".as_slice()));
    assert_eq!(result["automatic_retry"],false);
    assert!(!result["error"].as_str().unwrap().contains("actual-out"));
}
