use std::{
    collections::BTreeSet,
    env,
    error::Error,
    fmt, fs,
    path::{Path, PathBuf},
    process::ExitCode,
};

use epilogos_workcell_control::{
    ControlClient, ControlClientError, TcpControlTransport,
};
use epilogos_workcell_core::{
    DesiredMaterialState, ExecutionDemand, MaterialisedExecutionWorld, WorkcellError, WorldRef,
};
use epilogos_workcell_wire::decode_world;
use serde_json::{json, Value};

mod local_cli {
    include!("../main.rs");

    pub(super) fn invoke() -> std::process::ExitCode {
        main()
    }

    pub(super) fn print_help_for_selector() {
        print_help();
    }

    pub(super) fn parse_remote_global(
        args: Vec<String>,
    ) -> Result<super::RemoteGlobal, epilogos_workcell_core::WorkcellError> {
        let global = parse_global(args)?;
        Ok(super::RemoteGlobal {
            json: global.json,
            state_root: global.state_root,
            receipt: global.receipt,
            workspace_source: global.workspace_source,
            remaining: global.remaining,
        })
    }

    pub(super) fn parse_remote_demand(
        args: &[String],
    ) -> Result<epilogos_workcell_core::ExecutionDemand, epilogos_workcell_core::WorkcellError> {
        parse_demand(args)
    }

    pub(super) fn parse_remote_desired(
        args: &[String],
    ) -> Result<Vec<epilogos_workcell_core::DesiredMaterialState>, epilogos_workcell_core::WorkcellError>
    {
        parse_desired(args)
    }

    pub(super) fn receipt_path(state_root: &std::path::Path, world_ref: &str) -> std::path::PathBuf {
        default_receipt_path(state_root, world_ref)
    }

    pub(super) fn persist_receipt(
        path: &std::path::Path,
        world: &epilogos_workcell_core::MaterialisedExecutionWorld,
    ) -> Result<(), epilogos_workcell_core::WorkcellError> {
        write_receipt(path, world)
    }
}

struct RemoteGlobal {
    json: bool,
    state_root: PathBuf,
    receipt: Option<PathBuf>,
    workspace_source: Option<PathBuf>,
    remaining: Vec<String>,
}

struct RemoteSelection {
    endpoint: Option<String>,
    authorization: Option<String>,
    cleaned_args: Vec<String>,
}

#[derive(Debug)]
enum CliError {
    Workcell(WorkcellError),
    Control(ControlClientError),
}

impl fmt::Display for CliError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Workcell(error) => error.fmt(formatter),
            Self::Control(error) => error.fmt(formatter),
        }
    }
}

impl Error for CliError {}

impl From<WorkcellError> for CliError {
    fn from(error: WorkcellError) -> Self {
        Self::Workcell(error)
    }
}

impl From<ControlClientError> for CliError {
    fn from(error: ControlClientError) -> Self {
        Self::Control(error)
    }
}

type RemoteClient = ControlClient<TcpControlTransport>;

fn main() -> ExitCode {
    let original_args = env::args().skip(1).collect::<Vec<_>>();
    if original_args.is_empty()
        || original_args
            .iter()
            .any(|arg| matches!(arg.as_str(), "help" | "-h" | "--help"))
    {
        print_combined_help();
        return ExitCode::SUCCESS;
    }

    let json_requested = original_args.iter().any(|arg| arg == "--json");
    let selection = match extract_remote_selection(&original_args) {
        Ok(selection) => selection,
        Err(error) => return report_error(error, json_requested),
    };

    let Some(endpoint) = selection.endpoint else {
        if selection.authorization.is_some() {
            return report_error(
                WorkcellError::InvalidDemand(
                    "--authorization/WORKCELL_CONTROL_TOKEN requires a remote --endpoint or WORKCELL_CONTROL_ENDPOINT"
                        .into(),
                )
                .into(),
                json_requested,
            );
        }
        return local_cli::invoke();
    };

    match run_remote(
        selection.cleaned_args,
        endpoint,
        selection.authorization,
    ) {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => report_error(error, json_requested),
    }
}

