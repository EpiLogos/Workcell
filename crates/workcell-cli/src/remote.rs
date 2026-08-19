use std::{collections::BTreeSet, env, fs};

use epilogos_workcell_control::{ControlClient, TcpControlTransport};
use epilogos_workcell_core::{PlanStatus, WorkcellError, WorldRef};
use epilogos_workcell_wire::decode_world;
use serde_json::{json, Map, Value};

use super::{
    default_receipt_path, parse_demand, parse_desired, write_receipt, CliError, GlobalArgs,
};

pub fn run(global: &GlobalArgs, command: &str, args: &[String]) -> Result<(), CliError> {
    if global.workspace_source.is_some() {
        return Err(WorkcellError::InvalidDemand(
            "--workspace-source is a local material binding and cannot be projected onto a remote Workcell; bind the source on the remote Workcell instead"
                .into(),
        )
        .into());
    }

    let mut client = client(global)?;
    match command {
        "status" => status(global, &mut client),
        "discover" => discover(global, &mut client),
        "providers" => providers(global, &mut client),
        "doctor" => doctor(global, &mut client),
        "plan" => plan(global, args, &mut client),
        "prepare" => prepare(global, args, &mut client),
        "observe" => observe(global, &mut client),
        "expose" => expose(global, &mut client),
        "collect" => collect(global, &mut client),
        "release" => release(global, &mut client),
        "reconcile" => reconcile(global, args, &mut client),
        other => Err(WorkcellError::InvalidDemand(format!(
            "unknown command `{other}`; run `workcell help`"
        ))
        .into()),
    }
}

fn client(global: &GlobalArgs) -> Result<ControlClient<TcpControlTransport>, CliError> {
    let endpoint = global.endpoint.as_ref().ok_or_else(|| {
        WorkcellError::InvalidDemand("remote backend requires --endpoint HOST:PORT".into())
    })?;
    let mut client = ControlClient::new(TcpControlTransport::new(endpoint.clone()));
    if let Some(authorization) = &global.authorization {
        client = client.with_authorization(authorization.clone());
    }
    Ok(client)
}

fn status(
    global: &GlobalArgs,
    client: &mut ControlClient<TcpControlTransport>,
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
        println!(
            "control endpoint: {}",
            global.endpoint.as_deref().unwrap_or("unknown")
        );
    }
    Ok(())
}

fn discover(
    global: &GlobalArgs,
    client: &mut ControlClient<TcpControlTransport>,
) -> Result<(), CliError> {
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

fn providers(
    global: &GlobalArgs,
    client: &mut ControlClient<TcpControlTransport>,
) -> Result<(), CliError> {
    let discovery = client.discover()?;
    let mut provider_refs = BTreeSet::new();
    if let Some(offers) = discovery["offers"].as_array() {
        for offer in offers {
            if let Some(provider_ref) = offer["provider_ref"].as_str() {
                provider_refs.insert(provider_ref.to_owned());
            }
        }
    }
    if global.json {
        emit_json(json!({
            "ok": true,
            "workcell_ref": discovery["workcell_ref"],
            "providers": provider_refs,
        }));
    } else {
        for provider in provider_refs {
            println!("{provider}");
        }
    }
    Ok(())
}

fn doctor(
    global: &GlobalArgs,
    client: &mut ControlClient<TcpControlTransport>,
) -> Result<(), CliError> {
    let status = client.status()?;
    let discovery = client.discover()?;
    let healthy = status["health"] != "unavailable";
    if global.json {
        emit_json(json!({
            "ok": healthy,
            "backend": "service",
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
    Ok(())
}

fn plan(
    global: &GlobalArgs,
    args: &[String],
    client: &mut ControlClient<TcpControlTransport>,
) -> Result<(), CliError> {
    let demand = parse_demand(args)?;
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
    if value["status"] == plan_status_value(PlanStatus::Unsatisfiable) {
        return Err(WorkcellError::UnsatisfiedDemand(
            "remote materialisation plan is unsatisfiable".into(),
        )
        .into());
    }
    Ok(())
}

fn prepare(
    global: &GlobalArgs,
    args: &[String],
    client: &mut ControlClient<TcpControlTransport>,
) -> Result<(), CliError> {
    let demand = parse_demand(args)?;
    let value = client.prepare(&demand)?;
    let world = decode_world(&serde_json::to_string(&value).map_err(|error| {
        WorkcellError::OperationFailed(format!("encode remote material-world response: {error}"))
    })?)?;
    let receipt = global
        .receipt
        .clone()
        .unwrap_or_else(|| default_receipt_path(&global.state_root, world.world_ref.as_str()));
    write_receipt(&receipt, &world)?;

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

fn observe(
    global: &GlobalArgs,
    client: &mut ControlClient<TcpControlTransport>,
) -> Result<(), CliError> {
    let world_ref = receipt_world_ref(global)?;
    let value = client.observe(&world_ref)?;
    output_bundle(global, value, "observations")
}

fn expose(
    global: &GlobalArgs,
    client: &mut ControlClient<TcpControlTransport>,
) -> Result<(), CliError> {
    let world_ref = receipt_world_ref(global)?;
    let value = client.expose(&world_ref)?;
    output_bundle(global, value, "surfaces")
}

fn collect(
    global: &GlobalArgs,
    client: &mut ControlClient<TcpControlTransport>,
) -> Result<(), CliError> {
    let world_ref = receipt_world_ref(global)?;
    let value = client.collect(&world_ref)?;
    output_bundle(global, value, "outputs")
}

fn release(
    global: &GlobalArgs,
    client: &mut ControlClient<TcpControlTransport>,
) -> Result<(), CliError> {
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

fn reconcile(
    global: &GlobalArgs,
    args: &[String],
    client: &mut ControlClient<TcpControlTransport>,
) -> Result<(), CliError> {
    let desired = parse_desired(args)?;
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

fn receipt_world_ref(global: &GlobalArgs) -> Result<WorldRef, CliError> {
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

fn output_bundle(global: &GlobalArgs, value: Value, collection_key: &str) -> Result<(), CliError> {
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

fn plan_status_value(status: PlanStatus) -> Value {
    Value::String(
        match status {
            PlanStatus::Satisfiable => "satisfiable",
            PlanStatus::Degraded => "degraded",
            PlanStatus::Unsatisfiable => "unsatisfiable",
        }
        .into(),
    )
}
