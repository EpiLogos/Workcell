//! Configuration-plane contract tests for Workcell's owner contribution
//! (`oi.configuration-contribution/v1`) and its owner-native mutation
//! transport (`workcell config validate|plan|apply|reset`), frozen by
//! O-I `docs/cradle/09-CONFIGURATION-PLANE.md` (#299 Gate A / C0).
//!
//! The JSON Schemas under `fixtures/oi-configuration/` are byte-exact copies
//! of the frozen C0 contract schemas (`schemas/oi.*.schema.json` in the O-I
//! c1-kernel reference checkout, Gate A). Every document this owner emits is
//! validated against them here, so contract drift fails in this repository and
//! not at the O-I mount.

use std::{
    fs,
    io::Write,
    path::{Path, PathBuf},
    process::{Command, Output, Stdio},
    time::{SystemTime, UNIX_EPOCH},
};

use serde_json::{json, Value};

fn binary() -> &'static str {
    env!("CARGO_BIN_EXE_workcell")
}

fn fixture_schema(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../fixtures/oi-configuration")
        .join(name)
}

fn temp_path(label: &str) -> PathBuf {
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    std::env::temp_dir().join(format!(
        "epilogos-workcell-config-{label}-{}-{nonce}",
        std::process::id()
    ))
}

fn run(args: &[String]) -> Output {
    Command::new(binary()).args(args).output().unwrap()
}

fn run_with_stdin(args: &[String], input: &str) -> Output {
    let mut child = Command::new(binary())
        .args(args)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    child
        .stdin
        .take()
        .unwrap()
        .write_all(input.as_bytes())
        .unwrap();
    child.wait_with_output().unwrap()
}

/// `workcell --state-root <state> config <verb> <flags...>`
fn config_args(state: &Path, verb: &str, flags: &[&str]) -> Vec<String> {
    let mut args = vec![
        "--state-root".to_owned(),
        state.display().to_string(),
        "config".to_owned(),
        verb.to_owned(),
    ];
    args.extend(flags.iter().map(|flag| (*flag).to_owned()));
    args
}

/// `workcell --state-root <state> <command>` (contribution, disclosure, discovery).
fn plain_args(state: &Path, command: &str) -> Vec<String> {
    vec![
        "--state-root".to_owned(),
        state.display().to_string(),
        command.to_owned(),
        "--json".to_owned(),
    ]
}