fn extract_remote_selection(args: &[String]) -> Result<RemoteSelection, CliError> {
    let mut endpoint = env::var("WORKCELL_CONTROL_ENDPOINT").ok();
    let mut authorization = env::var("WORKCELL_CONTROL_TOKEN").ok();
    let mut cleaned_args = Vec::with_capacity(args.len());
    let mut index = 0;

    while index < args.len() {
        match args[index].as_str() {
            "--endpoint" => {
                endpoint = Some(required_selector_value(args, index, "--endpoint")?.to_owned());
                index += 2;
            }
            "--authorization" => {
                authorization = Some(
                    required_selector_value(args, index, "--authorization")?.to_owned(),
                );
                index += 2;
            }
            _ => {
                cleaned_args.push(args[index].clone());
                index += 1;
            }
        }
    }

    Ok(RemoteSelection {
        endpoint,
        authorization,
        cleaned_args,
    })
}

fn required_selector_value<'a>(
    args: &'a [String],
    index: usize,
    flag: &str,
) -> Result<&'a str, CliError> {
    args.get(index + 1)
        .map(String::as_str)
        .ok_or_else(|| WorkcellError::InvalidDemand(format!("`{flag}` requires a value")).into())
}

fn run_remote(
    args: Vec<String>,
    endpoint: String,
    authorization: Option<String>,
) -> Result<(), CliError> {
    let global = local_cli::parse_remote_global(args)?;
    let Some(command) = global.remaining.first().map(String::as_str) else {
        print_combined_help();
        return Ok(());
    };
    let command_args = &global.remaining[1..];

    if global.workspace_source.is_some() {
        return Err(WorkcellError::InvalidDemand(
            "--workspace-source is a local material binding and cannot be projected onto a remote Workcell; bind the source on the remote Workcell instead"
                .into(),
        )
        .into());
    }

    let mut client = ControlClient::new(TcpControlTransport::new(endpoint.clone()));
    if let Some(authorization) = authorization {
        client = client.with_authorization(authorization);
    }

    match command {
        "status" => remote_status(&global, &endpoint, &mut client),
        "discover" => remote_discover(&global, &mut client),
        "providers" => remote_providers(&global, &mut client),
        "doctor" => remote_doctor(&global, &endpoint, &mut client),
        "plan" => remote_plan(&global, command_args, &mut client),
        "prepare" => remote_prepare(&global, command_args, &mut client),
        "observe" => remote_observe(&global, &mut client),
        "expose" => remote_expose(&global, &mut client),
        "collect" => remote_collect(&global, &mut client),
        "release" => remote_release(&global, &mut client),
        "reconcile" => remote_reconcile(&global, command_args, &mut client),
        other => Err(WorkcellError::InvalidDemand(format!(
            "unknown command `{other}`; run `workcell help`"
        ))
        .into()),
    }
}

fn remote_status(
    global: &RemoteGlobal,
    endpoint: &str,
    client: &mut RemoteClient,
) -> Result<(), CliError> {
    let value = client.status()?;
    if global.json {
        emit_json(with_ok(value));
    } else {
        println!(
            "Workcell {}",
            value["workcell_ref"].as_str().unwrap_or("unknown")
        );
        println!("health: {}", value["health"].as_str().unwrap_or("unknown"));
        println!("providers: {}", value["providers"].as_u64().unwrap_or(0));
        println!("offers: {}", value["offers"].as_u64().unwrap_or(0));
        println!("backend: service");
        println!("control endpoint: {endpoint}");
    }
    Ok(())
}

fn remote_discover(global: &RemoteGlobal, client: &mut RemoteClient) -> Result<(), CliError> {
    let value = client.discover()?;
    if global.json {
        emit_json(with_ok(value));
    } else {
        println!(
            "{} — {}",
            value["workcell_ref"].as_str().unwrap_or("unknown"),
            value["health"].as_str().unwrap_or("unknown")
        );
        if let Some(offers) = value["offers"].as_array() {
            for offer in offers {
                println!(
                    "{} [{}] {} / {}",
                    offer["provider_ref"].as_str().unwrap_or("unknown"),
                    offer["port"].as_str().unwrap_or("unknown"),
                    offer["availability"].as_str().unwrap_or("unknown"),
                    offer["health"].as_str().unwrap_or("unknown")
                );
            }
        }
    }
    Ok(())
}

