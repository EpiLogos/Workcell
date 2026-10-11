#![cfg(any(target_os = "linux", target_os = "macos"))]

//! Separate real CLI invocations exercise the existing material owner, native
//! operator grant, journal, process lifetime and Run ledger. Artifact placement
//! is supplied by the same product/hosted qualification as the native tests.
use epilogos_workcell_core::{
    ProviderOperation, ProviderPortKind, WorkcellControlPlane, WorkcellRef,
};
use epilogos_workcell_runtime::{
    capture_bounded_process, CollapsedLocalConfig, MaterialProcessIdentity,
    MaterialProcessIdentityState,
};
use epilogos_workcell_wire::{
    decode_world, material_operation_grant_value, material_operation_request_value,
    material_world_revision,
};
use serde_json::{json, Value};
use std::{
    collections::BTreeMap,
    fs,
    path::PathBuf,
    process::{Command, Stdio},
    time::{Duration, SystemTime, UNIX_EPOCH},
};

struct Fixture {
    root: PathBuf,
    state: PathBuf,
}
impl Fixture {
    fn new(label: &str) -> Self {
        let base = PathBuf::from(
            std::env::var_os("WORKCELL_TEST_ARTIFACT_ROOT")
                .expect("real CLI qualification requires an admitted artifact root"),
        )
        .canonicalize()
        .unwrap();
        let source = PathBuf::from(
            std::env::var_os("WORKCELL_TEST_ARTIFACT_OWNER_SOURCE")
                .expect("real CLI qualification requires its actual owner source"),
        )
        .canonicalize()
        .unwrap();
        let owner = std::env::var("WORKCELL_TEST_ARTIFACT_OWNER_REF").unwrap();
        match std::env::var("WORKCELL_TEST_ARTIFACT_ADMISSION")
            .unwrap()
            .as_str()
        {
            "product-scratch" => {
                let product = source.parent().unwrap().parent().unwrap();
                let reading: Value = serde_json::from_slice(&fs::read(&source).unwrap()).unwrap();
                assert_eq!(reading["schema"], "central.project/v1");
                assert_eq!(reading["human_source"], "ProjectCentral/user");
                assert_eq!(
                    owner,
                    format!("project:{}", reading["project_id"].as_str().unwrap())
                );
                assert_eq!(source, product.join("ProjectCentral/project.json"));
                assert!(base.starts_with(
                    product
                        .join("ProjectCentral/now/tmp")
                        .canonicalize()
                        .unwrap()
                ));
            }
            "hosted-runner" => {
                let workspace = PathBuf::from(std::env::var_os("GITHUB_WORKSPACE").unwrap())
                    .canonicalize()
                    .unwrap();
                assert_eq!(std::env::var("GITHUB_ACTIONS").unwrap(), "true");
                assert_eq!(std::env::var("CI").unwrap(), "true");
                assert!(!std::env::var("GITHUB_RUN_ID").unwrap().is_empty());
                assert_eq!(
                    base,
                    workspace
                        .join("evidence/native-processes")
                        .canonicalize()
                        .unwrap()
                );
                assert_eq!(source, workspace.join("evidence/source-commit.txt"));
                assert_eq!(fs::read_to_string(source).unwrap().trim(), owner);
                assert_eq!(owner.len(), 40);
                assert!(owner.bytes().all(|b| b.is_ascii_hexdigit()));
            }
            other => panic!("unsupported actual CLI artifact admission: {other}"),
        }
        let root = base.join(format!(
            "local-material-{label}-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir(&root).unwrap();
        Self {
            state: root.join("state"),
            root,
        }
    }
    fn cli(
        &self,
        label: &str,
        args: &[&str],
        credential: Option<&str>,
        json_mode: bool,
    ) -> (bool, Value) {
        let mut command = Command::new(env!("CARGO_BIN_EXE_workcell"));
        command
            .args([
                "--state-root",
                self.state.to_str().unwrap(),
                "--workcell-ref",
                "workcell:cli-test",
            ])
            .args(if json_mode {
                vec!["--json"]
            } else {
                Vec::new()
            })
            .args(args)
            .env_remove("WORKCELL_CONTROL_TOKEN")
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        if let Some(credential) = credential {
            command.env("WORKCELL_CONTROL_TOKEN", credential);
        }
        let output = capture_bounded_process(command, Duration::from_secs(20), 65_536)
            .unwrap_or_else(|error| panic!("bounded actual CLI capture failed: {error}"));
        assert!(!output.timed_out && output.output_complete && !output.output_truncated);
        let mut value = if json_mode {
            serde_json::from_slice(if output.status.success() {
                &output.stdout
            } else {
                &output.stderr
            })
            .unwrap_or_else(|_| panic!("{label} did not return complete structured CLI output"))
        } else {
            json!({"text":String::from_utf8(output.stdout.clone()).unwrap()})
        };
        // Native authorise emits its credential once. Keep it in memory for
        // this private test connection; persist no credential or command env.
        let credential_value = value.get("credential").cloned();
        if credential_value.is_some() {
            value["credential"] = json!("[private native credential omitted]");
        }
        fs::write(
            self.root.join(format!("{label}.json")),
            json!({
            "exit_code":output.status.code(),"stdout_bytes":output.stdout.len(),
            "stderr_bytes":output.stderr.len(),"reading":value})
            .to_string(),
        )
        .unwrap();
        if let Some(credential) = credential_value {
            value["credential"] = credential;
        }
        (output.status.success(), value)
    }
    fn start(&self, slug: &str) -> Value {
        let (ok, value) = self.cli(
            "start",
            &["run", "start", "--run", slug, "--require", "shell"],
            None,
            true,
        );
        assert!(ok, "actual local preparation refused: {value}");
        assert_eq!(
            value["run"]["material_owner"],
            "workcell.durable-local-run/v1"
        );
        value["run"].clone()
    }
}

#[test]
fn local_material_cli_uses_native_grant_restarts_and_replays_without_second_effect() {
    let fixture = Fixture::new("operation");
    let slug = "local-operation";
    let run = fixture.start(slug);
    let world =
        decode_world(&fs::read_to_string(run["world_receipt"].as_str().unwrap()).unwrap()).unwrap();
    let binding = world
        .binding_graph
        .bindings
        .iter()
        .find(|b| b.port == ProviderPortKind::Execution)
        .unwrap();
    let operation = ProviderOperation {
        key: "process".into(),
        parameters: BTreeMap::from([
            ("program".into(), "/bin/sh".into()),
            ("cwd".into(), fixture.root.display().to_string()),
            ("arg.0".into(), "-c".into()),
            (
                "arg.1".into(),
                "printf 'one\n' >> effect-count; printf 'actual-material-return'".into(),
            ),
        ]),
    };
    let request = epilogos_workcell_core::MaterialOperationRequest {
        world_ref: world.world_ref.clone(),
        binding_ref: binding.binding_ref.clone(),
        expected_world_revision: material_world_revision(&world).unwrap(),
        operation_id: "operation:local-cli-exact".into(),
        grant_ref: "grant:local-cli-exact".into(),
        operation: operation.clone(),
        timeout_ms: 5_000,
        output_limit_bytes: 4_096,
    };
    let grant = epilogos_workcell_core::MaterialOperationGrant {
        grant_ref: request.grant_ref.clone(),
        authority_ref: "authority:owned-cli-test-directory".into(),
        world_ref: world.world_ref.clone(),
        binding_ref: binding.binding_ref.clone(),
        expected_world_revision: request.expected_world_revision.clone(),
        provider_ref: binding.provider_ref.clone(),
        material_ref: binding.material_ref.clone(),
        operation_id: request.operation_id.clone(),
        operation,
        timeout_ms: request.timeout_ms,
        output_limit_bytes: request.output_limit_bytes,
        expires_at_unix_ms: SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_millis() as u64
            + 600_000,
    };
    let request_path = fixture.root.join("request.json");
    fs::write(
        &request_path,
        material_operation_request_value(&request).to_string(),
    )
    .unwrap();
    let grant_path = fixture.root.join("grant.json");
    fs::write(
        &grant_path,
        material_operation_grant_value(&grant).to_string(),
    )
    .unwrap();
    let execute = [
        "run",
        "execute",
        "--run",
        slug,
        "--request",
        request_path.to_str().unwrap(),
    ];
    let (ok, _) = fixture.cli("unadmitted", &execute, None, true);
    assert!(
        !ok && !fixture.root.join("effect-count").exists(),
        "a payload grant cannot grant execution"
    );
    let (ok, admitted) = fixture.cli(
        "authorise",
        &[
            "authorise",
            "--client",
            "local-material-test",
            "--allow",
            "execute",
            "--allow",
            "operation-read",
            "--execution-grant",
            grant_path.to_str().unwrap(),
            "--expires-in",
            "15m",
        ],
        None,
        true,
    );
    assert!(ok);
    let credential = admitted["credential"].as_str().unwrap();
    let (ok, reading) = fixture.cli("executed", &execute, Some(credential), true);
    assert!(ok, "{reading}");
    assert_eq!(reading["operation"]["state"], "completed");
    assert_eq!(
        reading["operation"]["result"]["output"]["stdout"],
        "actual-material-return"
    );
    assert_eq!(
        fs::read_to_string(fixture.root.join("effect-count")).unwrap(),
        "one\n"
    );
    let retained = reading["operation"].clone();
    for role in ["guardian_identity", "command_identity"] {
        let identity = MaterialProcessIdentity::from_value(
            &serde_json::from_str(retained["failure_observation"][role].as_str().unwrap()).unwrap(),
        )
        .unwrap();
        let observed =
            epilogos_workcell_runtime::material_process_identity_state(&identity).unwrap();
        assert_eq!(
            observed.identity_state,
            MaterialProcessIdentityState::Absent
        );
        assert!(observed.original_group_absent);
    }
    let read = [
        "run",
        "operation-read",
        "--run",
        slug,
        "--operation-id",
        request.operation_id.as_str(),
    ];
    let (ok, after_restart) = fixture.cli("read-after-restart", &read, Some(credential), true);
    assert!(ok);
    assert_eq!(after_restart["operation"], retained);
    let (ok, replay) = fixture.cli("exact-replay", &execute, Some(credential), true);
    assert!(ok);
    assert_eq!(replay["operation"], retained);
    assert_eq!(
        fs::read_to_string(fixture.root.join("effect-count")).unwrap(),
        "one\n"
    );
    let mut changed = request.clone();
    changed.operation.parameters.insert(
        "arg.1".into(),
        "printf 'unadmitted\n' >> effect-count".into(),
    );
    fs::write(
        &request_path,
        material_operation_request_value(&changed).to_string(),
    )
    .unwrap();
    let (ok, _) = fixture.cli("changed-request", &execute, Some(credential), true);
    assert!(!ok);
    assert_eq!(
        fs::read_to_string(fixture.root.join("effect-count")).unwrap(),
        "one\n"
    );
    let (ok, human) = fixture.cli("human-read", &read, Some(credential), false);
    assert!(ok);
    assert!(human["text"]
        .as_str()
        .unwrap()
        .contains("operation:local-cli-exact: completed"));
    let (ok, _) = fixture.cli("collect", &["run", "collect", "--run", slug], None, true);
    assert!(ok);
    // A changed immutable material receipt never becomes native authority.
    let receipt_path = PathBuf::from(run["world_receipt"].as_str().unwrap());
    let compatibility_before = fs::read(&receipt_path).unwrap();
    let mut foreign_material = world.clone();
    foreign_material.binding_graph.bindings[0]
        .material_ref
        .push_str("-changed");
    fs::write(
        &receipt_path,
        epilogos_workcell_wire::encode_world(&foreign_material).unwrap(),
    )
    .unwrap();
    let (ok, _) = fixture.cli("changed-immutable-receipt", &read, Some(credential), true);
    assert!(!ok);
    fs::write(&receipt_path, &compatibility_before).unwrap();
    // Actual native release completes, then stop at the precise publication
    // checkpoint before the CLI's compatibility receipt and Run CAS. No
    // fabricated tombstone or command replay stands in for the owner effect.
    {
        let mut owner = epilogos_workcell_cli::DurableCollapsedLocalWorkcell::new(
            CollapsedLocalConfig::new(
                WorkcellRef::new("workcell:cli-test").unwrap(),
                &fixture.state,
            )
            .with_persistent_host_lifetime()
            .with_native_material_executable(PathBuf::from(env!("CARGO_BIN_EXE_workcell"))),
        )
        .unwrap();
        assert!(owner.release(&request.world_ref).unwrap().changed);
        assert!(owner
            .inspect(&request.world_ref)
            .unwrap()
            .binding_graph
            .bindings
            .iter()
            .all(|b| b.presence == epilogos_workcell_core::BindingPresence::Released));
    }
    assert_eq!(fs::read(&receipt_path).unwrap(), compatibility_before);
    let (ok, checkpoint_read) = fixture.cli(
        "release-checkpoint-restart-read",
        &read,
        Some(credential),
        true,
    );
    assert!(ok);
    assert_eq!(checkpoint_read["operation"], retained);
    let (ok, released) = fixture.cli("release", &["run", "release", "--run", slug], None, true);
    assert!(ok, "{released}");
    assert_eq!(released["run"]["execution_status"], "success");
    let tombstone =
        decode_world(&fs::read_to_string(run["world_receipt"].as_str().unwrap()).unwrap()).unwrap();
    assert!(tombstone
        .binding_graph
        .bindings
        .iter()
        .all(|b| b.presence == epilogos_workcell_core::BindingPresence::Released));
    let (ok, final_read) = fixture.cli("read-after-release", &read, Some(credential), true);
    assert!(ok);
    assert_eq!(final_read["operation"], retained);
}

#[test]
fn local_material_cli_preserves_exclusive_host_lease_before_preparation() {
    let fixture = Fixture::new("lease");
    let _owner = epilogos_workcell_cli::DurableCollapsedLocalWorkcell::new(
        CollapsedLocalConfig::new(
            WorkcellRef::new("workcell:cli-test").unwrap(),
            &fixture.state,
        )
        .with_persistent_host_lifetime(),
    )
    .unwrap();
    let (ok, refused) = fixture.cli(
        "lease-refusal",
        &["run", "start", "--run", "held-lease", "--require", "shell"],
        None,
        true,
    );
    assert!(!ok);
    assert_eq!(refused["error"]["kind"], "unavailable");
    assert!(refused["error"]["message"]
        .as_str()
        .unwrap()
        .contains("another material host owns this state root"));
    let worlds = fs::read_dir(fixture.state.join("control-worlds"))
        .unwrap()
        .filter_map(Result::ok)
        .filter(|p| p.path().extension().is_some_and(|e| e == "json"))
        .count();
    assert_eq!(
        worlds, 0,
        "a competing CLI may retain a failed Run, but cannot allocate material"
    );
}

#[test]
fn local_material_cli_keeps_unmarked_legacy_run_lifecycle() {
    let fixture = Fixture::new("legacy");
    let slug = "legacy-lifecycle";
    // Exercise a genuine legacy, non-durable material receipt produced by
    // the existing prepare owner, then retain its unmarked historical Run.
    let receipt = fixture.root.join("legacy-world.json");
    let (ok, prepared) = fixture.cli(
        "legacy-prepare",
        &[
            "--receipt",
            receipt.to_str().unwrap(),
            "prepare",
            "--require",
            "shell",
        ],
        None,
        true,
    );
    assert!(ok);
    let ledger = epilogos_workcell_runtime::RunLedger::new(&fixture.state);
    let run = ledger.create(json!({"run_slug":slug,"demand_ref":prepared["world"]["demand_ref"],
        "execution_status":"running","rung":"local","provider_ref":"provider:collapsed-local-host-process",
        "world_ref":prepared["world"]["world_ref"],"world_receipt":receipt.display().to_string()})).unwrap();
    assert!(run.get("material_owner").is_none());
    assert!(!fixture.state.join("control-worlds").exists());
    let (ok, _) = fixture.cli(
        "legacy-observe",
        &["run", "observe", "--run", slug],
        None,
        true,
    );
    assert!(ok);
    let (ok, _) = fixture.cli(
        "legacy-collect",
        &["run", "collect", "--run", slug],
        None,
        true,
    );
    assert!(ok);
    let (ok, value) = fixture.cli(
        "legacy-release",
        &["run", "release", "--run", slug],
        None,
        true,
    );
    assert!(ok);
    assert_eq!(value["run"]["execution_status"], "success");
}