fn json_stdout(output: &Output) -> Value {
    serde_json::from_slice(&output.stdout).unwrap_or_else(|error| {
        panic!(
            "invalid JSON stdout: {error}\nstdout={}\nstderr={}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        )
    })
}

/// The document must satisfy the frozen C0 JSON Schema — byte-exact copy,
/// no local re-decision.
fn assert_satisfies(document: &Value, schema_file: &str) {
    let raw = fs::read_to_string(fixture_schema(schema_file))
        .unwrap_or_else(|error| panic!("read frozen schema {schema_file}: {error}"));
    let schema: Value = serde_json::from_str(&raw).unwrap();
    let validator = jsonschema::validator_for(&schema).unwrap();
    let errors: Vec<String> = validator
        .iter_errors(document)
        .map(|error| format!("{error} (at {})", error.instance_path()))
        .collect();
    assert!(
        errors.is_empty(),
        "document does not satisfy the frozen schema {schema_file}:\n{}",
        errors.join("\n")
    );
}

const SETTING: &str = "workcell:processes-services:services.declared";

/// An executable that certainly exists on this machine: this test's own binary.
fn an_existing_program() -> String {
    std::env::current_exe().unwrap().display().to_string()
}

fn good_service_value() -> String {
    json!([{
        "logical_ref": "inference:local-chat",
        "lifetime": "provider-process-scoped",
        "endpoint": "http://127.0.0.1:21434",
        "program": an_existing_program(),
        "readiness": { "host": "127.0.0.1", "port": 21434, "timeout_ms": 5000 }
    }])
    .to_string()
}

fn plan_good_value(state: &Path) -> Value {
    json_stdout(&run(&config_args(
        state,
        "plan",
        &[
            "--json",
            "--setting",
            SETTING,
            "--value",
            &good_service_value(),
        ],
    )))
}

#[test]
fn contribution_satisfies_the_frozen_schema_and_maps_onto_the_disclosure() {
    let state = temp_path("contribution");
    let contribution = json_stdout(&run(&plain_args(&state, "config-contribution")));
    assert_satisfies(
        &contribution,
        "oi.configuration-contribution-v1.schema.json",
    );

    assert_eq!(contribution["schema"], "oi.configuration-contribution/v1");
    assert_eq!(
        contribution["contract_revision"],
        "configuration-plane/contribution.1"
    );
    // Owner identity is the product_id, not this machine's WorkcellRef.
    assert_eq!(contribution["owner"]["owner_ref"], "workcell");
    assert_eq!(contribution["owner"]["owner_kind"], "product");
    assert_eq!(
        contribution["owner"]["contribution_command"],
        json!(["workcell", "config-contribution", "--json"])
    );
    let digest = contribution["owner"]["reading_digest"].as_str().unwrap();
    assert_eq!(digest.len(), 64, "reading_digest is sha256 hex");
    // Two readings of an unchanged world share the canonical digest.
    let again = json_stdout(&run(&plain_args(&state, "config-contribution")));
    assert_eq!(digest, again["owner"]["reading_digest"]);

    // Structural identity mapping (09 §17): the contributed section id and
    // setting key must exist verbatim in the v2 disclosure plane.
    let disclosure = json_stdout(&run(&plain_args(&state, "system")));
    let section_id = contribution["sections"][0]["id"].as_str().unwrap();
    let setting_key = SETTING.rsplit(':').next().unwrap();
    let disclosure_section = disclosure["sections"]
        .as_array()
        .unwrap()
        .iter()
        .find(|section| section["id"] == section_id)
        .unwrap_or_else(|| panic!("disclosure has no section `{section_id}`"));
    assert!(
        disclosure_section["settings"]
            .as_array()
            .unwrap()
            .iter()
            .any(|setting| setting["key"] == setting_key),
        "disclosure section `{section_id}` has no key `{setting_key}`"
    );

    // Observed facts are not contributed as writable settings: the only
    // contributed setting is the operator's declared-services policy.
    let contributed: Vec<&str> = contribution["sections"]
        .as_array()
        .unwrap()
        .iter()
        .flat_map(|section| section["settings"].as_array().unwrap())
        .filter_map(|setting| setting["setting_ref"].as_str())
        .collect();
    assert_eq!(contributed, vec![SETTING]);
    assert!(!contribution["obligations"].as_array().unwrap().is_empty());
    let _ = fs::remove_dir_all(state);
}

#[test]
fn verbs_round_trip_in_a_sandbox_with_a_truthful_effect() {
    let state = temp_path("roundtrip");

    // validate answers: the value is honurable on this machine.
    let validation = json_stdout(&run(&config_args(
        state.as_path(),
        "validate",
        &[
            "--json",
            "--setting",
            SETTING,
            "--value",
            &good_service_value(),
        ],
    )));
    assert_satisfies(&validation, "oi.config-validation-v1.schema.json");
    assert_eq!(validation["valid"], true);
    assert_eq!(validation["violations"], json!([]));

    // plan: owner-minted plan_id and plan_digest.
    let plan = plan_good_value(state.as_path());
    assert_satisfies(&plan, "oi.config-plan-v1.schema.json");
    assert!(plan["plan_id"].as_str().unwrap().starts_with("wcplan-"));
    let digest = plan["plan_digest"].as_str().unwrap();
    assert_eq!(digest.len(), 64, "plan_digest is sha256 hex");

    // apply: the plan crosses as a file; the receipt is owner-minted and names
    // the owner's own history.
    fs::create_dir_all(&state).unwrap();
    let plan_file = state.join("plan.json");
    fs::write(&plan_file, plan.to_string()).unwrap();
    let apply = run(&config_args(
        state.as_path(),
        "apply",
        &[
            "--json",
            "--plan-file",
            path_arg(&plan_file),
            "--changeset",
            "cs-config-test-1",
        ],
    ));
    assert!(
        apply.status.success(),
        "{}",
        String::from_utf8_lossy(&apply.stderr)
    );
    let receipt = json_stdout(&apply);
    assert_satisfies(&receipt, "oi.config-receipt-v1.schema.json");
    assert_eq!(receipt["outcome"], "applied");
    assert_eq!(receipt["operation"], "apply");
    assert_eq!(receipt["owner_ref"], "workcell");
    assert_eq!(receipt["plan_digest"], digest);
    assert_eq!(receipt["changeset_id"], "cs-config-test-1");
    assert!(
        receipt["native_ref"]
            .as_str()
            .unwrap()
            .starts_with("workcell:config:history:"),
        "the receipt must point into Workcell's own history"
    );
    let original_receipt_id = receipt["receipt_id"].as_str().unwrap().to_owned();

    // The material effect is real: the state-root declaration now holds the
    // service and the next discovery offers it.
    let declaration: Value =
        serde_json::from_str(&fs::read_to_string(state.join("services.json")).unwrap()).unwrap();
    assert_eq!(declaration["schema"], "workcell.service-declaration/v1");
    assert_eq!(
        declaration["services"],
        serde_json::from_str::<Value>(&good_service_value()).unwrap()
    );
    let discovery = json_stdout(&run(&plain_args(&state, "discover")));
    assert!(
        discovery["offers"]
            .as_array()
            .unwrap()
            .iter()
            .any(|offer| offer["connections"]
                .as_array()
                .unwrap()
                .iter()
                .any(|connection| connection == "inference:local-chat")),
        "the declared service must be offered after apply"
    );

    // Replay under the frozen idempotency key: no re-execution, no_op naming
    // the original receipt.
    let replay = run(&config_args(
        state.as_path(),
        "apply",
        &[
            "--json",
            "--plan-file",
            path_arg(&plan_file),
            "--changeset",
            "cs-config-test-1",
        ],
    ));
    assert!(replay.status.success());
    let replay_receipt = json_stdout(&replay);
    assert_satisfies(&replay_receipt, "oi.config-receipt-v1.schema.json");
    assert_eq!(replay_receipt["outcome"], "no_op");
    assert_eq!(replay_receipt["original_receipt_id"], original_receipt_id);
    assert_eq!(replay_receipt["native_ref"], receipt["native_ref"]);

    let _ = fs::remove_dir_all(state);
}

#[test]
fn apply_accepts_the_plan_from_stdin_without_argv_limits() {
    let state = temp_path("stdin");
    let plan = json_stdout(&run_with_stdin(
        &config_args(
            state.as_path(),
            "plan",
            &["--json", "--setting", SETTING, "--value-file", "-"],
        ),
        &good_service_value(),
    ));
    assert!(
        plan.get("plan_digest").is_some(),
        "plan must mint from a stdin value"
    );

    let output = run_with_stdin(
        &config_args(
            state.as_path(),
            "apply",
            &[
                "--json",
                "--plan-file",
                "-",
                "--changeset",
                "cs-config-test-stdin",
            ],
        ),
        &plan.to_string(),
    );
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let receipt = json_stdout(&output);
    assert_satisfies(&receipt, "oi.config-receipt-v1.schema.json");
    assert_eq!(receipt["outcome"], "applied");
    let _ = fs::remove_dir_all(state);
}

#[test]
fn an_impossible_provider_value_fails_native_validation_and_never_enters_declared_state() {
    let state = temp_path("impossible");
    let impossible = json!([{
        "logical_ref": "inference:phantom",
        "lifetime": "provider-process-scoped",
        "endpoint": "http://127.0.0.1:9",
        "program": "/nonexistent/workcell-config-unavailable-program"
    }])
    .to_string();

    // validate answers with owner reasons, valid: false.
    let validation = json_stdout(&run(&config_args(
        state.as_path(),
        "validate",
        &["--json", "--setting", SETTING, "--value", &impossible],
    )));
    assert_satisfies(&validation, "oi.config-validation-v1.schema.json");
    assert_eq!(validation["valid"], false);
    let violations = validation["violations"].as_array().unwrap();
    assert!(!violations.is_empty());
    assert_eq!(violations[0]["code"], "invalid_value");
    let message = violations[0]["message"].as_str().unwrap();
    assert!(
        message.contains("cannot be honoured on this machine"),
        "the owner reason must name material possibility: {message}"
    );

    // plan refuses: non-zero exit, oi.config-error/v1 on stdout.
    let planned = run(&config_args(
        state.as_path(),
        "plan",
        &["--json", "--setting", SETTING, "--value", &impossible],
    ));
    assert!(!planned.status.success());
    let error = json_stdout(&planned);
    assert_satisfies(&error, "oi.config-error-v1.schema.json");
    assert_eq!(error["schema"], "oi.config-error/v1");
    assert_eq!(error["error_code"], "invalid_value");
    assert_eq!(error["setting_ref"], SETTING);

    // Nothing entered declared state.
    assert!(!state.join("services.json").exists());
    let _ = fs::remove_dir_all(state);
}

#[test]
fn tampered_plans_are_refused() {
    let state = temp_path("tamper");
    let mut plan = plan_good_value(state.as_path());
    plan["changes"][0]["summary"] = json!("tampered after minting");
    fs::create_dir_all(&state).unwrap();
    let plan_file = state.join("tampered-plan.json");
    fs::write(&plan_file, plan.to_string()).unwrap();

    let applied = run(&config_args(
        state.as_path(),
        "apply",
        &[
            "--json",
            "--plan-file",
            path_arg(&plan_file),
            "--changeset",
            "cs-config-test-tamper",
        ],
    ));
    assert!(!applied.status.success());
    let error = json_stdout(&applied);
    assert_satisfies(&error, "oi.config-error-v1.schema.json");
    assert_eq!(error["error_code"], "validation_failed");
    assert!(!state.join("services.json").exists());
    let _ = fs::remove_dir_all(state);
}

#[test]
fn unknown_setting_and_out_of_scope_addressing_are_explicit_errors() {
    let state = temp_path("addressing");

    // A setting outside the contribution is unsupported, never guessed.
    let refused = run(&config_args(
        state.as_path(),
        "validate",
        &[
            "--json",
            "--setting",
            "workcell:placement:placement.policy",
            "--value",
            "local-first",
        ],
    ));
    assert!(!refused.status.success());
    let error = json_stdout(&refused);
    assert_satisfies(&error, "oi.config-error-v1.schema.json");
    assert_eq!(error["error_code"], "unsupported_setting");

    // A scope kind outside the setting's allowed_scopes is unsupported_scope.
    let refused = run(&config_args(
        state.as_path(),
        "validate",
        &[
            "--json",
            "--setting",
            SETTING,
            "--scope",
            "project:epilogos/o-i",
            "--value",
            "[]",
        ],
    ));
    assert!(!refused.status.success());
    let error = json_stdout(&refused);
    assert_eq!(error["error_code"], "unsupported_scope");
    assert_eq!(error["scope_kind"], "project");

    // A kind outside the frozen registry is unknown_scope_kind.
    let refused = run(&config_args(
        state.as_path(),
        "validate",
        &[
            "--json",
            "--setting",
            SETTING,
            "--scope",
            "cluster:west",
            "--value",
            "[]",
        ],
    ));
    assert!(!refused.status.success());
    let error = json_stdout(&refused);
    assert_eq!(error["error_code"], "unknown_scope_kind");
    assert_eq!(error["scope_kind"], "cluster");
    let _ = fs::remove_dir_all(state);
}

#[test]
fn reset_restores_the_baseline_and_replays_no_op() {
    let state = temp_path("reset");
    let plan = plan_good_value(state.as_path());
    fs::create_dir_all(&state).unwrap();
    let plan_file = state.join("plan.json");
    fs::write(&plan_file, plan.to_string()).unwrap();
    let applied = run(&config_args(
        state.as_path(),
        "apply",
        &[
            "--json",
            "--plan-file",
            path_arg(&plan_file),
            "--changeset",
            "cs-config-test-reset",
        ],
    ));
    assert!(applied.status.success());

    let receipt = json_stdout(&run(&config_args(
        state.as_path(),
        "reset",
        &[
            "--json",
            "--setting",
            SETTING,
            "--changeset",
            "cs-config-test-reset-baseline",
        ],
    )));
    assert_satisfies(&receipt, "oi.config-receipt-v1.schema.json");
    assert_eq!(receipt["operation"], "reset");
    assert_eq!(receipt["outcome"], "applied");
    let declaration: Value =
        serde_json::from_str(&fs::read_to_string(state.join("services.json")).unwrap()).unwrap();
    assert_eq!(
        declaration["services"],
        json!([]),
        "reset restores the no-declared-services baseline"
    );

    let replay_receipt = json_stdout(&run(&config_args(
        state.as_path(),
        "reset",
        &[
            "--json",
            "--setting",
            SETTING,
            "--changeset",
            "cs-config-test-reset-baseline",
        ],
    )));
    assert_eq!(replay_receipt["outcome"], "no_op");
    assert_eq!(replay_receipt["original_receipt_id"], receipt["receipt_id"]);
    let _ = fs::remove_dir_all(state);
}

fn path_arg(path: &Path) -> &str {
    path.to_str().unwrap()
}