fn remote_providers(global: &RemoteGlobal, client: &mut RemoteClient) -> Result<(), CliError> {
    let discovery = client.discover()?;
    let mut providers = BTreeSet::new();
    if let Some(offers) = discovery["offers"].as_array() {
        for offer in offers {
            if let Some(provider_ref) = offer["provider_ref"].as_str() {
                providers.insert(provider_ref.to_owned());
            }
        }
    }

    if global.json {
        emit_json(json!({
            "ok": true,
            "workcell_ref": discovery["workcell_ref"],
            "providers": providers,
        }));
    } else {
        for provider in providers {
            println!("{provider}");
        }
    }
    Ok(())
}

fn remote_doctor(
    global: &RemoteGlobal,
    endpoint: &str,
    client: &mut RemoteClient,
) -> Result<(), CliError> {
    let status = client.status()?;
    let discovery = client.discover()?;
    let healthy = status["health"].as_str() != Some("unavailable");
    if global.json {
        emit_json(json!({
            "ok": healthy,
            "backend": "service",
            "control_endpoint": endpoint,
            "control_endpoint_reachable": true,
            "workcell_ref": status["workcell_ref"],
            "health": status["health"],
            "offers": discovery["offers"].as_array().map(Vec::len).unwrap_or(0),
        }));
    } else {
        println!("doctor: {}", if healthy { "healthy" } else { "degraded" });
        println!("  backend: service");
        println!("  control endpoint reachable: yes");
        println!(
            "  workcell: {}",
            status["workcell_ref"].as_str().unwrap_or("unknown")
        );
    }
    if !healthy {
        return Err(WorkcellError::Unavailable(
            "remote Workcell reports unavailable health".into(),
        )
        .into());
    }
    Ok(())
}

fn remote_plan(
    global: &RemoteGlobal,
    args: &[String],
    client: &mut RemoteClient,
) -> Result<(), CliError> {
    let demand = local_cli::parse_remote_demand(args)?;
    let value = client.plan(&demand)?;
    if global.json {
        emit_json(with_ok(value.clone()));
    } else {
        println!(
            "{} — {}",
            value["plan_ref"].as_str().unwrap_or("unknown"),
            value["status"].as_str().unwrap_or("unknown")
        );
        print_degradation(&value);
    }
    if value["status"].as_str() == Some("unsatisfiable") {
        return Err(WorkcellError::UnsatisfiedDemand(
            "remote materialisation plan is unsatisfiable".into(),
        )
        .into());
    }
    Ok(())
}

fn remote_prepare(
    global: &RemoteGlobal,
    args: &[String],
    client: &mut RemoteClient,
) -> Result<(), CliError> {
    let demand = local_cli::parse_remote_demand(args)?;
    let value = client.prepare(&demand)?;
    let world = decode_remote_world(&value)?;
    let receipt = global
        .receipt
        .clone()
        .unwrap_or_else(|| local_cli::receipt_path(&global.state_root, world.world_ref.as_str()));
    local_cli::persist_receipt(&receipt, &world)?;

    if global.json {
        emit_json(json!({"ok": true, "receipt": receipt, "world": value}));
    } else {
        println!("prepared {}", world.world_ref);
        println!("bindings: {}", world.binding_graph.bindings.len());
        println!("receipt: {}", receipt.display());
        println!("backend: service");
    }
    Ok(())
}

fn remote_observe(global: &RemoteGlobal, client: &mut RemoteClient) -> Result<(), CliError> {
    let world_ref = receipt_world_ref(global)?;
    let value = client.observe(&world_ref)?;
    output_bundle(global, value, "observations")
}

