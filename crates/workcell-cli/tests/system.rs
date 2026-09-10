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
        "epilogos-workcell-system-{label}-{}-{nonce}",
        std::process::id()
    ))
}

fn run(args: &[&str]) -> Output {
    Command::new(binary()).args(args).output().unwrap()
}

fn path_arg(path: &Path) -> &str {
    path.to_str().unwrap()
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

fn section<'a>(reading: &'a Value, id: &str) -> &'a Value {
    reading["sections"]
        .as_array()
        .unwrap()
        .iter()
        .find(|section| section["id"] == id)
        .unwrap_or_else(|| panic!("missing section `{id}`"))
}

fn setting<'a>(section: &'a Value, key: &str) -> &'a Value {
    section["settings"]
        .as_array()
        .unwrap()
        .iter()
        .find(|setting| setting["key"] == key)
        .unwrap_or_else(|| panic!("missing setting `{key}`"))
}

#[test]
fn system_disclosure_is_a_valid_v2_descriptor() {
    let state = temp_path("v2");
    let output = run(&[
        "--state-root",
        path_arg(&state),
        "--workcell-ref",
        "workcell:owner-machine-test",
        "system",
        "--json",
    ]);
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let reading = json_stdout(&output);

    assert_eq!(reading["schema"], "oi.product-settings-disclosure/v2");
    assert_eq!(reading["product_id"], "workcell");
    assert_eq!(reading["contract_revision"], "wave-5/system.1");
    assert_eq!(reading["owner"]["owner_id"], "workcell");
    assert_eq!(reading["owner"]["owner_ref"], "workcell:owner-machine-test");
    assert_eq!(
        reading["owner"]["reading_command"],
        serde_json::json!(["workcell", "system", "--json"])
    );
    assert!(
        reading["owner"]["reading_digest"]
            .as_str()
            .is_some_and(|digest| digest.len() == 64),
        "reading_digest should be a 64-char sha256 hex"
    );

    // All nine sections named by the Wave 5 Workcell scope are present.
    let expected = [
        "workcells",
        "providers",
        "processes-services",
        "storage",
        "fabric",
        "local-remote",
        "model-serving",
        "hardware",
        "lifecycle",
    ];
    for id in expected {
        assert!(section(&reading, id).get("settings").is_some());
    }

    assert_eq!(reading["availability"]["state"], "available");
    assert!(!reading["obligations"].as_array().unwrap().is_empty());
    let _ = fs::remove_dir_all(state);
}

#[test]
fn unavailable_faculties_are_disclosed_as_unavailable_not_fabricated() {
    let state = temp_path("honest");
    let output = run(&["--state-root", path_arg(&state), "system", "--json"]);
    assert!(output.status.success());
    let reading = json_stdout(&output);

    let hardware = section(&reading, "hardware");
    let accelerator = setting(hardware, "hardware.accelerator");
    for axis in ["declared", "effective", "active"] {
        assert_eq!(
            accelerator["axes"][axis]["value"]["state"], "unavailable",
            "accelerator axis `{axis}` must not invent a GPU"
        );
    }

    let fabric = section(&reading, "fabric");
    let tailscale = setting(fabric, "fabric.tailscale");
    assert_eq!(tailscale["axes"]["active"]["value"]["state"], "unavailable");

    let degradations = reading["degradations"].as_array().unwrap();
    assert!(degradations
        .iter()
        .any(|d| d["subject_ref"] == "hardware.accelerator"));
    assert!(degradations
        .iter()
        .any(|d| d["subject_ref"] == "fabric.tailscale"));
    let _ = fs::remove_dir_all(state);
}

#[test]
fn logical_identity_stays_distinct_from_provider_process_and_material_binding() {
    let state = temp_path("identity");
    let output = run(&[
        "--state-root",
        path_arg(&state),
        "--workcell-ref",
        "workcell:owner-machine-test",
        "system",
        "--json",
    ]);
    assert!(output.status.success());
    let reading = json_stdout(&output);

    let workcells = section(&reading, "workcells");
    let identity = setting(workcells, "workcells.current");
    let declared = identity["axes"]["declared"]["value"].as_object().unwrap();
    let effective = identity["axes"]["effective"]["value"].as_object().unwrap();
    let active = identity["axes"]["active"]["value"].as_object().unwrap();

    // Identity carries only the WorkcellRef — never a provider, pid or material locator.
    assert_eq!(
        declared.get("workcell_ref").and_then(Value::as_str),
        Some("workcell:owner-machine-test")
    );
    assert_eq!(
        effective.get("workcell_ref").and_then(Value::as_str),
        Some("workcell:owner-machine-test")
    );
    assert_eq!(
        active.get("workcell_ref").and_then(Value::as_str),
        Some("workcell:owner-machine-test")
    );
    // Identity carries only the WorkcellRef — never a provider, pid, material
    // locator or network endpoint.
    for object in [declared, effective, active] {
        assert!(object.get("provider_ref").is_none());
        assert!(object.get("pid").is_none());
        assert!(object.get("material_ref").is_none());
        assert!(object.get("endpoint").is_none());
    }
    assert_eq!(identity["drift"]["state"], "none");

    // Providers are disclosed separately, keyed by provider_ref and port.
    let providers = section(&reading, "providers");
    let inventory = setting(providers, "providers.inventory");
    assert!(inventory["axes"]["declared"]["value"]["providers"]
        .as_array()
        .unwrap()
        .iter()
        .any(|p| p["provider_ref"] == "provider:collapsed-local-host-process"));
    let _ = fs::remove_dir_all(state);
}

#[test]
fn actions_are_disclosed_and_obligations_are_named() {
    let state = temp_path("actions");
    let output = run(&["--state-root", path_arg(&state), "system", "--json"]);
    assert!(output.status.success());
    let reading = json_stdout(&output);

    let actions = reading["actions"].as_array().unwrap();
    let prepare = actions
        .iter()
        .find(|action| action["action_ref"] == "workcell.prepare")
        .expect("workcell.prepare should be disclosed");
    assert_eq!(prepare["availability"], "disclosed");
    assert_eq!(prepare["exposure"]["headless"], true);
    assert_eq!(prepare["exposure"]["ui"], false);

    let intent = actions
        .iter()
        .find(|action| action["action_ref"] == "workcell.intent")
        .expect("workcell.intent should be a named obligation");
    assert_eq!(intent["availability"], "missing_native_obligation");
    let _ = fs::remove_dir_all(state);
}

#[test]
fn remote_system_reading_reports_unavailable_instead_of_fabricating() {
    let output = run(&["--endpoint", "127.0.0.1:1", "system", "--json"]);
    assert!(output.status.success());
    let reading = json_stdout(&output);
    assert_eq!(reading["schema"], "oi.product-settings-disclosure/v2");
    assert_eq!(reading["product_id"], "workcell");
    assert_eq!(reading["availability"]["state"], "unavailable");
    assert!(!reading["obligations"].as_array().unwrap().is_empty());
}