fn remote_expose(global: &RemoteGlobal, client: &mut RemoteClient) -> Result<(), CliError> {
    let world_ref = receipt_world_ref(global)?;
    let value = client.expose(&world_ref)?;
    output_bundle(global, value, "surfaces")
}

fn remote_collect(global: &RemoteGlobal, client: &mut RemoteClient) -> Result<(), CliError> {
    let world_ref = receipt_world_ref(global)?;
    let value = client.collect(&world_ref)?;
    output_bundle(global, value, "outputs")
}

fn remote_release(global: &RemoteGlobal, client: &mut RemoteClient) -> Result<(), CliError> {
    let world_ref = receipt_world_ref(global)?;
    let value = client.release(&world_ref)?;
    if global.json {
        emit_json(with_ok(value.clone()));
    } else {
        println!(
            "{} — {}{}",
            value["world_ref"].as_str().unwrap_or(world_ref.as_str()),
            value["disposition"].as_str().unwrap_or("unknown"),
            if value["changed"].as_bool().unwrap_or(false) {
                " (changed)"
            } else {
                ""
            }
        );
    }
    Ok(())
}

fn remote_reconcile(
    global: &RemoteGlobal,
    args: &[String],
    client: &mut RemoteClient,
) -> Result<(), CliError> {
    let _world_ref = receipt_world_ref(global)?;
    let desired = local_cli::parse_remote_desired(args)?;
    let value = client.reconcile(&desired)?;
    if global.json {
        emit_json(with_ok(value.clone()));
    } else if value["deltas"].as_array().is_none_or(Vec::is_empty) {
        println!("reconcile: no delta");
    } else if let Some(deltas) = value["deltas"].as_array() {
        for delta in deltas {
            println!(
                "{}: {} -> {}{}",
                delta["logical_ref"].as_str().unwrap_or("unknown"),
                delta["observed"].as_str().unwrap_or("unknown"),
                delta["desired"].as_str().unwrap_or("unknown"),
                delta["action"]
                    .as_str()
                    .map(|action| format!(" ({action})"))
                    .unwrap_or_default()
            );
        }
    }
    Ok(())
}

fn receipt_world_ref(global: &RemoteGlobal) -> Result<WorldRef, CliError> {
    let receipt = global.receipt.as_ref().ok_or_else(|| {
        WorkcellError::InvalidDemand(
            "this remote command requires `--receipt <material-world.json>`".into(),
        )
    })?;
    let encoded = fs::read_to_string(receipt).map_err(|error| {
        WorkcellError::NotFound(format!(
            "read material-world receipt `{}`: {error}",
            receipt.display()
        ))
    })?;
    Ok(decode_world(&encoded)?.world_ref)
}

fn decode_remote_world(value: &Value) -> Result<MaterialisedExecutionWorld, CliError> {
    let encoded = serde_json::to_string(value).map_err(|error| {
        WorkcellError::OperationFailed(format!("encode remote material-world response: {error}"))
    })?;
    Ok(decode_world(&encoded)?)
}

fn output_bundle(global: &RemoteGlobal, value: Value, collection_key: &str) -> Result<(), CliError> {
    if global.json {
        emit_json(with_ok(value));
    } else {
        println!("{}", value["world_ref"].as_str().unwrap_or("unknown"));
        if let Some(items) = value[collection_key].as_array() {
            if items.is_empty() {
                println!("no {collection_key}");
            } else {
                for item in items {
                    if let Some(logical_ref) = item["logical_ref"].as_str() {
                        println!("{logical_ref}");
                    }
                }
            }
        }
    }
    Ok(())
}

fn print_degradation(value: &Value) {
    if let Some(items) = value["degradations"].as_array() {
        for item in items {
            println!(
                "degraded {} ({}): {}",
                item["requirement"].as_str().unwrap_or("unknown"),
                item["necessity"].as_str().unwrap_or("unknown"),
                item["reason"].as_str().unwrap_or("unknown")
            );
        }
    }
    if let Some(items) = value["omissions"].as_array() {
        for item in items {
            println!(
                "omitted {} ({}): {}",
                item["requirement"].as_str().unwrap_or("unknown"),
                item["necessity"].as_str().unwrap_or("unknown"),
                item["reason"].as_str().unwrap_or("unknown")
            );
        }
    }
}

fn with_ok(value: Value) -> Value {
    match value {
        Value::Object(mut object) => {
            object.insert("ok".into(), Value::Bool(true));
            Value::Object(object)
        }
        other => json!({"ok": true, "value": other}),
    }
}

fn emit_json(value: Value) {
    println!("{value}");
}

fn print_combined_help() {
    local_cli::print_help_for_selector();
    println!(
        "\nRemote backend:\n  --endpoint HOST:PORT       Use the same Workcell commands through workcell.control/v1\n  --authorization TOKEN      Control-service credential (prefer WORKCELL_CONTROL_TOKEN)\n\nEnvironment:\n  WORKCELL_CONTROL_ENDPOINT  Default remote endpoint\n  WORKCELL_CONTROL_TOKEN     Default remote authorization credential\n\nWithout a remote endpoint, `workcell` remains the zero-daemon collapsed-local Workcell. Endpoint, TCP route, SSH tunnel or private fabric are material routing facts and never Workcell or caller semantic identity."
    );
}

fn report_error(error: CliError, json: bool) -> ExitCode {
    if json {
        eprintln!(
            "{}",
            json!({
                "ok": false,
                "error": {
                    "kind": error_kind(&error),
                    "message": error.to_string(),
                }
            })
        );
    } else {
        eprintln!("workcell: {error}");
    }
    ExitCode::from(exit_code(&error))
}

fn error_kind(error: &CliError) -> &'static str {
    match error {
        CliError::Workcell(error) => workcell_error_kind(error),
        CliError::Control(ControlClientError::TransportUnavailable(_)) => "transport-unavailable",
        CliError::Control(ControlClientError::ProtocolIncompatible(_)) => "protocol-incompatible",
        CliError::Control(ControlClientError::AuthenticationFailed(_)) => "authentication-failed",
        CliError::Control(ControlClientError::Remote(error)) => workcell_error_kind(error),
        CliError::Control(ControlClientError::InvalidResponse(_)) => "invalid-control-response",
    }
}

fn workcell_error_kind(error: &WorkcellError) -> &'static str {
    match error {
        WorkcellError::InvalidDemand(_) => "invalid-demand",
        WorkcellError::UnsatisfiedDemand(_) => "unsatisfied-demand",
        WorkcellError::Unavailable(_) => "unavailable",
        WorkcellError::Degraded(_) => "degraded",
        WorkcellError::OperationFailed(_) => "operation-failed",
        WorkcellError::CleanupFailed(_) => "cleanup-failed",
        WorkcellError::ReconciliationFailed(_) => "reconciliation-failed",
        WorkcellError::NotFound(_) => "not-found",
        WorkcellError::Unsupported(_) => "unsupported",
    }
}

fn exit_code(error: &CliError) -> u8 {
    match error {
        CliError::Workcell(error) | CliError::Control(ControlClientError::Remote(error)) => {
            workcell_exit_code(error)
        }
        CliError::Control(ControlClientError::TransportUnavailable(_)) => 8,
        CliError::Control(ControlClientError::ProtocolIncompatible(_)) => 9,
        CliError::Control(ControlClientError::AuthenticationFailed(_)) => 10,
        CliError::Control(ControlClientError::InvalidResponse(_)) => 7,
    }
}

fn workcell_exit_code(error: &WorkcellError) -> u8 {
    match error {
        WorkcellError::InvalidDemand(_) => 2,
        WorkcellError::UnsatisfiedDemand(_) => 3,
        WorkcellError::Unavailable(_) | WorkcellError::Degraded(_) => 4,
        WorkcellError::NotFound(_) => 5,
        WorkcellError::Unsupported(_) => 6,
        WorkcellError::OperationFailed(_)
        | WorkcellError::CleanupFailed(_)
        | WorkcellError::ReconciliationFailed(_) => 7,
    }
}
