use std::{
    collections::{BTreeMap, BTreeSet},
    env, fs,
    path::{Path, PathBuf},
    process::ExitCode,
    time::{SystemTime, UNIX_EPOCH},
};

use epilogos_workcell_control::{
    check_compatibility, credential_sha256, generate_credential, grant_ref_for, parse_duration_millis,
    software_version, validate_label, ConnectionGrants, ControlClient, ControlClientError,
    ControlService, CreateOutcome, ProjectionDecision, RemoteMachineDeclaration, RemoteMachineRegistry,
    SecretProjectionLedger, SecretProjectionRecord, TcpControlServer, TcpControlTransport,
    CONTROL_OPERATIONS, CONTROL_PROTOCOL_VERSION, GRANTS_FILE,
};
use epilogos_workcell_core::{
    broker_handle, correlate_projection, AffordanceRequirement, Availability, BindingRef,
    BrokerPolicy, BrokerRoute, CollectionBundle, Degradation, DemandRef, DesiredMaterialState,
    Discovery, ExecutionDemand, ExposureBundle, ExposureRequirement, ExternalRef, HealthState,
    IsolationTrustRequirement, LogicalConnectionRequirement, MaterialisationPlan,
    MaterialisedExecutionWorld, ObservationBundle, OutputRequirement, PersistenceScope, PlanOmission,
    PlanStatus, ProjectRuntimeRequirement, ProjectionCorrelation, ProviderAllocation,
    ProviderPortKind, ReconciliationResult, ReleaseDisposition, ReleaseResult, RequirementNecessity,
    ResourceRequirement, RetentionExpectation, SecretMaterialisationClass,
    SecretMaterialisationRequest, SecretProjectionRequest, SecretProjectionTarget,
    SecretRevocationState, Tiered, WorkcellControlPlane, WorkcellError, WorkcellRef, WorkspaceAccess,
    WorkspaceRequirement, WorldRef, AIKIT_WORKTREE_PROJECTION_SOURCE,
};
use epilogos_workcell_keychain::{
    store_bootstrap_material as store_keychain_material, KeychainAclPolicy, KeychainSecretProvider,
};
use epilogos_workcell_onepassword::{OnePasswordCli, OnePasswordSecretProvider};
use epilogos_workcell_secret_scan as secret_scan;
use epilogos_workcell_secret_service::{
    store_bootstrap_material as store_secret_service_material, SecretServiceSecretProvider,
};
use epilogos_workcell_opensandbox::{
    project_credential_to_sandbox, OpenSandboxCredentialAuth, OpenSandboxCredentialBindingSpec,
    OpenSandboxCredentialBroker, OpenSandboxConfig, StdHttpOpenSandboxTransport,
};
use epilogos_workcell_runtime::{
    compose_prepared_run_scope, set_run_status, CollapsedLocalConfig, CollapsedLocalWorkcell,
    RunLedger,
};
use epilogos_workcell_wire::{
    connection_value, correlated_observation_value, decode_connection, decode_world,
    encode_connection, encode_world, world_value, ConnectionGrant, ConnectionRecord,
};
use serde_json::{json, Map, Value};

const DEFAULT_WORKCELL_REF: &str = "workcell:local";
const DEFAULT_DEMAND_REF: &str = "demand:cli";

#[derive(Debug, Clone)]
struct GlobalArgs {
    json: bool,
    state_root: PathBuf,
    workcell_ref: String,
    receipt: Option<PathBuf>,
    workspace_source: Option<PathBuf>,
    services: Option<PathBuf>,
    remaining: Vec<String>,
}

fn main() -> ExitCode {
    let args = env::args().skip(1).collect::<Vec<_>>();
    let json_requested = args.iter().any(|arg| arg == "--json");
    match run(args) {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            if json_requested {
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
    }
}

fn run(args: Vec<String>) -> Result<(), WorkcellError> {
    let global = parse_global(args)?;
    let Some(command) = global.remaining.first().map(String::as_str) else {
        print_help();
        return Ok(());
    };
    let command_args = &global.remaining[1..];

    match command {
        "help" | "-h" | "--help" => {
            print_help();
            Ok(())
        }
        "status" => command_status(&global),
        "discover" => command_discover(&global),
        "providers" => command_providers(&global),
        "doctor" => command_doctor(&global),
        "plan" => command_plan(&global, command_args),
        "prepare" => command_prepare(&global, command_args),
        "observe" => command_observe(&global),
        "inspect" => command_material(&global),
        "recover" => command_recover(&global),
        "expose" => command_expose(&global),
        "material" => command_material(&global),
        "collect" => command_collect(&global),
        "release" => command_release(&global),
        "reconcile" => command_reconcile(&global, command_args),
        "correlate-projection" => command_correlate_projection(&global, command_args),
        "instances" => command_instances(&global, command_args),
        "places" => command_places(&global),
        "place" => command_place(&global, command_args),
        "sandboxes" => command_sandboxes(&global, command_args),
        "run" => command_run(&global, command_args),
        "serve" => command_serve(&global, command_args),
        "authorise" => command_authorise(&global, command_args),
        "revoke" => command_revoke(&global, command_args),
        "connect" => command_connect(&global, command_args),
        "connections" => command_connections(&global, command_args),
        "secret" => command_secret(&global, command_args),
        "machine" => command_machine(&global, command_args),
        "system" => command_system(&global),
        "config" => command_config(&global, command_args),
        "config-contribution" => command_config_contribution(&global),
        other if other.starts_with('-') => Err(WorkcellError::InvalidDemand(format!(
            "unknown option `{other}`; run `workcell help`"
        ))),
        other => Err(WorkcellError::InvalidDemand(format!(
            "unknown command `{other}`; run `workcell help`"
        ))),
    }
}

fn parse_global(args: Vec<String>) -> Result<GlobalArgs, WorkcellError> {
    let mut json = false;
    let mut state_root = None;
    let mut workcell_ref = DEFAULT_WORKCELL_REF.to_owned();
    let mut receipt = None;
    let mut workspace_source = None;
    let mut services = None;
    let mut remaining = Vec::new();
    let mut index = 0;

    while index < args.len() {
        match args[index].as_str() {
            "--json" => {
                json = true;
                index += 1;
            }
            "--state-root" => {
                state_root = Some(PathBuf::from(require_value(&args, index, "--state-root")?));
                index += 2;
            }
            "--workcell-ref" => {
                workcell_ref = require_value(&args, index, "--workcell-ref")?.to_owned();
                index += 2;
            }
            "--receipt" => {
                receipt = Some(PathBuf::from(require_value(&args, index, "--receipt")?));
                index += 2;
            }
            "--workspace-source" => {
                workspace_source = Some(PathBuf::from(require_value(
                    &args,
                    index,
                    "--workspace-source",
                )?));
                index += 2;
            }
            "--services" => {
                services = Some(PathBuf::from(require_value(&args, index, "--services")?));
                index += 2;
            }
            _ => {
                remaining.push(args[index].clone());
                index += 1;
            }
        }
    }

    Ok(GlobalArgs {
        json,
        state_root: state_root.unwrap_or_else(default_state_root),
        workcell_ref,
        receipt,
        workspace_source,
        services,
        remaining,
    })
}

fn command_status(global: &GlobalArgs) -> Result<(), WorkcellError> {
    let workcell = new_local(
        global,
        parse_workcell_ref(&global.workcell_ref)?,
        BTreeSet::new(),
    )?;
    let discovery = workcell.discover()?;
    let receipts = receipt_count(&global.state_root)?;
    let connections = list_connection_records(&global.state_root)?;
    let grants_summary = grants_status_summary(global)?;
    if global.json {
        emit_json(json!({
            "ok": true,
            "workcell_ref": discovery.workcell_ref.as_str(),
            "health": health(&discovery.health),
            "providers": provider_count(&discovery),
            "offers": discovery.offers.len(),
            "persisted_world_receipts": receipts,
            "connections": connections.iter().map(connection_status_json).collect::<Vec<_>>(),
            "connection_grants": grants_summary.map(|(active, expired, revoked)| json!({
                "active": active,
                "expired": expired,
                "revoked": revoked,
            })),
            "state_root": global.state_root,
        }));
    } else {
        println!("Workcell {}", discovery.workcell_ref);
        println!("health: {}", health(&discovery.health));
        println!("providers: {}", provider_count(&discovery));
        println!("offers: {}", discovery.offers.len());
        println!("persisted worlds: {receipts}");
        if connections.is_empty() {
            println!("connections: none");
        } else {
            println!("connections: {}", connections.len());
            for record in &connections {
                println!(
                    "  {} [{}] -> {} ({}){}",
                    record.label,
                    record.state,
                    record.endpoint,
                    record.remote_workcell_ref.as_deref().unwrap_or("remote identity unknown"),
                    expiry_note(record.expires_at_unix_ms),
                );
            }
        }
        if let Some((active, expired, revoked)) = grants_summary {
            print!("connection grants: {active} active");
            if expired > 0 {
                print!(", {expired} expired");
            }
            println!(", {revoked} revoked");
        }
        println!("state root: {}", global.state_root.display());
    }
    Ok(())
}

/// The ` expires at …` suffix naming when a connection's grant stops
/// authorising; empty for grants without an expiry.
fn expiry_note(expires_at_unix_ms: Option<u64>) -> String {
    match expires_at_unix_ms {
        Some(expires_at) => format!(" expires at {expires_at} (unix ms)"),
        None => String::new(),
    }
}

fn connection_status_json(record: &ConnectionRecord) -> Value {
    json!({
        "label": record.label,
        "state": record.state,
        "endpoint": record.endpoint,
        "protocol": record.protocol,
        "remote_workcell_ref": record.remote_workcell_ref,
        "expires_at_unix_ms": record.expires_at_unix_ms,
    })
}

/// Active/expired/revoked grant counts when this state root holds a grants
/// registry. "Active" counts grants that still authorise: an expired grant
/// is kept and counted as expired, not silently active.
fn grants_status_summary(
    global: &GlobalArgs,
) -> Result<Option<(usize, usize, usize)>, WorkcellError> {
    let registry = ConnectionGrants::new(&global.state_root, parse_workcell_ref(&global.workcell_ref)?);
    if !registry.path().exists() {
        return Ok(None);
    }
    let now = now_unix_ms();
    let mut active = 0;
    let mut expired = 0;
    let mut revoked = 0;
    for grant in registry.list()? {
        if !grant.is_active() {
            revoked += 1;
        } else if grant.is_expired_at(now) {
            expired += 1;
        } else {
            active += 1;
        }
    }
    Ok(Some((active, expired, revoked)))
}

fn command_discover(global: &GlobalArgs) -> Result<(), WorkcellError> {
    let workcell = new_local(
        global,
        parse_workcell_ref(&global.workcell_ref)?,
        BTreeSet::new(),
    )?;
    let discovery = workcell.discover()?;
    if global.json {
        emit_json(discovery_json(&discovery));
    } else {
        println!("{} — {}", discovery.workcell_ref, health(&discovery.health));
        for offer in &discovery.offers {
            println!(
                "{} [{}] {} / {}",
                offer.provider_ref,
                offer.port,
                availability(&offer.availability),
                health(&offer.health)
            );
            if !offer.affordances.is_empty() {
                println!("  affordances: {}", offer.affordances.join(", "));
            }
        }
    }
    Ok(())
}

fn command_providers(global: &GlobalArgs) -> Result<(), WorkcellError> {
    let workcell = new_local(
        global,
        parse_workcell_ref(&global.workcell_ref)?,
        BTreeSet::new(),
    )?;
    let discovery = workcell.discover()?;
    let mut providers: BTreeMap<String, Vec<&epilogos_workcell_core::OperationalOffer>> =
        BTreeMap::new();
    for offer in &discovery.offers {
        providers
            .entry(offer.provider_ref.to_string())
            .or_default()
            .push(offer);
    }

    if global.json {
        let values = providers
            .iter()
            .map(|(provider, offers)| {
                json!({
                    "provider_ref": provider,
                    "ports": offers.iter().map(|offer| offer.port.as_str()).collect::<Vec<_>>(),
                    "offers": offers.iter().map(|offer| offer.offer_ref.as_str()).collect::<Vec<_>>(),
                    "health": offers.iter().map(|offer| health(&offer.health)).collect::<Vec<_>>(),
                })
            })
            .collect::<Vec<_>>();
        emit_json(json!({"ok": true, "providers": values}));
    } else {
        for (provider, offers) in providers {
            let ports = offers
                .iter()
                .map(|offer| offer.port.as_str())
                .collect::<BTreeSet<_>>()
                .into_iter()
                .collect::<Vec<_>>()
                .join(", ");
            println!("{provider} — {ports}");
        }
    }
    Ok(())
}

fn command_doctor(global: &GlobalArgs) -> Result<(), WorkcellError> {
    fs::create_dir_all(&global.state_root).map_err(|error| {
        WorkcellError::OperationFailed(format!("create state root for doctor: {error}"))
    })?;
    let probe = global
        .state_root
        .join(format!(".doctor-{}", std::process::id()));
    fs::write(&probe, b"workcell-doctor\n").map_err(|error| {
        WorkcellError::Unavailable(format!("state root is not writable: {error}"))
    })?;
    fs::remove_file(&probe).map_err(|error| {
        WorkcellError::OperationFailed(format!("remove doctor write probe: {error}"))
    })?;

    let workcell = new_local(
        global,
        parse_workcell_ref(&global.workcell_ref)?,
        BTreeSet::new(),
    )?;
    let discovery = workcell.discover()?;
    let shell = discovery
        .offers
        .iter()
        .any(|offer| offer.affordances.iter().any(|value| value == "shell"));
    let writable_workspace = discovery.offers.iter().any(|offer| {
        offer
            .affordances
            .iter()
            .any(|value| value == "workspace:writable")
    });
    let filesystem_artifacts = discovery.offers.iter().any(|offer| {
        offer.port == ProviderPortKind::ArtifactStorage.as_str()
            && offer.availability == Availability::Available
    });

    if !(shell && writable_workspace && filesystem_artifacts) {
        return Err(WorkcellError::Unavailable(
            "collapsed-local baseline is incomplete".into(),
        ));
    }

    let scan = epilogos_workcell_runtime::scan_live(
        &global.state_root,
        &parse_workcell_ref(&global.workcell_ref)?,
        "actuation",
    );

    // Declared services are not part of the zero-setup baseline, so they never
    // make doctor fail. They are reported because a declared service that does
    // not answer is the thing an operator most needs to see.
    let services = declared_service_report(&discovery);

    if global.json {
        emit_json(json!({
            "ok": true,
            "state_root_writable": true,
            "shell": shell,
            "writable_workspace": writable_workspace,
            "filesystem_artifacts": filesystem_artifacts,
            "optional_external_providers_required": false,
            "declared_services": services,
            "instances": {
                "status": scan.status,
                "reason": scan.reason,
                "live": scan.live.len(),
                "stale": scan.stale.len(),
            },
        }));
    } else {
        println!("doctor: healthy");
        println!("  state root writable: yes");
        println!("  host-process execution: yes");
        println!("  writable local workspace: yes");
        println!("  local artifact storage: yes");
        println!("  Docker/Arrakis/Tailscale required: no");
        if services.is_empty() {
            println!("  declared services: none");
        } else {
            println!("  declared services: {}", services.len());
            for service in &services {
                println!(
                    "    {} — {} ({}, {})",
                    service["logical_ref"].as_str().unwrap_or("?"),
                    service["availability"].as_str().unwrap_or("?"),
                    service["lifetime"].as_str().unwrap_or("?"),
                    service["endpoint"].as_str().unwrap_or("no endpoint"),
                );
            }
        }
        match scan.status {
            "ok" => println!(
                "  harness instances: {} live, {} stale",
                scan.live.len(),
                scan.stale.len()
            ),
            other => println!(
                "  harness instances: unavailable ({})",
                scan.reason.unwrap_or_else(|| other.to_owned())
            ),
        }
    }
    Ok(())
}

/// What the service port is actually offering, from discovery alone.
fn declared_service_report(discovery: &Discovery) -> Vec<Value> {
    discovery
        .offers
        .iter()
        .filter(|offer| offer.port == ProviderPortKind::Service.as_str())
        .map(|offer| {
            json!({
                "logical_ref": offer.connections.first().map(String::as_str).unwrap_or(""),
                "provider_ref": offer.provider_ref.as_str(),
                "availability": availability(&offer.availability),
                "health": health(&offer.health),
                "lifetime": offer.metadata.get("lifetime").map(String::as_str).unwrap_or("unknown"),
                "endpoint": offer.metadata.get("endpoint").map(String::as_str),
            })
        })
        .collect()
}

fn command_plan(global: &GlobalArgs, args: &[String]) -> Result<(), WorkcellError> {
    let demand = parse_demand(args)?;
    let channels = demand_output_channels(&demand);
    let workcell = new_local(global, parse_workcell_ref(&global.workcell_ref)?, channels)?;
    let plan = workcell.plan(&demand)?;
    if global.json {
        emit_json(plan_json(&plan));
    } else {
        print_plan(&plan);
    }
    if plan.status == PlanStatus::Unsatisfiable {
        return Err(WorkcellError::UnsatisfiedDemand(
            "materialisation plan is unsatisfiable".into(),
        ));
    }
    Ok(())
}

fn command_prepare(global: &GlobalArgs, args: &[String]) -> Result<(), WorkcellError> {
    let demand = parse_demand(args)?;
    let channels = demand_output_channels(&demand);
    let mut workcell = new_local(global, parse_workcell_ref(&global.workcell_ref)?, channels)?;
    let world = workcell.prepare(&demand)?;
    let receipt = global
        .receipt
        .clone()
        .unwrap_or_else(|| default_receipt_path(&global.state_root, world.world_ref.as_str()));
    write_receipt(&receipt, &world)?;

    if global.json {
        emit_json(json!({
            "ok": true,
            "receipt": receipt,
            "world": world_value(&world)?,
        }));
    } else {
        println!("prepared {}", world.world_ref);
        println!("bindings: {}", world.binding_graph.bindings.len());
        println!("state: {}", health(&world.state));
        println!("receipt: {}", receipt.display());
        if !world.plan_degradations.is_empty() {
            println!("degradations: {}", world.plan_degradations.len());
        }
        if !world.plan_omissions.is_empty() {
            println!("omissions: {}", world.plan_omissions.len());
        }
    }
    Ok(())
}

fn command_recover(global: &GlobalArgs) -> Result<(), WorkcellError> {
    let (mut workcell, world, _) = resume(global)?;
    let recovered = workcell.recover(&world.world_ref)?;
    let receipt = default_receipt_path(&global.state_root, recovered.world_ref.as_str());
    write_receipt(&receipt, &recovered)?;
    if global.json { emit_json(json!({"ok":true,"receipt":receipt,"world":world_value(&recovered)?})); }
    else { println!("material recovery: {} -> {}; receipt {}", world.world_ref, recovered.world_ref, receipt.display()); }
    Ok(())
}

fn command_observe(global: &GlobalArgs) -> Result<(), WorkcellError> {
    let (workcell, world, _) = resume(global)?;
    let result = workcell.observe(&world.world_ref)?;
    if global.json {
        emit_json(observation_json(&result));
    } else {
        println!("{}", result.world_ref);
        for observation in result.observations {
            println!(
                "{} — {}",
                observation.logical_ref,
                health(&observation.state)
            );
            for (key, value) in observation.detail {
                println!("  {key}: {value}");
            }
        }
    }
    Ok(())
}

fn command_expose(global: &GlobalArgs) -> Result<(), WorkcellError> {
    let (workcell, world, _) = resume(global)?;
    let result = workcell.expose(&world.world_ref)?;
    if global.json {
        emit_json(exposure_json(&result));
    } else {
        println!("{}", result.world_ref);
        if result.surfaces.is_empty() {
            println!("no material exposure surfaces");
        }
        for surface in result.surfaces {
            println!("{} — {}", surface.logical_ref, surface.interaction);
            for (key, value) in surface.material {
                println!("  {key}: {value}");
            }
        }
        print_degradations(&result.degradations, &result.omissions);
    }
    Ok(())
}

fn command_material(global: &GlobalArgs) -> Result<(), WorkcellError> {
    let (workcell, world, _) = resume(global)?;
    let observed = workcell.observe(&world.world_ref);
    let exposed = workcell.expose(&world.world_ref);
    let bodies = exposed.as_ref().ok().map(|bundle| {
        bundle
            .surfaces
            .iter()
            .map(|surface| {
                json!({
                    "logical_ref": surface.logical_ref,
                    "interaction": surface.interaction,
                    "material": surface.material,
                    "provenance": surface.provenance,
                })
            })
            .collect::<Vec<_>>()
    });

    emit_json(json!({
        "ok": true,
        "contract": "workcell.material-reading/v1",
        "backend": "native-cli",
        "consistency": "sequential-not-atomic",
        "receipt_world": world_value(&world)?,
        "observation": material_outcome(observed.map(|bundle| observation_json(&bundle))),
        "exposure": material_outcome(exposed.map(|bundle| exposure_json(&bundle))),
        "bodies": bodies,
    }));
    Ok(())
}

fn material_outcome(result: Result<Value, WorkcellError>) -> Value {
    let completed_at_unix_ms = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_millis() as u64)
        .unwrap_or(0);
    match result {
        Ok(reading) => json!({
            "status": "supplied",
            "completed_at_unix_ms": completed_at_unix_ms,
            "reading": reading,
        }),
        Err(error) => json!({
            "status": "error",
            "completed_at_unix_ms": completed_at_unix_ms,
            "error": {
                "kind": error_kind(&error),
                "message": error.to_string(),
            },
        }),
    }
}

fn command_collect(global: &GlobalArgs) -> Result<(), WorkcellError> {
    let (workcell, world, _) = resume(global)?;
    let result = workcell.collect(&world.world_ref)?;
    if global.json {
        emit_json(collection_json(&result));
    } else {
        println!("{}", result.world_ref);
        if result.outputs.is_empty() {
            println!("no collected outputs");
        }
        for output in result.outputs {
            println!("{} -> {}", output.logical_ref, output.material_locator);
        }
        print_degradations(&result.degradations, &result.omissions);
    }
    Ok(())
}

fn command_release(global: &GlobalArgs) -> Result<(), WorkcellError> {
    let (mut workcell, world, receipt) = resume(global)?;
    let result = workcell.release(&world.world_ref)?;
    if let Some(updated) = workcell.world(&world.world_ref) {
        write_receipt(&receipt, updated)?;
    }
    if global.json {
        emit_json(release_json(&result));
    } else {
        println!(
            "{} — {}{}",
            result.world_ref,
            release_disposition(&result.disposition),
            if result.changed { " (changed)" } else { "" }
        );
    }
    Ok(())
}

fn command_reconcile(global: &GlobalArgs, args: &[String]) -> Result<(), WorkcellError> {
    let desired = parse_desired(args)?;
    let (mut workcell, world, receipt) = resume(global)?;
    let result = workcell.reconcile(&desired)?;
    if let Some(updated) = workcell.world(&world.world_ref) {
        write_receipt(&receipt, updated)?;
    }
    if global.json {
        emit_json(reconciliation_json(&result));
    } else if result.deltas.is_empty() {
        println!("reconcile: no delta");
    } else {
        for delta in result.deltas {
            println!(
                "{}: {} -> {}{}",
                delta.logical_ref,
                delta.observed.as_deref().unwrap_or("unknown"),
                delta.desired,
                delta
                    .action
                    .as_deref()
                    .map(|action| format!(" ({action})"))
                    .unwrap_or_default()
            );
        }
    }
    Ok(())
}

// ---- Projection correlation -------------------------------------------
//
// Surface an AIKit git-projection verdict (`aikit worktree project --json`,
// schema `aikit.worktree-projection/v1`) as a correlated observation on a
// prepared Workcell material world. Workcell owns material lifecycle, not git:
// it carries AIKit's verdict verbatim, attributed to AIKit, correlated to an
// opaque checkout subject on the world. It runs no git, re-derives nothing, and
// does not touch material lifecycle — this is an annotation over a world
// reading, never a `reconcile` that computes git.
fn command_correlate_projection(
    global: &GlobalArgs,
    args: &[String],
) -> Result<(), WorkcellError> {
    let mut projection_path = None;
    let mut subject_key = None;
    let mut index = 0;
    while index < args.len() {
        let flag = args[index].as_str();
        match flag {
            "--projection" => {
                projection_path = Some(PathBuf::from(require_value(args, index, "--projection")?))
            }
            "--subject" => subject_key = Some(require_value(args, index, "--subject")?.to_owned()),
            unknown => {
                return Err(WorkcellError::InvalidDemand(format!(
                    "unknown correlate-projection option `{unknown}`"
                )))
            }
        }
        index += 2;
    }

    let projection_path = projection_path.ok_or_else(|| {
        WorkcellError::InvalidDemand(
            "correlate-projection requires `--projection <aikit-worktree-projection.json>`".into(),
        )
    })?;
    let subject_key = subject_key.ok_or_else(|| {
        WorkcellError::InvalidDemand(
            "correlate-projection requires `--subject <world-subject-key>` (e.g. checkout:workcell)"
                .into(),
        )
    })?;

    // The material world the verdict annotates — read from its receipt, as
    // observe/reconcile resume a prepared world. No control plane is built:
    // correlation is a pure annotation, not a material operation.
    let world = load_world_receipt(global)?;

    // Read AIKit's already-rendered verdict into the opaque carrier. This runs
    // no git and inspects no ancestry/cleanliness/branch — it reads the action
    // label AIKit stamped on each entry.
    let correlation = read_aikit_projection_correlation(&projection_path, subject_key)?;

    let observation = correlate_projection(&world, correlation)?;

    if global.json {
        let mut document = correlated_observation_value(&observation);
        if let Some(object) = document.as_object_mut() {
            object.insert("ok".to_string(), Value::Bool(true));
        }
        emit_json(document);
    } else {
        println!(
            "{} — projection verdict for `{}` (via {})",
            observation.world_ref, observation.subject_key, observation.attributed_to
        );
        println!("  target: {}", observation.correlation.target);
        println!("  projected: {}", observation.correlation.projected);
        println!(
            "  mode: {}",
            if observation.correlation.applied {
                "apply"
            } else {
                "observe"
            }
        );
        if !observation.correlation.surfaced.is_empty() {
            println!(
                "  surfaced (needs attention): {}",
                observation.correlation.surfaced.join(", ")
            );
        }
        if !observation.correlation.summary.is_empty() {
            for line in observation.correlation.summary.lines() {
                println!("  {line}");
            }
        }
    }
    Ok(())
}

/// Read a prepared material world from `--receipt`, without building a control
/// plane. Correlation is a reading over the world's subjects, not a lifecycle op.
fn load_world_receipt(global: &GlobalArgs) -> Result<MaterialisedExecutionWorld, WorkcellError> {
    let receipt = global.receipt.clone().ok_or_else(|| {
        WorkcellError::InvalidDemand(
            "this command requires `--receipt <material-world.json>`".into(),
        )
    })?;
    let encoded = fs::read_to_string(&receipt).map_err(|error| {
        WorkcellError::NotFound(format!(
            "read material-world receipt `{}`: {error}",
            receipt.display()
        ))
    })?;
    decode_world(&encoded)
}

/// Translate an AIKit `SuiteProjection` reading into the opaque
/// `ProjectionCorrelation` carrier. This is caller-side plumbing: it reads the
/// per-entry action labels AIKit already stamped and the envelope's
/// `target`/`applied`/`version`, and carries them. It performs no git — no
/// fetch, no ancestry, no cleanliness, no branch resolution — and it never
/// re-decides an entry's verdict; it only tallies AIKit's own labels into the
/// suite reading the carrier holds.
fn read_aikit_projection_correlation(
    path: &Path,
    subject_key: String,
) -> Result<ProjectionCorrelation, WorkcellError> {
    let raw = fs::read(path).map_err(|error| {
        WorkcellError::InvalidDemand(format!("read projection JSON `{}`: {error}", path.display()))
    })?;
    if raw.len() > 4_194_304 {
        return Err(WorkcellError::InvalidDemand(
            "projection JSON exceeds 4 MiB".into(),
        ));
    }
    let value: Value = serde_json::from_slice(&raw)
        .map_err(|error| WorkcellError::InvalidDemand(format!("parse projection JSON: {error}")))?;

    // `aikit worktree project --json` wraps the SuiteProjection in a reply
    // envelope (under `data`); a caller may also pass the SuiteProjection bare.
    // Either way we key off its own version tag — Workcell never invents the
    // shape.
    let suite = locate_projection_object(&value).ok_or_else(|| {
        WorkcellError::InvalidDemand(format!(
            "input carries no `{AIKIT_WORKTREE_PROJECTION_SOURCE}` projection object"
        ))
    })?;

    let source = suite
        .get("version")
        .and_then(Value::as_str)
        .unwrap_or(AIKIT_WORKTREE_PROJECTION_SOURCE)
        .to_owned();
    let target = suite
        .get("target")
        .and_then(Value::as_str)
        .ok_or_else(|| WorkcellError::InvalidDemand("projection is missing `target`".into()))?
        .to_owned();
    let applied = suite
        .get("applied")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let entries = suite
        .get("entries")
        .and_then(Value::as_array)
        .ok_or_else(|| WorkcellError::InvalidDemand("projection is missing `entries`".into()))?;

    let mut surfaced = Vec::new();
    let mut projected = !entries.is_empty();
    for entry in entries {
        let key = entry
            .get("key")
            .and_then(Value::as_str)
            .ok_or_else(|| WorkcellError::InvalidDemand("projection entry is missing `key`".into()))?;
        let action = entry
            .get("action")
            .and_then(Value::as_object)
            .and_then(|action| action.get("action"))
            .and_then(Value::as_str)
            .ok_or_else(|| {
                WorkcellError::InvalidDemand(format!(
                    "projection entry `{key}` is missing its action verdict"
                ))
            })?;
        match action {
            // AIKit's verdict labels: a checkout ending at the target.
            "already-projected" | "fast-forwarded" => {}
            // Drift or failure AIKit left for a human — carried as a surfaced key.
            "surfaced" | "failed" => {
                surfaced.push(key.to_owned());
                projected = false;
            }
            // Clean-behind in observe mode: not surfaced, but not yet projected.
            "would-fast-forward" => projected = false,
            other => {
                return Err(WorkcellError::Unsupported(format!(
                    "projection entry `{key}` carries an action `{other}` unknown to this Workcell"
                )))
            }
        }
    }

    let summary = match suite.get("summary") {
        Some(Value::Array(lines)) => lines
            .iter()
            .filter_map(Value::as_str)
            .collect::<Vec<_>>()
            .join("\n"),
        Some(Value::String(text)) => text.clone(),
        _ => String::new(),
    };

    Ok(ProjectionCorrelation {
        subject_key,
        source,
        target,
        applied,
        projected,
        surfaced,
        summary,
    })
}

/// Find the AIKit `SuiteProjection` object in the supplied JSON: the top-level
/// object when it carries the projection version tag, else one nested under a
/// reply `data` envelope.
fn locate_projection_object(value: &Value) -> Option<&Map<String, Value>> {
    fn is_suite(object: &Map<String, Value>) -> bool {
        object.get("version").and_then(Value::as_str) == Some(AIKIT_WORKTREE_PROJECTION_SOURCE)
            && object.contains_key("entries")
    }
    let object = value.as_object()?;
    if is_suite(object) {
        return Some(object);
    }
    if let Some(data) = object.get("data").and_then(Value::as_object) {
        if is_suite(data) {
            return Some(data);
        }
    }
    None
}

fn parse_demand(args: &[String]) -> Result<ExecutionDemand, WorkcellError> {
    if args.first().map(String::as_str) == Some("--demand-json") {
        if args.len() != 2 { return Err(WorkcellError::InvalidDemand("--demand-json requires exactly one file and cannot be mixed with requirement flags".into())); }
        let raw = fs::read(&args[1]).map_err(|e| WorkcellError::InvalidDemand(format!("read demand JSON: {e}")))?;
        if raw.len() > 1_048_576 { return Err(WorkcellError::InvalidDemand("demand JSON exceeds 1 MiB".into())); }
        let value = serde_json::from_slice(&raw).map_err(|e| WorkcellError::InvalidDemand(format!("parse demand JSON: {e}")))?;
        let demand = epilogos_workcell_control::codec::decode_demand(&value)?;
        demand.validate()?;
        return Ok(demand);
    }
    let mut demand_ref = DEFAULT_DEMAND_REF.to_owned();
    let mut affordances = Tiered::default();
    let mut connectivity = Tiered::default();
    let mut exposure = Tiered::default();
    let mut outputs = Tiered::default();
    let mut workspace_access = None;
    let mut workspace_ref = None;
    let mut workspace_revision = None;
    let mut project_runtime = None;
    let mut resources = Vec::new();
    let mut subjects = BTreeMap::new();
    let mut persistence = None;
    let mut isolation = None;
    let mut retention = RetentionExpectation::Release;
    let mut extensions = BTreeMap::new();
    let mut index = 0;

    while index < args.len() {
        let flag = args[index].as_str();
        let value = || require_value(args, index, flag);
        match flag {
            "--demand-ref" => demand_ref = value()?.to_owned(),
            "--require" => affordances
                .required
                .push(AffordanceRequirement::new(value()?)?),
            "--prefer" => affordances
                .preferred
                .push(AffordanceRequirement::new(value()?)?),
            "--optional" => affordances
                .optional
                .push(AffordanceRequirement::new(value()?)?),
            "--connect" => connectivity
                .required
                .push(LogicalConnectionRequirement::new(value()?)?),
            "--prefer-connect" => connectivity
                .preferred
                .push(LogicalConnectionRequirement::new(value()?)?),
            "--optional-connect" => connectivity
                .optional
                .push(LogicalConnectionRequirement::new(value()?)?),
            "--expose" => exposure.required.push(ExposureRequirement::new(value()?)?),
            "--prefer-expose" => exposure.preferred.push(ExposureRequirement::new(value()?)?),
            "--optional-expose" => exposure.optional.push(ExposureRequirement::new(value()?)?),
            "--output" => outputs.required.push(OutputRequirement::new(value()?)?),
            "--prefer-output" => outputs.preferred.push(OutputRequirement::new(value()?)?),
            "--optional-output" => outputs.optional.push(OutputRequirement::new(value()?)?),
            "--workspace" => workspace_access = Some(parse_workspace_access(value()?)?),
            "--workspace-ref" => {
                workspace_ref = Some(ExternalRef::new(value()?).map_err(WorkcellError::from)?)
            }
            "--revision" => workspace_revision = Some(value()?.to_owned()),
            "--project-runtime" => {
                project_runtime = Some(ProjectRuntimeRequirement::new(value()?)?)
            }
            "--resource" => resources.push(parse_resource(value()?)?),
            "--subject" => {
                let (role, reference) = parse_pair(value()?, "subject")?;
                subjects.insert(
                    role.to_owned(),
                    ExternalRef::new(reference).map_err(WorkcellError::from)?,
                );
            }
            "--persistence" => persistence = Some(parse_persistence(value()?)?),
            "--isolation" => isolation = Some(IsolationTrustRequirement::new(value()?)?),
            "--retention" => retention = parse_retention(value()?)?,
            "--extension" => {
                let (key, extension_value) = parse_pair(value()?, "extension")?;
                extensions.insert(key.to_owned(), extension_value.to_owned());
            }
            unknown => {
                return Err(WorkcellError::InvalidDemand(format!(
                    "unknown demand option `{unknown}`"
                )))
            }
        }
        index += 2;
    }

    let mut demand = ExecutionDemand::new(DemandRef::new(demand_ref).map_err(WorkcellError::from)?);
    demand.affordances = affordances;
    demand.connectivity = connectivity;
    demand.exposure = exposure;
    demand.outputs = outputs;
    demand.project_runtime = project_runtime;
    demand.resources = resources;
    demand.subjects = subjects;
    demand.persistence = persistence;
    demand.isolation_trust = isolation;
    demand.retention = retention;
    demand.extensions = extensions;

    if workspace_access.is_some() || workspace_ref.is_some() || workspace_revision.is_some() {
        demand.workspace = Some(WorkspaceRequirement {
            source: workspace_ref,
            revision: workspace_revision,
            access: workspace_access.unwrap_or(WorkspaceAccess::Writable),
        });
    }
    demand.validate()?;
    Ok(demand)
}

fn parse_desired(args: &[String]) -> Result<Vec<DesiredMaterialState>, WorkcellError> {
    let mut desired = Vec::new();
    let mut index = 0;
    while index < args.len() {
        if args[index] != "--desired" {
            return Err(WorkcellError::InvalidDemand(format!(
                "unknown reconcile option `{}`",
                args[index]
            )));
        }
        let value = require_value(args, index, "--desired")?;
        let (logical_ref, state) = parse_pair(value, "desired state")?;
        desired.push(DesiredMaterialState {
            logical_ref: logical_ref.to_owned(),
            desired: state.to_owned(),
        });
        index += 2;
    }
    if desired.is_empty() {
        return Err(WorkcellError::InvalidDemand(
            "reconcile requires at least one `--desired logical-ref=state`".into(),
        ));
    }
    Ok(desired)
}

fn resume(
    global: &GlobalArgs,
) -> Result<
    (
        CollapsedLocalWorkcell,
        epilogos_workcell_core::MaterialisedExecutionWorld,
        PathBuf,
    ),
    WorkcellError,
> {
    let receipt = global.receipt.clone().ok_or_else(|| {
        WorkcellError::InvalidDemand(
            "this command requires `--receipt <material-world.json>`".into(),
        )
    })?;
    let encoded = fs::read_to_string(&receipt).map_err(|error| {
        WorkcellError::NotFound(format!(
            "read material-world receipt `{}`: {error}",
            receipt.display()
        ))
    })?;
    let world = decode_world(&encoded)?;
    let channels = world_artifact_channels(&world);
    let mut config = with_declared_services(
        CollapsedLocalConfig::new(world.workcell_ref.clone(), &global.state_root),
        global,
    );
    config.artifact_channels = channels.into_iter().collect();
    let mut workcell = CollapsedLocalWorkcell::new(config)?;
    workcell.register_world(world.clone())?;
    Ok((workcell, world, receipt))
}

fn new_local(
    global: &GlobalArgs,
    workcell_ref: WorkcellRef,
    additional_channels: BTreeSet<String>,
) -> Result<CollapsedLocalWorkcell, WorkcellError> {
    let mut config = with_declared_services(
        CollapsedLocalConfig::new(workcell_ref, &global.state_root),
        global,
    );
    if let Some(source) = &global.workspace_source {
        config = config.with_workspace_source(source);
    }
    let mut channels = BTreeSet::from(["logs:run".to_owned(), "artifacts:run".to_owned()]);
    channels.extend(additional_channels);
    config.artifact_channels = channels.into_iter().collect();
    CollapsedLocalWorkcell::new(config)
}

/// `--services PATH` names the declaration file; without it the conventional
/// `<state-root>/services.json` is read when it is there.
fn with_declared_services(
    config: CollapsedLocalConfig,
    global: &GlobalArgs,
) -> CollapsedLocalConfig {
    match &global.services {
        Some(path) => config.with_service_declaration_file(path),
        None => config,
    }
}

fn demand_output_channels(demand: &ExecutionDemand) -> BTreeSet<String> {
    demand
        .outputs
        .required
        .iter()
        .chain(&demand.outputs.preferred)
        .chain(&demand.outputs.optional)
        .map(|value| value.as_str().to_owned())
        .collect()
}

fn world_artifact_channels(
    world: &epilogos_workcell_core::MaterialisedExecutionWorld,
) -> BTreeSet<String> {
    let mut channels = BTreeSet::from(["logs:run".to_owned(), "artifacts:run".to_owned()]);
    for binding in &world.binding_graph.bindings {
        if binding.port == ProviderPortKind::ArtifactStorage {
            if let Some(channel) = binding.properties.get("logical_channel") {
                channels.insert(channel.clone());
            }
        }
    }
    channels
}

fn parse_resource(value: &str) -> Result<ResourceRequirement, WorkcellError> {
    let Some((key, specification)) = value.split_once('=') else {
        if value.trim().is_empty() {
            return Err(WorkcellError::InvalidDemand(
                "resource key must not be empty".into(),
            ));
        }
        return Ok(ResourceRequirement {
            key: value.to_owned(),
            minimum: None,
            unit: None,
        });
    };
    if key.trim().is_empty() || specification.trim().is_empty() {
        return Err(WorkcellError::InvalidDemand(
            "resource must use `key=amount[:unit]`".into(),
        ));
    }
    let (amount, unit) = specification
        .split_once(':')
        .map_or((specification, None), |(amount, unit)| (amount, Some(unit)));
    let minimum = amount.parse::<u64>().map_err(|error| {
        WorkcellError::InvalidDemand(format!("invalid resource amount `{amount}`: {error}"))
    })?;
    Ok(ResourceRequirement {
        key: key.to_owned(),
        minimum: Some(minimum),
        unit: unit.map(str::to_owned),
    })
}

fn parse_workspace_access(value: &str) -> Result<WorkspaceAccess, WorkcellError> {
    match value {
        "read-only" | "readonly" => Ok(WorkspaceAccess::ReadOnly),
        "writable" | "write" => Ok(WorkspaceAccess::Writable),
        other => Err(WorkcellError::InvalidDemand(format!(
            "workspace access must be `read-only` or `writable`, got `{other}`"
        ))),
    }
}

fn parse_persistence(value: &str) -> Result<PersistenceScope, WorkcellError> {
    match value {
        "ephemeral" => Ok(PersistenceScope::Ephemeral),
        "task-or-run" => Ok(PersistenceScope::TaskOrRun),
        "candidate" => Ok(PersistenceScope::Candidate),
        "project" => Ok(PersistenceScope::Project),
        "workcell" => Ok(PersistenceScope::Workcell),
        "factory" => Ok(PersistenceScope::Factory),
        "external" => Ok(PersistenceScope::External),
        other => Err(WorkcellError::InvalidDemand(format!(
            "unknown persistence scope `{other}`"
        ))),
    }
}

fn parse_retention(value: &str) -> Result<RetentionExpectation, WorkcellError> {
    match value {
        "release" => Ok(RetentionExpectation::Release),
        "preserve" => Ok(RetentionExpectation::Preserve),
        "suspend-if-supported" => Ok(RetentionExpectation::SuspendIfSupported),
        "snapshot-if-supported" => Ok(RetentionExpectation::SnapshotIfSupported),
        other => Err(WorkcellError::InvalidDemand(format!(
            "unknown retention expectation `{other}`"
        ))),
    }
}

fn parse_pair<'a>(value: &'a str, label: &str) -> Result<(&'a str, &'a str), WorkcellError> {
    let (key, pair_value) = value
        .split_once('=')
        .ok_or_else(|| WorkcellError::InvalidDemand(format!("{label} must use `key=value`")))?;
    if key.trim().is_empty() || pair_value.trim().is_empty() {
        return Err(WorkcellError::InvalidDemand(format!(
            "{label} key and value must not be empty"
        )));
    }
    Ok((key, pair_value))
}

fn parse_workcell_ref(value: &str) -> Result<WorkcellRef, WorkcellError> {
    WorkcellRef::new(value).map_err(WorkcellError::from)
}

fn require_value<'a>(
    args: &'a [String],
    index: usize,
    flag: &str,
) -> Result<&'a str, WorkcellError> {
    args.get(index + 1)
        .map(String::as_str)
        .ok_or_else(|| WorkcellError::InvalidDemand(format!("`{flag}` requires a value")))
}

fn default_state_root() -> PathBuf {
    env::var_os("WORKCELL_HOME")
        .map(PathBuf::from)
        .or_else(|| env::var_os("HOME").map(|home| PathBuf::from(home).join(".workcell")))
        .unwrap_or_else(|| PathBuf::from(".workcell"))
}

fn default_receipt_path(state_root: &Path, world_ref: &str) -> PathBuf {
    state_root
        .join("worlds")
        .join(format!("{}.json", safe_filename(world_ref)))
}

fn safe_filename(value: &str) -> String {
    value
        .chars()
        .map(|character| {
            if character.is_ascii_alphanumeric() || matches!(character, '-' | '_' | '.') {
                character
            } else {
                '_'
            }
        })
        .collect()
}

fn command_instances(global: &GlobalArgs, args: &[String]) -> Result<(), WorkcellError> {
    use epilogos_workcell_runtime::{
        build_instance_record, seam, InstanceRegistry, RegisterOutcome,
        EVIDENCE_DECLARED_UNVERIFIED, EVIDENCE_LIVE_PID,
    };

    let workcell_ref = parse_workcell_ref(&global.workcell_ref)?;
    let registry = InstanceRegistry::new(&global.state_root, workcell_ref.clone());
    let Some(subcommand) = args.first().map(String::as_str) else {
        return Err(WorkcellError::InvalidDemand(
            "usage: workcell instances <list|show|register|declare|scan|usage|candidates|project> [args]"
                .into(),
        ));
    };

    match subcommand {
        "usage" => {
            let Some(reference) = args.get(1) else {
                return Err(WorkcellError::InvalidDemand(
                    "usage: workcell instances usage <instance_ref> [--pid <n>] \
                     [--interval-ms <0..60000>] [--correlation-ref <opaque-ref>]..."
                        .into(),
                ));
            };
            let mut pid = None;
            let mut interval = epilogos_workcell_runtime::DEFAULT_INTERVAL;
            let mut correlation_refs = Vec::new();
            let mut index = 2;
            while index < args.len() {
                match args[index].as_str() {
                    "--pid" => {
                        index += 1;
                        pid = Some(
                            args.get(index)
                                .and_then(|value| value.parse::<u32>().ok())
                                .ok_or_else(|| {
                                    WorkcellError::InvalidDemand(
                                        "--pid requires a numeric process id".into(),
                                    )
                                })?,
                        );
                    }
                    "--interval-ms" => {
                        index += 1;
                        let millis = args
                            .get(index)
                            .and_then(|value| value.parse::<u64>().ok())
                            .ok_or_else(|| {
                                WorkcellError::InvalidDemand(
                                    "--interval-ms requires whole milliseconds".into(),
                                )
                            })?;
                        interval = std::time::Duration::from_millis(millis);
                    }
                    "--correlation-ref" => {
                        index += 1;
                        correlation_refs.push(
                            args.get(index)
                                .filter(|value| !value.is_empty())
                                .cloned()
                                .ok_or_else(|| {
                                    WorkcellError::InvalidDemand(
                                        "--correlation-ref requires an opaque reference".into(),
                                    )
                                })?,
                        );
                    }
                    other => {
                        return Err(WorkcellError::InvalidDemand(format!(
                            "unknown instances usage flag `{other}`"
                        )))
                    }
                }
                index += 1;
            }
            let report = epilogos_workcell_runtime::observe_resource_usage(
                &registry,
                reference,
                pid,
                interval,
                correlation_refs,
            )?;
            let value = report.as_json();
            if global.json {
                emit_json(value);
            } else {
                println!(
                    "usage: {} pid {} over {} ms",
                    value["harness_instance_ref"].as_str().unwrap_or("?"),
                    value["material_binding"]["pid"].as_u64().unwrap_or(0),
                    value["interval"]["duration_ms"].as_u64().unwrap_or(0),
                );
                for name in ["cpu_time", "cpu_utilisation", "memory_rss"] {
                    let metric = &value["metrics"][name];
                    match metric["standing"].as_str().unwrap_or("unavailable") {
                        "observed" | "derived" => println!(
                            "  {name}: {} {} ({})",
                            metric["value"],
                            metric["unit"].as_str().unwrap_or(""),
                            metric["standing"].as_str().unwrap_or("?"),
                        ),
                        standing => println!("  {name}: {standing}"),
                    }
                }
            }
            Ok(())
        }
        "candidates" => {
            let records = registry.list()?;
            let candidates = epilogos_workcell_runtime::projection_candidates(&records);
            if global.json {
                emit_json(json!({
                    "ok": true,
                    "workcell_ref": workcell_ref.as_str(),
                    "projection_candidate": !candidates.is_empty(),
                    "candidates": candidates,
                }));
            } else {
                println!(
                    "projection candidate: {} ({} detected instance(s) with evidence_grade live-pid or stronger)",
                    if candidates.is_empty() { "no" } else { "yes" },
                    candidates.len(),
                );
                for record in candidates {
                    println!(
                        "  {} [{}] {}",
                        record["instance_ref"].as_str().unwrap_or("?"),
                        record["evidence_grade"].as_str().unwrap_or("?"),
                        record["harness_ref"].as_str().unwrap_or("?"),
                    );
                }
            }
            Ok(())
        }
        "project" => {
            let Some(reference) = args.get(1) else {
                return Err(WorkcellError::InvalidDemand(
                    "usage: workcell instances project <instance_ref> --to-workcell <workcell:ref> \
                     [--to-state-root <path>]"
                        .into(),
                ));
            };
            let mut to_workcell = None;
            let mut to_state_root = global.state_root.clone();
            let mut index = 2;
            while index < args.len() {
                match args[index].as_str() {
                    "--to-workcell" => {
                        index += 1;
                        to_workcell = args.get(index).cloned();
                    }
                    "--to-state-root" => {
                        index += 1;
                        let Some(path) = args.get(index) else {
                            return Err(WorkcellError::InvalidDemand(
                                "--to-state-root requires a path".into(),
                            ));
                        };
                        to_state_root = PathBuf::from(path);
                    }
                    other => {
                        return Err(WorkcellError::InvalidDemand(format!(
                            "unknown instances project flag `{other}`"
                        )))
                    }
                }
                index += 1;
            }
            let Some(to_workcell) = to_workcell else {
                return Err(WorkcellError::InvalidDemand(
                    "usage: workcell instances project <instance_ref> --to-workcell <workcell:ref> \
                     [--to-state-root <path>]"
                        .into(),
                ));
            };
            let target_ref = parse_workcell_ref(&to_workcell)?;
            let report = epilogos_workcell_runtime::project_instance_live(
                &registry,
                reference,
                &to_state_root,
                &target_ref,
                "actuation",
            )?;
            let status = report.status;
            let reason = report
                .reason
                .clone()
                .unwrap_or_else(|| "no reason given".to_owned());
            if global.json {
                emit_json(epilogos_workcell_runtime::projection_report_json(&report));
            } else {
                match status {
                    "ok" => {
                        let redetected = report
                            .redetected
                            .expect("ok carries the re-detected record");
                        println!(
                            "projected: {} re-detected on {} (identity held: {})",
                            redetected["harness_ref"].as_str().unwrap_or("?"),
                            report.target_workcell_ref,
                            report.expected_instance_ref,
                        );
                    }
                    other => println!("project: {} ({})", other, reason),
                }
            }
            if status == "ok" {
                Ok(())
            } else {
                Err(WorkcellError::OperationFailed(format!(
                    "projection {status}: {reason}"
                )))
            }
        }
        "scan" => {
            let report = epilogos_workcell_runtime::scan_live(
                &global.state_root,
                &workcell_ref,
                "actuation",
            );
            if global.json {
                emit_json(epilogos_workcell_runtime::report_json(&report));
            } else {
                match report.status {
                    "ok" => {
                        println!(
                            "scan: {} live, {} stale ({} registered, {} refreshed, {} revived, {} adopted, {} went stale)",
                            report.live.len(),
                            report.stale.len(),
                            report.transitions.registered,
                            report.transitions.refreshed,
                            report.transitions.revived,
                            report.transitions.adopted,
                            report.transitions.went_stale,
                        );
                        for record in report.live.iter().chain(report.stale.iter()) {
                            println!(
                                "  {} [{}] {} (pid {})",
                                record["instance_ref"].as_str().unwrap_or("?"),
                                record["liveness"].as_str().unwrap_or("?"),
                                record["harness_ref"].as_str().unwrap_or("?"),
                                record["pids"].as_array().map_or("-".to_string(), |pids| {
                                    pids.iter().filter_map(Value::as_u64).map(|pid| pid.to_string()).collect::<Vec<_>>().join(",")
                                }),
                            );
                        }
                        for conflict in &report.conflicts {
                            println!(
                                "  ! conflict: {} — {} vs {} ({})",
                                conflict.slug,
                                conflict.existing_ref,
                                conflict.incoming_ref,
                                conflict.reason,
                            );
                        }
                        if !report.unmatched_processes.is_empty() {
                            println!("  unmatched processes: {}", report.unmatched_processes.join(", "));
                        }
                    }
                    other => println!(
                        "scan: unavailable ({})",
                        report.reason.unwrap_or_else(|| other.to_owned())
                    ),
                }
            }
            Ok(())
        }
        "list" => {
            let records = epilogos_workcell_runtime::sort_by_reference(registry.list()?);
            if global.json {
                emit_json(json!({
                    "ok": true,
                    "workcell_ref": workcell_ref.as_str(),
                    "instances": records,
                }));
            } else {
                if records.is_empty() {
                    println!("no harness instances registered in {}", workcell_ref);
                }
                for record in records {
                    println!(
                        "{} [{}] {} (pid {})",
                        record["instance_ref"].as_str().unwrap_or("?"),
                        record["evidence_grade"].as_str().unwrap_or("?"),
                        record["harness_ref"].as_str().unwrap_or("?"),
                        record["pids"].as_array().map_or("-".to_string(), |pids| {
                            pids.iter().filter_map(Value::as_u64).map(|pid| pid.to_string()).collect::<Vec<_>>().join(",")
                        }),
                    );
                }
            }
            Ok(())
        }
        "show" => {
            let Some(reference) = args.get(1) else {
                return Err(WorkcellError::InvalidDemand(
                    "usage: workcell instances show <instance_ref>".into(),
                ));
            };
            let record = registry.show(reference)?;
            if global.json {
                emit_json(json!({ "ok": true, "instance": record }));
            } else {
                println!("{}", serde_json::to_string_pretty(&record).expect("record is json"));
            }
            Ok(())
        }
        "register" => {
            let mut harness = None;
            let mut executable = None;
            let mut sha256 = None;
            let mut pids: Vec<u32> = Vec::new();
            let mut declared = false;
            let mut seams: Vec<Value> = Vec::new();
            let mut index = 1;
            while index < args.len() {
                match args[index].as_str() {
                    "--harness" => {
                        index += 1;
                        harness = args.get(index).cloned();
                    }
                    "--executable" => {
                        index += 1;
                        executable = args.get(index).map(PathBuf::from);
                    }
                    "--sha256" => {
                        index += 1;
                        sha256 = args.get(index).cloned();
                    }
                    "--pid" => {
                        index += 1;
                        let Some(parsed) = args.get(index).and_then(|value| value.parse::<u32>().ok())
                        else {
                            return Err(WorkcellError::InvalidDemand(
                                "--pid requires a numeric process id".into(),
                            ));
                        };
                        pids.push(parsed);
                    }
                    "--declared" => declared = true,
                    "--seam" => {
                        index += 1;
                        let Some(spec) = args.get(index) else {
                            return Err(WorkcellError::InvalidDemand(
                                "--seam requires kind:path:exists".into(),
                            ));
                        };
                        let mut parts = spec.splitn(3, ':');
                        let kind = parts.next().unwrap_or_default();
                        let path = parts.next().unwrap_or_default();
                        let exists = parts.next() == Some("true");
                        if kind.is_empty() || path.is_empty() {
                            return Err(WorkcellError::InvalidDemand(
                                "--seam requires kind:path:exists".into(),
                            ));
                        }
                        seams.push(seam(kind, path, exists, None));
                    }
                    other => {
                        return Err(WorkcellError::InvalidDemand(format!(
                            "unknown instances register flag `{other}`"
                        )))
                    }
                }
                index += 1;
            }
            let (Some(harness), Some(executable), Some(sha256)) = (harness, executable, sha256)
            else {
                return Err(WorkcellError::InvalidDemand(
                    "usage: workcell instances register --harness <slug> --executable <path> \
                     --sha256 <receipt> [--pid <n> | --declared] [--seam kind:path:exists]..."
                        .into(),
                ));
            };
            if declared && !pids.is_empty() {
                return Err(WorkcellError::InvalidDemand(
                    "--declared and --pid are mutually exclusive".into(),
                ));
            }
            let evidence_grade = if declared {
                EVIDENCE_DECLARED_UNVERIFIED
            } else {
                if pids.is_empty() {
                    return Err(WorkcellError::InvalidDemand(
                        "a live registration requires --pid (or pass --declared)".into(),
                    ));
                }
                EVIDENCE_LIVE_PID
            };
            let record = build_instance_record(
                &workcell_ref,
                &epilogos_workcell_runtime::InstanceObservation {
                    slug: harness.clone(),
                    executable: executable.clone(),
                    executable_sha256: sha256.clone(),
                    identity_material: executable.to_string_lossy().into_owned(),
                    pids,
                    // A manual registration declares live pids without a
                    // sampled start marker; `instances scan` records the
                    // host's start evidence instead.
                    executions: Vec::new(),
                    evidence_grade: evidence_grade.to_owned(),
                    seams,
                },
            );
            match registry.register(record)? {
                RegisterOutcome::Registered => {
                    if global.json {
                        emit_json(json!({ "ok": true, "outcome": "registered" }));
                    } else {
                        println!("registered");
                    }
                }
                RegisterOutcome::Unchanged => {
                    if global.json {
                        emit_json(json!({ "ok": true, "outcome": "unchanged" }));
                    } else {
                        println!("unchanged");
                    }
                }
                RegisterOutcome::Conflict { existing, incoming } => {
                    emit_json(json!({
                        "ok": false,
                        "error": {
                            "kind": "instance_conflict",
                            "message": format!(
                                "harness `{}` already holds executable {} (incoming {})",
                                harness,
                                existing["executable"]["sha256"].as_str().unwrap_or("?"),
                                incoming["executable"]["sha256"].as_str().unwrap_or("?"),
                            ),
                        }
                    }));
                    return Err(WorkcellError::InvalidDemand(format!(
                        "instance conflict for harness `{harness}`; see --json output"
                    )));
                }
            }
            Ok(())
        }
        "declare" => {
            let mut harness = None;
            let mut executable: Option<PathBuf> = None;
            let mut identity_material = None;
            let mut index = 1;
            while index < args.len() {
                match args[index].as_str() {
                    "--harness" => {
                        index += 1;
                        harness = args.get(index).cloned();
                    }
                    "--executable" => {
                        index += 1;
                        executable = args.get(index).map(PathBuf::from);
                    }
                    "--identity-material" => {
                        index += 1;
                        identity_material = args.get(index).cloned();
                    }
                    other => {
                        return Err(WorkcellError::InvalidDemand(format!(
                            "unknown instances declare flag `{other}`"
                        )))
                    }
                }
                index += 1;
            }
            let Some(harness) = harness else {
                return Err(WorkcellError::InvalidDemand(
                    "usage: workcell instances declare --harness <slug> \
                     [--executable <path>] [--identity-material <text>]"
                        .into(),
                ));
            };
            let identity_material = identity_material.unwrap_or_else(|| {
                executable
                    .as_ref()
                    .map(|path| path.to_string_lossy().into_owned())
                    .unwrap_or_else(|| format!("declared:{harness}"))
            });
            match registry.declare(&harness, executable.as_deref(), &identity_material)? {
                RegisterOutcome::Registered => {
                    if global.json {
                        emit_json(json!({ "ok": true, "outcome": "declared" }));
                    } else {
                        println!("declared: {harness} (declared-unverified; binds on next scan)");
                    }
                }
                RegisterOutcome::Unchanged => {
                    if global.json {
                        emit_json(json!({ "ok": true, "outcome": "unchanged" }));
                    } else {
                        println!("unchanged");
                    }
                }
                RegisterOutcome::Conflict { existing, incoming } => {
                    emit_json(json!({
                        "ok": false,
                        "error": {
                            "kind": "instance_conflict",
                            "message": format!(
                                "harness `{}` already holds identity {} (incoming {}); resolve the authored declaration",
                                harness,
                                existing["instance_ref"].as_str().unwrap_or("?"),
                                incoming["instance_ref"].as_str().unwrap_or("?"),
                            ),
                        }
                    }));
                    return Err(WorkcellError::InvalidDemand(format!(
                        "declaration conflict for harness `{harness}`; see --json output"
                    )));
                }
            }
            Ok(())
        }
        other => Err(WorkcellError::InvalidDemand(format!(
            "unknown instances subcommand `{other}`; expected list|show|register|declare|scan|usage|candidates|project"
        ))),
    }
}

/// `workcell places` — the read-only place census. It is a machine-first
/// census document, so it emits the JSON shape unconditionally (`--json`
/// changes nothing); there is no useful text rendering of a census.
fn command_places(_global: &GlobalArgs) -> Result<(), WorkcellError> {
    let census = epilogos_workcell_runtime::scan_places_live();
    emit_json(epilogos_workcell_runtime::census_json(&census));
    Ok(())
}

/// `workcell place <request|release>` — claim a persistent process place and
/// give it back. Machine-first like the census: both subcommands emit their
/// JSON contract document; refusals additionally carry typed evidence.
fn command_place(global: &GlobalArgs, args: &[String]) -> Result<(), WorkcellError> {
    let Some(subcommand) = args.first().map(String::as_str) else {
        return Err(WorkcellError::InvalidDemand(
            "usage: workcell place <request|release> [args]".into(),
        ));
    };
    match subcommand {
        "request" => command_place_request(global, &args[1..]),
        "release" => command_place_release(global, &args[1..]),
        other => Err(WorkcellError::InvalidDemand(format!(
            "unknown place subcommand `{other}`; expected request|release"
        ))),
    }
}

fn command_place_request(_global: &GlobalArgs, args: &[String]) -> Result<(), WorkcellError> {
    let mut provider = "auto";
    let mut name: Option<&str> = None;
    let mut index = 0;
    while index < args.len() {
        match args[index].as_str() {
            "--provider" => {
                index += 1;
                provider = args.get(index).map(String::as_str).ok_or_else(|| {
                    WorkcellError::InvalidDemand("--provider requires auto, herdr or tmux".into())
                })?;
            }
            "--name" => {
                index += 1;
                name = args.get(index).map(String::as_str);
            }
            other => {
                return Err(WorkcellError::InvalidDemand(format!(
                    "unknown place request flag `{other}`"
                )))
            }
        }
        index += 1;
    }
    let Some(name) = name else {
        return Err(WorkcellError::InvalidDemand(
            "usage: workcell place request --provider auto|herdr|tmux --name <slug>".into(),
        ));
    };
    let policy = epilogos_workcell_runtime::PlacePolicy::parse(provider)?;

    match epilogos_workcell_runtime::request_place_live(policy, name) {
        Ok(grant) => {
            // Success stdout is exactly the published grant document — the
            // contract artifact itself, flat and self-describing — so a
            // consumer can pin and parse `workcell.place-grant/v1` without
            // unwrapping a command envelope. Refusals keep the typed refusal
            // document on stdout with a non-zero exit.
            emit_json(grant.to_json());
            Ok(())
        }
        Err(refusal) => {
            // Mirror the instances conflict pattern: the typed refusal
            // document goes to stdout, the error to the exit path.
            emit_json(refusal.to_json());
            Err(refusal.to_error())
        }
    }
}

fn command_place_release(_global: &GlobalArgs, args: &[String]) -> Result<(), WorkcellError> {
    let mut place_ref: Option<&str> = None;
    let mut pid: Option<u32> = None;
    let mut start_marker: Option<&str> = None;
    let mut provider_close = false;
    let mut index = 0;
    while index < args.len() {
        match args[index].as_str() {
            "--place-ref" => {
                index += 1;
                place_ref = args.get(index).map(String::as_str);
            }
            "--pid" => {
                index += 1;
                pid = Some(
                    args.get(index)
                        .and_then(|value| value.parse::<u32>().ok())
                        .ok_or_else(|| {
                            WorkcellError::InvalidDemand("--pid requires a numeric process id".into())
                        })?,
                );
            }
            "--start-marker" => {
                index += 1;
                start_marker = args.get(index).map(String::as_str);
            }
            // Escape hatch for a place whose generation proof has failed
            // (e.g. a herdr room whose pane processes churned): close the
            // provider-native room itself, never a live pid. Only valid on a
            // failed proof — with a live proof the release refuses the flag
            // and the caller releases normally.
            "--provider-close" => provider_close = true,
            other => {
                return Err(WorkcellError::InvalidDemand(format!(
                    "unknown place release flag `{other}`"
                )))
            }
        }
        index += 1;
    }
    let (Some(place_ref), Some(pid), Some(start_marker)) = (place_ref, pid, start_marker) else {
        return Err(WorkcellError::InvalidDemand(
            "usage: workcell place release --place-ref <ref> --pid <n> --start-marker \"<ps lstart>\" [--provider-close]"
                .into(),
        ));
    };
    if start_marker.is_empty() {
        return Err(WorkcellError::InvalidDemand(
            "--start-marker requires the ps lstart value recorded in the place grant".into(),
        ));
    }
    let demand = epilogos_workcell_runtime::PlaceReleaseDemand {
        place_ref: place_ref.to_owned(),
        pid,
        start_marker: start_marker.to_owned(),
    };

    match epilogos_workcell_runtime::release_place_live(&demand, provider_close) {
        Ok(result) => {
            emit_json(result);
            Ok(())
        }
        Err(refusal) => {
            emit_json(refusal.to_json());
            Err(refusal.to_error())
        }
    }
}

fn command_sandboxes(global: &GlobalArgs, args: &[String]) -> Result<(), WorkcellError> {
    use epilogos_workcell_opensandbox::{
        SandboxLease, SandboxServerReconciler, StdHttpOpenSandboxTransport,
        OPENSANDBOX_DEFAULT_API_KEY_ENV,
    };

    let Some(subcommand) = args.first().map(String::as_str) else {
        return Err(WorkcellError::InvalidDemand(
            "usage: workcell sandboxes <reconcile> [args]".into(),
        ));
    };
    if subcommand != "reconcile" {
        return Err(WorkcellError::InvalidDemand(format!(
            "unknown sandboxes subcommand `{subcommand}`; expected reconcile"
        )));
    }

    let mut server = None;
    let mut api_key_env = OPENSANDBOX_DEFAULT_API_KEY_ENV.to_owned();
    let mut release_orphans = false;
    let mut include_snapshots = false;
    let mut operator_releases: Vec<String> = Vec::new();
    let mut index = 1;
    while index < args.len() {
        match args[index].as_str() {
            "--server" => {
                server = Some(require_value(args, index, "--server")?.to_owned());
                index += 2;
            }
            "--api-key-env" => {
                api_key_env = require_value(args, index, "--api-key-env")?.to_owned();
                index += 2;
            }
            "--release-orphans" => {
                release_orphans = true;
                index += 1;
            }
            "--release" => {
                operator_releases.push(require_value(args, index, "--release")?.to_owned());
                index += 2;
            }
            "--include-snapshots" => {
                include_snapshots = true;
                index += 1;
            }
            other => {
                return Err(WorkcellError::InvalidDemand(format!(
                    "unknown sandboxes reconcile flag `{other}`"
                )));
            }
        }
    }
    let Some(server) = server else {
        return Err(WorkcellError::InvalidDemand(
            "usage: workcell sandboxes reconcile --server <url> [--api-key-env <ENV>] [--release-orphans] [--release <id>...] [--include-snapshots]".into(),
        ));
    };
    // The credential enters through the named environment variable when set
    // and non-empty; otherwise no key is sent (servers without auth accept
    // that). A raw key literal is not accepted as argv: credentials belong to
    // their origin store or the environment, never to the command line.
    let api_key = std::env::var(&api_key_env)
        .ok()
        .filter(|value| !value.is_empty());

    let reconciler =
        SandboxServerReconciler::new(server.clone(), api_key, StdHttpOpenSandboxTransport);
    let mut report = reconciler.reconcile(release_orphans, include_snapshots)?;
    if !operator_releases.is_empty() {
        let (released, mut failures) = reconciler.release_asserted(&operator_releases);
        report.released_sandboxes.extend(released);
        report.failures.append(&mut failures);
    }

    if global.json {
        let sandboxes: Vec<Value> = report
            .sandboxes
            .iter()
            .map(|sandbox| {
                json!({
                    "id": sandbox.id,
                    "state": sandbox.state,
                    "expires_at": sandbox.expires_at,
                    "lease": sandbox.lease.as_str(),
                })
            })
            .collect();
        let snapshots: Vec<Value> = report
            .snapshots
            .iter()
            .map(|snapshot| {
                json!({
                    "id": snapshot.id,
                    "state": snapshot.state,
                    "created_at": snapshot.created_at,
                })
            })
            .collect();
        let failures: Vec<Value> = report
            .failures
            .iter()
            .map(|failure| {
                json!({
                    "operation": failure.operation,
                    "target": failure.target,
                    "reason": failure.reason,
                })
            })
            .collect();
        emit_json(json!({
            "ok": true,
            "server": server,
            "release_orphans": release_orphans,
            "include_snapshots": include_snapshots,
            "sandboxes": sandboxes,
            "snapshots": snapshots,
            "unrecognised_sandboxes": report.unrecognised_sandboxes,
            "unrecognised_snapshots": report.unrecognised_snapshots,
            "released_sandboxes": report.released_sandboxes,
            "deleted_snapshots": report.deleted_snapshots,
            "failures": failures,
        }));
    } else {
        let (live, unknown) = report
            .sandboxes
            .iter()
            .filter(|sandbox| sandbox.lease != SandboxLease::Expired)
            .fold((0usize, 0usize), |(live, unknown), sandbox| {
                if sandbox.lease == SandboxLease::Live {
                    (live + 1, unknown)
                } else {
                    (live, unknown + 1)
                }
            });
        println!(
            "reconcile: {} sandboxes ({} expired-orphans, {} live, {} unknown){}",
            report.sandboxes.len(),
            report.orphan_sandboxes().len(),
            live,
            unknown,
            if include_snapshots {
                format!(", {} snapshots", report.snapshots.len())
            } else {
                String::new()
            },
        );
        for sandbox in &report.sandboxes {
            println!(
                "  sandbox {} [{}] lease={} ({})",
                sandbox.id,
                sandbox.state,
                sandbox.lease.as_str(),
                sandbox.expires_at.as_deref().unwrap_or("no lease evidence"),
            );
        }
        for snapshot in &report.snapshots {
            println!(
                "  snapshot {} [{}] created={}",
                snapshot.id,
                snapshot.state,
                snapshot.created_at.as_deref().unwrap_or("-"),
            );
        }
        for id in &report.released_sandboxes {
            let asserted = operator_releases.contains(id);
            println!(
                "  released sandbox{}: {id}",
                if asserted { " (operator-asserted)" } else { " orphan" },
            );
        }
        for id in &report.deleted_snapshots {
            println!("  deleted snapshot: {id}");
        }
        for failure in &report.failures {
            println!(
                "  ! failure: {} {}: {}",
                failure.operation, failure.target, failure.reason
            );
        }
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Cross-cell connection lifecycle
//
// A connection is a durable, revocable, expirable, permissioned relation
// between two cells. `serve` exposes this cell behind the connection grants
// registry; `authorise`/`revoke` manage grants (receipted, auditable,
// revocation and expiry take effect at the next use); `connect` establishes
// the client side with an explicit compatibility handshake. Capability
// advertisement is not authorisation: discovery discloses, grants permit,
// and the two are reported separately everywhere.
// ---------------------------------------------------------------------------

// ---------------------------------------------------------------------------
// Runs — one agent run across every rung.
//
// `workcell run start|observe|collect|release|list|show|scope` carry one
// `workcell.run/v1` record shape with one execution-status vocabulary
// (Factory's, reused verbatim) and one deliverable contract. Workcell owns
// the material record; Factory's canonical RunRef rides as an optional ref;
// ai-kit owns the resident that lives inside the run. A dirty release
// refusal surfaces as run `blocked` — never deletion.
// ---------------------------------------------------------------------------

const RUN_USAGE: &str = "usage: workcell run start --run SLUG [--demand-json PATH | demand flags] [--rung local|remote|sandbox] [--machine LABEL] [--correlation FILE...] | observe --run SLUG [--correlate FILE] | collect --run SLUG | release --run SLUG | list | show --run SLUG | scope --run SLUG --policy-revision REV [--scope-out PATH] [--place-grant FILE]";

fn command_run(global: &GlobalArgs, args: &[String]) -> Result<(), WorkcellError> {
    let Some(subcommand) = args.first().map(String::as_str) else {
        return Err(WorkcellError::InvalidDemand(RUN_USAGE.into()));
    };
    let rest = &args[1..];
    match subcommand {
        "start" => run_start(global, rest),
        "observe" => run_observe(global, rest),
        "collect" => run_collect(global, rest),
        "release" => run_release(global, rest),
        "list" => run_list(global),
        "show" => run_show(global, rest),
        "scope" => run_scope(global, rest),
        other => Err(WorkcellError::InvalidDemand(format!(
            "unknown run subcommand `{other}`; {RUN_USAGE}"
        ))),
    }
}

/// One `--flag value` scanner for the run verbs (positional-free).
fn run_flag(args: &[String], name: &str) -> Result<Option<String>, WorkcellError> {
    let mut found = None;
    let mut index = 0;
    while index < args.len() {
        if args[index].as_str() == name {
            if found.is_some() {
                return Err(WorkcellError::InvalidDemand(format!(
                    "{name} may be given at most once"
                )));
            }
            found = Some(require_value(args, index, name)?.to_owned());
            index += 2;
        } else {
            index += 1;
        }
    }
    Ok(found)
}

fn run_flag_multiple(args: &[String], name: &str) -> Result<Vec<String>, WorkcellError> {
    let mut found = Vec::new();
    let mut index = 0;
    while index < args.len() {
        if args[index].as_str() == name {
            found.push(require_value(args, index, name)?.to_owned());
            index += 2;
        } else {
            index += 1;
        }
    }
    Ok(found)
}

fn demand_json_digest(demand: &ExecutionDemand) -> Result<String, WorkcellError> {
    use sha2::{Digest, Sha256};
    let encoded = epilogos_workcell_control::codec::demand_value(demand).to_string();
    Ok(format!("sha256:{:x}", Sha256::digest(encoded.as_bytes())))
}

fn require_run(global: &GlobalArgs, args: &[String]) -> Result<(RunLedger, Value), WorkcellError> {
    let slug = run_flag(args, "--run")?.ok_or_else(|| {
        WorkcellError::InvalidDemand("this command requires `--run <slug>`".into())
    })?;
    let ledger = RunLedger::new(&global.state_root);
    let record = ledger.get(&slug)?.ok_or_else(|| {
        WorkcellError::NotFound(format!(
            "run `{slug}` is not in this ledger; `workcell run list` shows what is"
        ))
    })?;
    Ok((ledger, record))
}

/// Append an attributed correlation observation
/// (`workcell.correlated-observation/v1` shape) onto the run's record. The
/// document is carried verbatim: Workcell adds attribution time, never edits
/// the verdict.
fn run_append_correlation(record: &mut Value, document: Value) -> Result<(), WorkcellError> {
    if !document.is_object() {
        return Err(WorkcellError::InvalidDemand(
            "a correlation document must be a JSON object".into(),
        ));
    }
    let mut entry = document;
    entry["recorded_at_unix_ms"] = json!(now_unix_ms());
    record["correlations"]
        .as_array_mut()
        .ok_or_else(|| {
            WorkcellError::OperationFailed(
                "run record correlations field is missing or not an array".into(),
            )
        })?
        .push(entry);
    Ok(())
}

fn run_start(global: &GlobalArgs, args: &[String]) -> Result<(), WorkcellError> {
    let slug = run_flag(args, "--run")?.ok_or_else(|| {
        WorkcellError::InvalidDemand("run start requires `--run <slug>`".into())
    })?;
    let rung = run_flag(args, "--rung")?.unwrap_or_else(|| "local".into());
    if !["local", "remote", "sandbox"].contains(&rung.as_str()) {
        return Err(WorkcellError::InvalidDemand(format!(
            "rung `{rung}` is outside the run vocabulary (local|remote|sandbox)"
        )));
    }
    let machine = run_flag(args, "--machine")?;
    if rung != "remote" && machine.is_some() {
        return Err(WorkcellError::InvalidDemand(
            "`--machine` belongs to the remote rung; pair it with `--rung remote`".into(),
        ));
    }
    let correlations = run_flag_multiple(args, "--correlation")?;
    let demand_flags = run_demand_flags(args);
    let agency_ref = run_flag(args, "--agency-ref")?;
    let agency_source = run_flag(args, "--agency-source")?;
    let harness_ref = run_flag(args, "--operative-harness")?;
    let connection_ref = run_flag(args, "--operative-connection")?;
    let model_ref = run_flag(args, "--operative-model")?;
    let agent_profile_ref = run_flag(args, "--operative-agent-profile")?;
    let canonical_run_ref = run_flag(args, "--canonical-run-ref")?;

    let demand = parse_demand(&demand_flags)?;
    demand.validate()?;
    if rung == "remote" && machine.is_none() {
        return Err(WorkcellError::InvalidDemand(
            "the remote rung requires `--machine LABEL` naming a declared machine (`workcell machine add`)"
                .into(),
        ));
    }

    let ledger = RunLedger::new(&global.state_root);
    if ledger.get(&slug)?.is_some() {
        return Err(WorkcellError::OperationFailed(format!(
            "run `{slug}` already exists; a run slug names one material execution"
        )));
    }

    let demand_digest = demand_json_digest(&demand)?;
    let mut record = json!({
        "schema": epilogos_workcell_runtime::RUN_SCHEMA,
        "run_slug": slug,
        "demand_ref": demand.demand_ref.as_str(),
        "canonical_run_ref": canonical_run_ref,
        "execution_status": "queued",
        "rung": rung,
        "provider_ref": if rung == "remote" {
            format!("control:{}", machine.clone().unwrap_or_default())
        } else {
            "provider:collapsed-local-host-process".to_owned()
        },
        "demand_digest": demand_digest,
        "world_receipt": Value::Null,
        "status_reason": Value::Null,
    });
    record["material_refs"] = json!([]);
    record["correlations"] = json!([]);
    record["deliverable"] = json!({"outputs": [], "branch": null});
    // A-1/A-6: optional agency and operative blocks. Both absent keeps the
    // run first-class on suite defaults (flagged owner question: absent
    // operative = suite-default harness + roster winner).
    if let Some(agency_ref) = &agency_ref {
        let agency_source = agency_source.as_ref().ok_or_else(|| {
            WorkcellError::InvalidDemand(
                "--agency-ref requires --agency-source <path to the minted agency source>"
                    .into(),
            )
        })?;
        let source_bytes = fs::read(agency_source).map_err(|error| {
            WorkcellError::NotFound(format!(
                "read agency source `{agency_source}`: {error}"
            ))
        })?;
        if source_bytes.len() > 1_048_576 {
            return Err(WorkcellError::InvalidDemand(
                "agency source exceeds 1 MiB".into(),
            ));
        }
        record["agency"] = json!({
            "agency_ref": agency_ref,
            "agency_rev": run_flag(args, "--agency-rev").unwrap_or_default(),
            "source_ref": agency_source,
            "source_digest": format!("blake3:{}", blake3::hash(&source_bytes).to_hex()),
            "binding_revision": run_flag(args, "--agency-binding-revision").unwrap_or_default(),
            "minted_by": run_flag(args, "--minted-by").unwrap_or_default(),
        });
        // The workcell run attaches to an already-actualised agency; it never
        // mints (A-3). A non-empty binding revision is the attach receipt.
        if record["agency"]["binding_revision"].as_str().unwrap_or("").is_empty() {
            return Err(WorkcellError::InvalidDemand(
                "attaching an agency requires --agency-binding-revision (the run attaches, it does not mint)"
                    .into(),
            ));
        }
    } else if agency_source.is_some() {
        return Err(WorkcellError::InvalidDemand(
            "--agency-source requires --agency-ref".into(),
        ));
    }
    if harness_ref.is_some()
        || connection_ref.is_some()
        || model_ref.is_some()
        || agent_profile_ref.is_some()
    {
        record["operative"] = json!({
            "harness_ref": harness_ref,
            "connection_ref": connection_ref,
            "model_ref": model_ref,
            "agent_profile_ref": agent_profile_ref,
        });
    }
    for correlation in &correlations {
        let document = read_json_file(Path::new(correlation))?;
        run_append_correlation(&mut record, document)?;
    }
    let record = ledger.create(record)?;

    let mut record = match rung.as_str() {
        "remote" => run_start_remote(global, args, record, &demand, &machine.expect("checked")),
        "sandbox" => run_start_sandbox(global, args, record, &demand),
        _ => run_start_local(global, record, &demand),
    }?;

    // Material provenance: every allocation this run's world carries.
    if let Some(receipt) = record["world_receipt"].as_str().map(PathBuf::from) {
        if let Ok(world) = load_world_receipt_path(&receipt) {
            let mut material_refs: Vec<String> = world
                .binding_graph
                .bindings
                .iter()
                .map(|binding| binding.material_ref.clone())
                .collect();
            material_refs.sort();
            material_refs.dedup();
            record["material_refs"] = json!(material_refs);
            record["world_ref"] = json!(world.world_ref.as_str());
        }
    }
    set_run_status(&mut record, "running", None)?;
    ledger.update(&record)?;
    if global.json {
        emit_json(json!({"ok": true, "run": record}));
    } else {
        println!(
            "run {} [{}] started ({}); world {}",
            record["run_slug"].as_str().unwrap_or(&slug),
            record["execution_status"].as_str().unwrap_or("?"),
            record["rung"].as_str().unwrap_or("?"),
            record["world_ref"].as_str().unwrap_or("unprepared"),
        );
    }
    Ok(())
}

/// Split `run start` args into the demand-flag tail. Everything after the
/// recognised run flags is handed to `parse_demand` unchanged, so a run demand
/// is expressed exactly like `workcell prepare --demand-json … | --require …`.
fn run_demand_flags(args: &[String]) -> Vec<String> {
    let recognised = [
        "--run", "--rung", "--machine", "--correlation", "--agency-ref", "--agency-source",
        "--agency-rev", "--agency-binding-revision", "--minted-by", "--operative-harness",
        "--operative-connection", "--operative-model", "--operative-agent-profile",
        "--canonical-run-ref",
    ];
    let mut demand_flags = Vec::new();
    let mut skip_next = false;
    for value in args {
        if skip_next {
            skip_next = false;
            continue;
        }
        if recognised.contains(&value.as_str()) {
            skip_next = true;
            continue;
        }
        demand_flags.push(value.clone());
    }
    demand_flags
}

fn read_json_file(path: &Path) -> Result<Value, WorkcellError> {
    let raw = fs::read(path).map_err(|error| {
        WorkcellError::NotFound(format!("read JSON file `{}`: {error}", path.display()))
    })?;
    if raw.len() > 4_194_304 {
        return Err(WorkcellError::InvalidDemand(format!(
            "JSON file `{}` exceeds 4 MiB",
            path.display()
        )));
    }
    serde_json::from_slice(&raw)
        .map_err(|error| WorkcellError::InvalidDemand(format!("parse JSON `{}`: {error}", path.display())))
}

fn load_world_receipt_path(path: &Path) -> Result<MaterialisedExecutionWorld, WorkcellError> {
    let encoded = fs::read_to_string(path).map_err(|error| {
        WorkcellError::NotFound(format!(
            "read material-world receipt `{}`: {error}",
            path.display()
        ))
    })?;
    decode_world(&encoded)
}

/// Local rung: prepare the material world here, write its receipt beside the
/// other durable receipts, and point the record at both.
fn run_start_local(
    global: &GlobalArgs,
    mut record: Value,
    demand: &ExecutionDemand,
) -> Result<Value, WorkcellError> {
    let channels = demand_output_channels(demand);
    let mut workcell = new_local(
        global,
        parse_workcell_ref(&global.workcell_ref)?,
        channels,
    )?;
    let world = workcell.prepare(demand)?;
    let receipt = global
        .receipt
        .clone()
        .unwrap_or_else(|| default_receipt_path(&global.state_root, world.world_ref.as_str()));
    write_receipt(&receipt, &world)?;
    record["world_receipt"] = json!(receipt.display().to_string());
    record["world_ref"] = json!(world.world_ref.as_str());
    Ok(record)
}

/// Sandbox rung: the composed OpenSandbox provider materialises the execution.
/// The run record is rung=sandbox; workspace material arrives as file-share or
/// volume mounts, and no long-lived place is claimed (the run records execd
/// endpoints, not a place grant). Requires a declared execution deployment.
fn run_start_sandbox(
    global: &GlobalArgs,
    _args: &[String],
    mut record: Value,
    demand: &ExecutionDemand,
) -> Result<Value, WorkcellError> {
    let channels = demand_output_channels(demand);
    let workcell = new_local(
        global,
        parse_workcell_ref(&global.workcell_ref)?,
        channels,
    )?;
    let discovery = workcell.discover()?;
    let sandbox_offer = discovery.offers.iter().find(|offer| {
        offer.port == ProviderPortKind::Execution.as_str()
            && offer.provider_ref.as_str().contains("opensandbox")
    });
    record["provider_ref"] = json!(sandbox_offer
        .map(|offer| offer.provider_ref.as_str().to_owned())
        .unwrap_or_else(|| "provider:opensandbox".to_owned()));
    let mut sandbox_demand = demand.clone();
    // The sandbox provider is execution-only: a sandbox run executes inside
    // the provider's own image, so a git-worktree branch law is refused
    // rather than silently dropped.
    if sandbox_demand.extensions.get("branch_law").map(String::as_str) == Some("aikit") {
        return Err(WorkcellError::InvalidDemand(
            "the sandbox rung has no git worktree materialisation; drop the branch_law extension or use the local rung"
                .into(),
        ));
    }
    sandbox_demand.isolation_trust = sandbox_demand.isolation_trust.take();
    let mut workcell = workcell;
    let world = workcell.prepare(&sandbox_demand)?;
    let receipt = global
        .receipt
        .clone()
        .unwrap_or_else(|| default_receipt_path(&global.state_root, world.world_ref.as_str()));
    write_receipt(&receipt, &world)?;
    record["world_receipt"] = json!(receipt.display().to_string());
    record["world_ref"] = json!(world.world_ref.as_str());
    Ok(record)
}

/// Remote rung: the same verbs over `workcell.control/v1`. The demand bytes
/// cross the endpoint; the workspace materialises on the remote machine under
/// its own providers; the world receipt stays there and is named by its
/// world_ref. Credentials resolve on the remote machine from demand refs.
fn run_start_remote(
    global: &GlobalArgs,
    args: &[String],
    mut record: Value,
    demand: &ExecutionDemand,
    machine_label: &str,
) -> Result<Value, WorkcellError> {
    let mut client = remote_client_for_machine(global, machine_label, args)?;
    let world_value = client
        .prepare(demand)
        .map_err(remote_error("prepare run material on the remote machine"))?;
    record["world_ref"] = json!(world_value["world_ref"].as_str().unwrap_or_default());
    record["remote_endpoint"] = json!(machine_label);
    Ok(record)
}

/// Build a control client bound to a declared machine, resolving the stored
/// credential ref exactly like `workcell connect` does.
fn remote_client_for_machine(
    global: &GlobalArgs,
    label: &str,
    _args: &[String],
) -> Result<ControlClient<TcpControlTransport>, WorkcellError> {
    let registry = RemoteMachineRegistry::new(&global.state_root);
    let machine = registry.get(label)?.ok_or_else(|| {
        WorkcellError::NotFound(format!(
            "machine `{label}` is not declared; add it with `workcell machine add --label {label} --endpoint HOST:PORT`"
        ))
    })?;
    let authorization = machine
        .credential_ref
        .as_deref()
        .map(|credential_ref| {
            resolve_connection_credential(None, Some(credential_ref))?
                .ok_or_else(|| WorkcellError::Unavailable(format!(
                    "machine `{label}` declares credential ref `{credential_ref}` but it could not be resolved"
                )))
        })
        .transpose()?;
    let mut client = ControlClient::new(TcpControlTransport::new(machine.endpoint.clone()));
    if let Some(authorization) = authorization {
        client = client.with_authorization(authorization);
    }
    Ok(client)
}

fn remote_error(
    context: &'static str,
) -> impl Fn(ControlClientError) -> WorkcellError {
    move |error| match error {
        ControlClientError::TransportUnavailable(message) => WorkcellError::Unavailable(format!(
            "{context}: remote machine is unreachable: {message}"
        )),
        other => WorkcellError::OperationFailed(format!("{context}: {other}")),
    }
}

fn run_observe(global: &GlobalArgs, args: &[String]) -> Result<(), WorkcellError> {
    let (ledger, mut record) = require_run(global, args)?;
    let correlate = run_flag(args, "--correlate")?;
    if let Some(document_path) = &correlate {
        let document = read_json_file(Path::new(document_path))?;
        run_append_correlation(&mut record, document)?;
    }
    let receipt = record["world_receipt"].as_str().map(PathBuf::from);
    let mut reason = String::new();
    let healthy = match (record["rung"].as_str(), receipt.as_ref()) {
        (Some("remote"), _) => {
            let mut client = remote_client_for_record(global, &record)?;
            match client
                .observe(&run_world_ref(&record)?)
                .map_err(remote_error("observe run on the remote machine"))
            {
                Ok(observation) => {
                    reason = format!(
                        "remote observation health: {}",
                        observation["health"].as_str().unwrap_or("unknown")
                    );
                    observation["health"].as_str() == Some("healthy")
                }
                Err(error) => {
                    reason = error.to_string();
                    false
                }
            }
        }
        (_, Some(receipt)) => {
            let (workcell, world, _) = resume_receipt(global, receipt)?;
            let bundle = workcell.observe(&world.world_ref)?;
            let unhealthy = bundle
                .observations
                .iter()
                .find(|observation| observation.state != HealthState::Healthy);
            match unhealthy {
                None => true,
                Some(observation) => {
                    reason = format!(
                        "material `{}` is unhealthy",
                        observation.logical_ref
                    );
                    false
                }
            }
        }
        _ => {
            reason = "run has no material world receipt yet".into();
            false
        }
    };
    let current = record["execution_status"].as_str().unwrap_or("queued").to_owned();
    let status = if current == "running" {
        if healthy { "running" } else { "blocked" }
    } else {
        current.as_str()
    };
    if status != current.as_str() {
        set_run_status(
            &mut record,
            status,
            if healthy { None } else { Some(reason.as_str()) },
        )?;
    } else if !healthy {
        record["status_reason"] = json!(reason);
    }
    ledger.update(&record)?;
    if global.json {
        emit_json(json!({"ok": true, "run": record}));
    } else {
        println!(
            "run {} — {}{}",
            record["run_slug"].as_str().unwrap_or("?"),
            record["execution_status"].as_str().unwrap_or("?"),
            record["status_reason"]
                .as_str()
                .map(|reason| format!(" ({reason})"))
                .unwrap_or_default(),
        );
        if !record["correlations"].as_array().unwrap_or(&Vec::new()).is_empty() {
            println!(
                "correlations: {}",
                record["correlations"].as_array().unwrap().len()
            );
        }
    }
    Ok(())
}

/// Deliverable branch facts for a worktree run: the checked-out branch, its
/// tip commit, and whether any remote-tracking ref on this machine already
/// contains the tip. A detached worktree records `branch: null`.
fn worktree_branch_deliverable(record: &Value) -> Result<Option<Value>, WorkcellError> {
    let Some(receipt) = record["world_receipt"].as_str().map(PathBuf::from) else {
        return Ok(None);
    };
    let world = load_world_receipt_path(&receipt)?;
    let git_binding = world.binding_graph.bindings.iter().find(|binding| {
        binding.material_ref.starts_with("workspace:git-worktree:")
    });
    let Some(binding) = git_binding else {
        return Ok(None);
    };
    let Some(path) = binding.properties.get("path") else {
        return Ok(None);
    };
    let facts = epilogos_workcell_runtime::git_branch_facts(Path::new(path))?;
    Ok(Some(json!({
        "name": facts.branch,
        "commit": facts.commit,
        "pushed": facts.pushed,
    })))
}

fn run_collect(global: &GlobalArgs, args: &[String]) -> Result<(), WorkcellError> {
    let (ledger, mut record) = require_run(global, args)?;
    let outputs = match record["rung"].as_str() {
        Some("remote") => {
            let mut client = remote_client_for_record(global, &record)?;
            client
                .collect(&run_world_ref(&record)?)
                .map_err(remote_error("collect run outputs from the remote machine"))?
        }
        _ => {
            let receipt = record["world_receipt"].as_str().map(PathBuf::from).ok_or_else(|| {
                WorkcellError::OperationFailed(
                    "run has no material world receipt; collect needs a prepared world".into(),
                )
            })?;
            let (workcell, world, _) = resume_receipt(global, &receipt)?;
            let collection = workcell.collect(&world.world_ref)?;
            json!({
                "outputs": collection.outputs.iter().map(|output| json!({
                    "logical_ref": output.logical_ref,
                    "material_locator": output.material_locator,
                })).collect::<Vec<_>>(),
            })
        }
    };
    record["deliverable"]["outputs"] = outputs["outputs"].clone();
    record["deliverable"]["branch"] = worktree_branch_deliverable(&record)?.unwrap_or(Value::Null);
    set_run_status(&mut record, "returned", None)?;
    ledger.update(&record)?;
    if global.json {
        emit_json(json!({
            "ok": true,
            "run_slug": record["run_slug"],
            "execution_status": record["execution_status"],
            "deliverable": record["deliverable"],
        }));
    } else {
        println!(
            "run {} collected (returned — recognition pending)",
            record["run_slug"].as_str().unwrap_or("?")
        );
        for output in record["deliverable"]["outputs"].as_array().unwrap_or(&Vec::new()) {
            println!(
                "  {} -> {}",
                output["logical_ref"].as_str().unwrap_or("?"),
                output["material_locator"].as_str().unwrap_or("?")
            );
        }
        if let Some(branch) = record["deliverable"]["branch"].as_object() {
            println!(
                "  branch {} at {}{}",
                branch["name"].as_str().unwrap_or("(detached)"),
                branch["commit"].as_str().unwrap_or("?"),
                if branch["pushed"].as_bool() == Some(true) {
                    " (pushed)"
                } else {
                    " (not pushed)"
                },
            );
        }
    }
    Ok(())
}

fn run_release(global: &GlobalArgs, args: &[String]) -> Result<(), WorkcellError> {
    let (ledger, mut record) = require_run(global, args)?;
    let release = match record["rung"].as_str() {
        Some("remote") => {
            let mut client = remote_client_for_record(global, &record)?;
            let value = client
                .release(&run_world_ref(&record)?)
                .map_err(remote_error("release run material on the remote machine"))?;
            Ok(ReleaseResult {
                world_ref: run_world_ref(&record)?,
                disposition: match value["disposition"].as_str() {
                    Some("preserved") => ReleaseDisposition::Preserved,
                    Some("suspended") => ReleaseDisposition::Suspended,
                    Some("snapshotted") => ReleaseDisposition::Snapshotted,
                    _ => ReleaseDisposition::Released,
                },
                changed: value["changed"].as_bool() == Some(true),
            })
        }
        _ => {
            let receipt = record["world_receipt"].as_str().map(PathBuf::from).ok_or_else(|| {
                WorkcellError::OperationFailed(
                    "run has no material world receipt; release needs a prepared world".into(),
                )
            })?;
            let (mut workcell, world, _) = resume_receipt(global, &receipt)?;
            workcell.release(&world.world_ref)
        }
    };
    match release {
        Ok(result) => {
            set_run_status(&mut record, "success", None)?;
            record["release_disposition"] = json!(match result.disposition {
                ReleaseDisposition::Released => "released",
                ReleaseDisposition::Preserved => "preserved",
                ReleaseDisposition::Suspended => "suspended",
                ReleaseDisposition::Snapshotted => "snapshotted",
            });
            ledger.update(&record)?;
            if global.json {
                emit_json(json!({"ok": true, "run": record}));
            } else {
                println!(
                    "run {} released — success",
                    record["run_slug"].as_str().unwrap_or("?")
                );
            }
            Ok(())
        }
        Err(error) => {
            // A dirty-worktree refusal (or any cleanup failure) surfaces as
            // run `blocked` with the reason — the material is never silently
            // discarded, and the record is never deleted.
            let reason = format!("release refused: {error}");
            set_run_status(&mut record, "blocked", Some(&reason))?;
            ledger.update(&record)?;
            if global.json {
                emit_json(json!({
                    "ok": false,
                    "run": record,
                    "error": {"kind": "cleanup-failed", "message": reason},
                }));
            } else {
                println!(
                    "run {} — blocked ({reason})",
                    record["run_slug"].as_str().unwrap_or("?")
                );
            }
            Err(WorkcellError::CleanupFailed(reason))
        }
    }
}

fn run_list(global: &GlobalArgs) -> Result<(), WorkcellError> {
    let ledger = RunLedger::new(&global.state_root);
    let runs = ledger.list()?;
    if global.json {
        emit_json(json!({
            "ok": true,
            "runs": runs.iter().map(|record| json!({
                "run_slug": record["run_slug"],
                "demand_ref": record["demand_ref"],
                "canonical_run_ref": record["canonical_run_ref"],
                "execution_status": record["execution_status"],
                "rung": record["rung"],
                "world_ref": record["world_ref"],
                "created_at_unix_ms": record["created_at_unix_ms"],
                "closed_at_unix_ms": record["closed_at_unix_ms"],
            })).collect::<Vec<_>>(),
        }));
    } else if runs.is_empty() {
        println!("no runs in this ledger");
    } else {
        for record in &runs {
            println!(
                "{} [{}] rung={} demand={}{}",
                record["run_slug"].as_str().unwrap_or("?"),
                record["execution_status"].as_str().unwrap_or("?"),
                record["rung"].as_str().unwrap_or("?"),
                record["demand_ref"].as_str().unwrap_or("?"),
                record["world_ref"]
                    .as_str()
                    .map(|world| format!(" world={world}"))
                    .unwrap_or_default(),
            );
        }
    }
    Ok(())
}

fn run_show(global: &GlobalArgs, args: &[String]) -> Result<(), WorkcellError> {
    let (_, record) = require_run(global, args)?;
    if global.json {
        emit_json(json!({"ok": true, "run": record}));
    } else {
        println!(
            "run {} [{}] rung={}",
            record["run_slug"].as_str().unwrap_or("?"),
            record["execution_status"].as_str().unwrap_or("?"),
            record["rung"].as_str().unwrap_or("?"),
        );
        for field in [
            "demand_ref",
            "canonical_run_ref",
            "world_ref",
            "provider_ref",
            "demand_digest",
            "status_reason",
            "boundary_digest",
        ] {
            if !record[field].is_null() {
                println!("{field}: {}", record[field]);
            }
        }
        if !record["material_refs"].as_array().unwrap_or(&Vec::new()).is_empty() {
            println!(
                "material: {}",
                record["material_refs"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .map(|value| value.as_str().unwrap_or("?"))
                    .collect::<Vec<_>>()
                    .join(", ")
            );
        }
        if record["agency"].is_object() {
            println!("agency: {}", record["agency"]["agency_ref"]);
        }
        if record["operative"].is_object() {
            println!("operative: {}", record["operative"]);
        }
        if let Some(branch) = record["deliverable"]["branch"].as_object() {
            println!(
                "branch: {} at {}{}",
                branch["name"].as_str().unwrap_or("(detached)"),
                branch["commit"].as_str().unwrap_or("?"),
                if branch["pushed"].as_bool() == Some(true) { " (pushed)" } else { "" },
            );
        }
    }
    Ok(())
}

/// `workcell run scope` — compose `workcell.prepared-run-scope/v1`: the
/// receipts an encounter resident needs to be born inside the run. The write
/// boundary is the real `workcell.prepared-write-boundary/v1` object; where
/// this OS has no material write adapter, the boundary is `null` with a named
/// degradation — never a weaker stand-in.
fn run_scope(global: &GlobalArgs, args: &[String]) -> Result<(), WorkcellError> {
    let (ledger, mut record) = require_run(global, args)?;
    let policy_revision = run_flag(args, "--policy-revision")?
        .ok_or_else(|| {
            WorkcellError::InvalidDemand(
                "run scope requires --policy-revision REV (the placement policy revision the boundary is prepared under)"
                    .into(),
            )
        })?;
    let scope_out = run_flag(args, "--scope-out")?;
    let place_grant = match run_flag(args, "--place-grant")? {
        Some(path) => Some(read_json_file(Path::new(&path))?),
        None => None,
    };

    // The worktree material this run must bind the resident to.
    let receipt = record["world_receipt"].as_str().map(PathBuf::from).ok_or_else(|| {
        WorkcellError::OperationFailed(
            "run scope needs a prepared material world; start the run first".into(),
        )
    })?;
    let world = load_world_receipt_path(&receipt)?;
    let git_binding = world
        .binding_graph
        .bindings
        .iter()
        .find(|binding| binding.material_ref.starts_with("workspace:git-worktree:"))
        .ok_or_else(|| {
            WorkcellError::OperationFailed(
                "run scope needs a git-worktree workspace binding; start the run on the local rung with the branch_law: aikit extension"
                    .into(),
            )
        })?;
    let worktree_path = git_binding
        .properties
        .get("path")
        .ok_or_else(|| {
            WorkcellError::OperationFailed(
                "run scope git-worktree binding has no material path".into(),
            )
        })?
        .clone();

    // The prepared write boundary: prepared-not-executed, exact object and
    // digest, or null with a named degradation on an OS without an adapter.
    use epilogos_workcell_runtime::{
        WriteBoundaryRequirements, PreparedWriteBoundary,
    };
    let coverage = epilogos_workcell_runtime::WRITE_BOUNDARY_COVERAGE
        .iter()
        .map(|value| value.to_string())
        .collect::<Vec<_>>();
    let requirements = WriteBoundaryRequirements {
        policy_ref: format!(
            "workcell.run/{}/write-boundary",
            record["run_slug"].as_str().unwrap_or("?")
        ),
        policy_revision: policy_revision.clone(),
        authority_ref: format!(
            "authority:workcell-run/{}",
            record["run_slug"].as_str().unwrap_or("?")
        ),
        writable_paths: vec![PathBuf::from(&worktree_path)],
        protected_paths: Vec::new(),
        required_coverage: coverage,
        expires_at_unix_ms: now_unix_ms() + 86_400_000,
    };
    let (prepared_write_boundary, boundary_digest, degradation) =
        match PreparedWriteBoundary::prepare(requirements, &policy_revision) {
            Ok(prepared) => {
                let inspection = prepared.inspect(&policy_revision)?;
                let digest = inspection["requirements_digest"].as_str().map(str::to_owned);
                (Some(inspection), digest, None)
            }
            Err(error) => (None, None, Some(error.to_string())),
        };
    record["boundary_digest"] = match &boundary_digest {
        Some(digest) => json!(digest),
        None => Value::Null,
    };
    ledger.update(&record)?;

    let mut scope =
        compose_prepared_run_scope(
            &record,
            &worktree_path,
            git_binding.material_ref.as_str(),
            prepared_write_boundary.as_ref(),
            place_grant.as_ref(),
        );
    if let Some(reason) = degradation.clone() {
        scope["degradations"] = json!([{
            "subject_ref": "prepared_write_boundary",
            "state": "unavailable",
            "reason": reason,
        }]);
    }

    let out_path = scope_out.map(PathBuf::from).unwrap_or_else(|| {
        global
            .state_root
            .join(epilogos_workcell_runtime::RUNS_DIRECTORY)
            .join(format!(
                "{}.scope.json",
                record["run_slug"].as_str().unwrap_or("run")
            ))
    });
    if let Some(parent) = out_path.parent() {
        fs::create_dir_all(parent).map_err(|error| {
            WorkcellError::OperationFailed(format!("create scope directory: {error}"))
        })?;
    }
    fs::write(
        &out_path,
        serde_json::to_vec_pretty(&scope).map_err(|error| {
            WorkcellError::OperationFailed(format!("encode prepared-run-scope: {error}"))
        })?,
    )
    .map_err(|error| {
        WorkcellError::OperationFailed(format!(
            "write prepared-run-scope `{}`: {error}",
            out_path.display()
        ))
    })?;

    if global.json {
        emit_json(json!({
            "ok": true,
            "scope": scope,
            "scope_path": out_path.display().to_string(),
        }));
    } else {
        println!(
            "prepared run scope for {} written to {}",
            record["run_slug"].as_str().unwrap_or("?"),
            out_path.display()
        );
        println!("worktree: {worktree_path}");
        match &prepared_write_boundary {
            Some(_) => println!(
                "write boundary: prepared-not-executed (digest {})",
                boundary_digest.as_deref().unwrap_or("?")
            ),
            None => println!(
                "write boundary: unavailable on this OS — {}",
                degradation.as_deref().unwrap_or("unknown reason")
            ),
        }
    }
    Ok(())
}

fn remote_client_for_record(
    global: &GlobalArgs,
    record: &Value,
) -> Result<ControlClient<TcpControlTransport>, WorkcellError> {
    let machine = record["remote_endpoint"]
        .as_str()
        .ok_or_else(|| {
            WorkcellError::OperationFailed(
                "remote run record names no machine; the record cannot be continued"
                    .into(),
            )
        })?
        .to_owned();
    remote_client_for_machine(global, &machine, &[])
}

/// The recorded world identity of a run, for control verbs over the wire.
fn run_world_ref(record: &Value) -> Result<WorldRef, WorkcellError> {
    WorldRef::new(
        record["world_ref"]
            .as_str()
            .filter(|value| !value.is_empty())
            .ok_or_else(|| {
                WorkcellError::OperationFailed(
                    "run record names no world; the remote run cannot be continued".into(),
                )
            })?,
    )
    .map_err(WorkcellError::from)
}

/// Resume a prepared world from an explicit receipt path (the run's recorded
/// receipt, not the caller's --receipt).
fn resume_receipt(
    global: &GlobalArgs,
    receipt: &Path,
) -> Result<
    (
        CollapsedLocalWorkcell,
        MaterialisedExecutionWorld,
        PathBuf,
    ),
    WorkcellError,
> {
    let encoded = fs::read_to_string(receipt).map_err(|error| {
        WorkcellError::NotFound(format!(
            "read material-world receipt `{}`: {error}",
            receipt.display()
        ))
    })?;
    let world = decode_world(&encoded)?;
    let channels = world_artifact_channels(&world);
    let mut config = with_declared_services(
        CollapsedLocalConfig::new(world.workcell_ref.clone(), &global.state_root),
        global,
    );
    config.artifact_channels = channels.into_iter().collect();
    let mut workcell = CollapsedLocalWorkcell::new(config)?;
    workcell.register_world(world.clone())?;
    Ok((workcell, world, receipt.to_path_buf()))
}

fn command_serve(global: &GlobalArgs, args: &[String]) -> Result<(), WorkcellError> {
    // `workcell serve` alone runs the daemon in the foreground; the
    // `enable|disable|status` forms manage the declared-service route that
    // keeps a serving cell reachable across reboots.
    match args.first().map(String::as_str) {
        Some("enable") => return serve_enable(global, &args[1..]),
        Some("disable") => return serve_disable(global, &args[1..]),
        Some("status") => return serve_status(global, &args[1..]),
        Some("stop") => return serve_stop(global, &args[1..]),
        _ => {}
    }

    let mut listen = "127.0.0.1:7777".to_owned();
    let mut authorization = env::var("WORKCELL_CONTROL_TOKEN").ok();
    let mut index = 0;
    while index < args.len() {
        match args[index].as_str() {
            "--listen" => {
                listen = require_value(args, index, "--listen")?.to_owned();
                index += 2;
            }
            "--authorization" => {
                authorization = Some(require_value(args, index, "--authorization")?.to_owned());
                index += 2;
            }
            other => {
                return Err(WorkcellError::InvalidDemand(format!(
                    "unknown serve option `{other}`; expected `--listen HOST:PORT` or `--authorization TOKEN`"
                )))
            }
        }
    }

    let workcell_ref = parse_workcell_ref(&global.workcell_ref)?;
    let grants = ConnectionGrants::new(&global.state_root, workcell_ref.clone());
    let active_grants = grants.active_count()?;
    if !is_loopback_listener(&listen) && authorization.is_none() && active_grants == 0 {
        return Err(WorkcellError::Unavailable(
            "a non-loopback `workcell serve` requires an authorization token or at least one active connection grant; run `workcell authorise` first".into(),
        ));
    }

    let mut config = with_declared_services(
        CollapsedLocalConfig::new(workcell_ref, &global.state_root)
            .with_persistent_host_lifetime(),
        global,
    );
    if let Some(source) = &global.workspace_source {
        config = config.with_workspace_source(source);
    }
    let workcell = epilogos_workcell_cli::DurableCollapsedLocalWorkcell::new(config)?;
    // The serving cell answers `system` with the same descriptor this host
    // prints, so `workcell --endpoint … system` reads the machine's own
    // disclosure rather than a fabricated remote reading.
    let disclosure_global = global.clone();
    let service = match authorization {
        Some(token) => ControlService::new(workcell)
            .with_authorization(token)
            .with_connection_grants(grants)
            .with_system_disclosure(std::sync::Arc::new(move || {
                system_descriptor(&disclosure_global)
            })),
        None => ControlService::new(workcell)
            .with_connection_grants(grants)
            .with_system_disclosure(std::sync::Arc::new(move || {
                system_descriptor(&disclosure_global)
            })),
    };
    let mut server =
        TcpControlServer::bind(&listen, service).map_err(|error| {
            WorkcellError::OperationFailed(format!("bind serve endpoint: {error}"))
        })?;
    let bound = server.local_addr().map_err(|error| {
        WorkcellError::OperationFailed(format!("read bound serve endpoint: {error}"))
    })?;
    if global.json {
        emit_json(json!({
            "ok": true,
            "listening": bound.to_string(),
            "protocol": CONTROL_PROTOCOL_VERSION,
            "software": software_version(),
            "settings_disclosure": true,
        }));
    } else {
        println!("serving {}", bound);
        println!("protocol: {CONTROL_PROTOCOL_VERSION}");
        println!("software: {}", software_version());
    }
    eprintln!(
        "workcell serve: listening on {} ({}) — authorisation is enforced per request from the grants registry",
        bound, CONTROL_PROTOCOL_VERSION
    );
    // The pid file is the target-native stop surface for the declared
    // service form (`workcell serve stop --listen` / ensure-running's
    // stop-on-refusal). It is written only after a successful bind.
    let pid_path = serve_pid_path(&global.state_root);
    fs::write(&pid_path, std::process::id().to_string()).map_err(|error| {
        WorkcellError::OperationFailed(format!("write serve pid file: {error}"))
    })?;
    let result = server.serve().map_err(|error| {
        WorkcellError::OperationFailed(format!("serve connection endpoint: {error}"))
    });
    let _ = fs::remove_file(&pid_path);
    result
}

const SERVE_ENABLE_USAGE: &str = "usage: workcell serve enable --listen HOST:PORT [--authorization-from-env] [--note TEXT] | workcell serve disable --listen HOST:PORT | workcell serve status --listen HOST:PORT | workcell serve stop --listen HOST:PORT";

/// Where `workcell serve` records its daemon pid, so the declared-service
/// stop command has a target-native stop surface.
fn serve_pid_path(state_root: &Path) -> PathBuf {
    state_root.join("serve.pid")
}

fn serve_listen_address(args: &[String]) -> Result<String, WorkcellError> {
    let mut listen = "127.0.0.1:7777".to_owned();
    let mut index = 0;
    while index < args.len() {
        match args[index].as_str() {
            "--listen" => {
                listen = require_value(args, index, "--listen")?.to_owned();
                index += 2;
            }
            other => {
                return Err(WorkcellError::InvalidDemand(format!(
                    "unknown serve option `{other}`; {SERVE_ENABLE_USAGE}"
                )))
            }
        }
    }
    Ok(listen)
}

fn current_exe_path() -> Result<PathBuf, WorkcellError> {
    env::current_exe().map_err(|error| {
        WorkcellError::OperationFailed(format!("resolve current executable: {error}"))
    })
}

/// `workcell serve enable` — declare this cell's own daemon through the #98
/// operator-declared services mechanism: a target-owned `services.json`
/// entry with `acquisition: ensure-running`, a TCP readiness window and a
/// target-native stop command, so a declared machine is reachable again
/// after reboot when any demand materialises the service. Nothing is started
/// here: declaration is not availability, and this verb never leaves a
/// running process behind.
fn serve_enable(global: &GlobalArgs, args: &[String]) -> Result<(), WorkcellError> {
    let listen = serve_listen_address(args)?;
    if !is_loopback_listener(&listen) {
        let authorization = env::var("WORKCELL_CONTROL_TOKEN").ok();
        let workcell_ref = parse_workcell_ref(&global.workcell_ref)?;
        let grants = ConnectionGrants::new(&global.state_root, workcell_ref);
        if authorization.is_none() && grants.active_count()? == 0 {
            return Err(WorkcellError::Unavailable(
                "a non-loopback serve declaration requires an authorization token in WORKCELL_CONTROL_TOKEN or at least one active connection grant; run `workcell authorise` first".into(),
            ));
        }
    }
    let exe = current_exe_path()?;
    let exe = exe.display().to_string();
    let service = json!({
        "lifetime": "target-owned",
        "logical_ref": "service:workcell-control",
        "endpoint": format!("tcp://{listen}"),
        "status": {
            "program": exe,
            "args": ["serve", "status", "--listen", listen],
        },
        "readiness": {
            "program": exe,
            "args": ["serve", "status", "--listen", listen],
            "timeout_ms": 10_000,
            "interval_ms": 200,
        },
        "start": {
            "program": exe,
            "args": ["serve", "--listen", listen],
        },
        "stop": {
            "program": exe,
            "args": ["serve", "stop", "--listen", listen],
        },
        "acquisition": "ensure-running",
    });
    let declared =
        epilogos_workcell_runtime::read_state_root_service_declarations(&global.state_root)?;
    if declared
        .target_owned
        .iter()
        .any(|service| service.logical_ref == "service:workcell-control")
    {
        return Err(WorkcellError::OperationFailed(
            "logical service `service:workcell-control` is already declared; disable it first with `workcell serve disable`".into(),
        ));
    }
    write_serve_declaration(global, &service)?;

    if global.json {
        emit_json(json!({
            "ok": true,
            "enabled": true,
            "logical_ref": "service:workcell-control",
            "listen": listen,
            "acquisition": "ensure-running",
            "declaration": epilogos_workcell_runtime::default_service_declaration_path(&global.state_root),
            "note": "the daemon is declared, not started here; ensure-running materialises it when a demand resolves the service",
        }));
    } else {
        println!(
            "serve declared at {listen} (service:workcell-control, ensure-running, readiness window 10s); nothing was started"
        );
        println!(
            "declaration: {}",
            epilogos_workcell_runtime::default_service_declaration_path(&global.state_root).display()
        );
    }
    Ok(())
}

/// Append (or replace) the `service:workcell-control` entry in the state
/// root's services.json, preserving every other declared service verbatim.
fn write_serve_declaration(global: &GlobalArgs, service: &Value) -> Result<(), WorkcellError> {
    let path =
        epilogos_workcell_runtime::default_service_declaration_path(&global.state_root);
    let mut document = match fs::read_to_string(&path) {
        Ok(raw) => serde_json::from_str::<Value>(&raw).map_err(|error| {
            WorkcellError::OperationFailed(format!(
                "parse existing service declaration `{}`: {error}",
                path.display()
            ))
        })?,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => json!({
            "schema": epilogos_workcell_runtime::SERVICE_DECLARATION_SCHEMA,
        }),
        Err(error) => {
            return Err(WorkcellError::OperationFailed(format!(
                "read service declaration `{}`: {error}",
                path.display()
            )))
        }
    };
    if !document.is_object() {
        return Err(WorkcellError::OperationFailed(
            "existing service declaration must be a JSON object".into(),
        ));
    }
    if document.get("schema").and_then(Value::as_str)
        != Some(epilogos_workcell_runtime::SERVICE_DECLARATION_SCHEMA)
    {
        return Err(WorkcellError::OperationFailed(format!(
            "service declaration `{}` does not declare {}",
            path.display(),
            epilogos_workcell_runtime::SERVICE_DECLARATION_SCHEMA
        )));
    }
    let services = document
        .as_object_mut()
        .expect("checked object")
        .entry("services")
        .or_insert_with(|| json!([]));
    if !services.is_array() {
        return Err(WorkcellError::OperationFailed(
            "service declaration `services` must be an array".into(),
        ));
    }
    let array = services.as_array_mut().expect("checked array");
    array.retain(|entry| entry.get("logical_ref").and_then(Value::as_str) != Some("service:workcell-control"));
    array.push(service.clone());
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|error| {
            WorkcellError::OperationFailed(format!("create declaration directory: {error}"))
        })?;
    }
    fs::write(&path, serde_json::to_vec_pretty(&document).map_err(|error| {
        WorkcellError::OperationFailed(format!("encode service declaration: {error}"))
    })?)
    .map_err(|error| {
        WorkcellError::OperationFailed(format!(
            "write service declaration `{}`: {error}",
            path.display()
        ))
    })
}

/// `workcell serve disable` — remove the declared daemon entry, keeping every
/// other declared service verbatim. A running daemon is not killed here; the
/// declaration's stop command remains the target-native surface.
fn serve_disable(global: &GlobalArgs, args: &[String]) -> Result<(), WorkcellError> {
    let _ = serve_listen_address(args)?;
    let path =
        epilogos_workcell_runtime::default_service_declaration_path(&global.state_root);
    let mut document: Value = match fs::read_to_string(&path) {
        Ok(raw) => serde_json::from_str(&raw).map_err(|error| {
            WorkcellError::OperationFailed(format!(
                "parse service declaration `{}`: {error}",
                path.display()
            ))
        })?,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Err(WorkcellError::NotFound(
                "no service declaration exists; serve is not enabled".into(),
            ))
        }
        Err(error) => {
            return Err(WorkcellError::OperationFailed(format!(
                "read service declaration `{}`: {error}",
                path.display()
            )))
        }
    };
    let removed = match document.get_mut("services").and_then(Value::as_array_mut) {
        Some(array) => {
            let before = array.len();
            array.retain(|entry| {
                entry.get("logical_ref").and_then(Value::as_str) != Some("service:workcell-control")
            });
            before != array.len()
        }
        None => false,
    };
    if !removed {
        return Err(WorkcellError::NotFound(
            "serve is not declared in this state root's services.json".into(),
        ));
    }
    fs::write(&path, serde_json::to_vec_pretty(&document).map_err(|error| {
        WorkcellError::OperationFailed(format!("encode service declaration: {error}"))
    })?)
    .map_err(|error| {
        WorkcellError::OperationFailed(format!(
            "write service declaration `{}`: {error}",
            path.display()
        ))
    })?;
    if global.json {
        emit_json(json!({"ok": true, "enabled": false, "logical_ref": "service:workcell-control"}));
    } else {
        println!("serve declaration removed (service:workcell-control); other declared services kept");
    }
    Ok(())
}

/// `workcell serve status` — the declared service's status command: exit 0
/// when the declared endpoint answers, non-zero otherwise. Read-only.
fn serve_status(global: &GlobalArgs, args: &[String]) -> Result<(), WorkcellError> {
    let listen = serve_listen_address(args)?;
    let (host, port) = listen
        .rsplit_once(':')
        .ok_or_else(|| {
            WorkcellError::InvalidDemand(format!("serve listen `{listen}` must be HOST:PORT"))
        })?;
    let answering = std::net::TcpStream::connect((host, port.parse::<u16>().map_err(|_| {
        WorkcellError::InvalidDemand(format!("serve listen `{listen}` has an invalid port"))
    })?))
    .is_ok();
    if global.json {
        emit_json(json!({
            "ok": answering,
            "logical_ref": "service:workcell-control",
            "listen": listen,
            "state": if answering { "available" } else { "unavailable" },
        }));
    } else if answering {
        println!("serve: {listen} answering");
    } else {
        println!("serve: {listen} not answering");
    }
    if answering {
        Ok(())
    } else {
        Err(WorkcellError::Unavailable(format!(
            "serve endpoint {listen} is not answering"
        )))
    }
}

/// `workcell serve stop` — the declared service's target-native stop command:
/// read the daemon's pid file (written by `workcell serve` after a successful
/// bind), refuse unless the pid is alive, and terminate it. Never a broad
/// process scan — one named pid from a file inside this state root.
fn serve_stop(global: &GlobalArgs, args: &[String]) -> Result<(), WorkcellError> {
    let _ = serve_listen_address(args)?;
    let pid_path = serve_pid_path(&global.state_root);
    let raw = fs::read_to_string(&pid_path).map_err(|error| {
        WorkcellError::NotFound(format!(
            "no serve pid file at `{}` ({}); is a `workcell serve` daemon running from this state root?",
            pid_path.display(),
            error
        ))
    })?;
    let pid: u32 = raw.trim().parse().map_err(|error| {
        WorkcellError::OperationFailed(format!("parse serve pid file: {error}"))
    })?;
    let signalled = std::process::Command::new("kill")
        .arg("-TERM")
        .arg(pid.to_string())
        .status()
        .map(|status| status.success())
        .unwrap_or(false);
    if !signalled {
        return Err(WorkcellError::Unavailable(format!(
            "serve daemon pid {pid} could not be signalled; it may have already exited"
        )));
    }
    let _ = fs::remove_file(&pid_path);
    if global.json {
        emit_json(json!({"ok": true, "stopped": true, "pid": pid}));
    } else {
        println!("serve daemon pid {pid} stopped");
    }
    Ok(())
}

fn command_authorise(global: &GlobalArgs, args: &[String]) -> Result<(), WorkcellError> {
    let mut client_label = None;
    let mut operations: Vec<String> = Vec::new();
    let mut advertise: Vec<String> = Vec::new();
    let mut expires_in: Option<String> = None;
    let mut store_credential = false;
    let mut index = 0;
    while index < args.len() {
        match args[index].as_str() {
            "--client" => {
                client_label = Some(require_value(args, index, "--client")?.to_owned());
                index += 2;
            }
            "--allow" => {
                operations.push(require_value(args, index, "--allow")?.to_owned());
                index += 2;
            }
            "--advertise" => {
                advertise.push(require_value(args, index, "--advertise")?.to_owned());
                index += 2;
            }
            "--expires-in" => {
                expires_in = Some(require_value(args, index, "--expires-in")?.to_owned());
                index += 2;
            }
            "--store-credential" => {
                store_credential = true;
                index += 1;
            }
            other => {
                return Err(WorkcellError::InvalidDemand(format!(
                    "unknown authorise option `{other}`"
                )))
            }
        }
    }
    let label = client_label.ok_or_else(|| {
        WorkcellError::InvalidDemand(
            "usage: workcell authorise --client <label> --allow <operation>... [--advertise <port>...] [--expires-in <duration>] [--store-credential]".into(),
        )
    })?;
    validate_label(&label)?;
    if operations.is_empty() {
        return Err(WorkcellError::InvalidDemand(
            "authorise requires at least one `--allow <operation>`; a grant must say exactly what it permits".into(),
        ));
    }
    for operation in &operations {
        if !CONTROL_OPERATIONS.contains(&operation.as_str()) {
            return Err(WorkcellError::InvalidDemand(format!(
                "unknown control operation `{operation}`; known operations are {}",
                CONTROL_OPERATIONS.join(", ")
            )));
        }
    }
    // An optional lifetime: the grant stops authorising at
    // created + duration and refuses at the next use, named and distinct
    // from revocation. Without it the grant never expires.
    let expires_at_unix_ms = match expires_in.as_deref() {
        None => None,
        Some(duration) => Some(
            now_unix_ms()
                .checked_add(parse_duration_millis(duration)?)
                .ok_or_else(|| {
                    WorkcellError::InvalidDemand(format!(
                        "`--expires-in {duration}` overflows the expiry timestamp; choose a shorter lifetime"
                    ))
                })?,
        ),
    };

    let credential = generate_credential()?;
    let credential_ref = if store_credential {
        Some(store_connection_credential(&label, &credential)?)
    } else {
        None
    };

    let workcell_ref = parse_workcell_ref(&global.workcell_ref)?;
    let grants = ConnectionGrants::new(&global.state_root, workcell_ref);
    let grant = ConnectionGrant {
        grant_ref: grant_ref_for(&label, &credential),
        client_label: label.clone(),
        protocol: CONTROL_PROTOCOL_VERSION.to_owned(),
        operations: operations.clone(),
        advertise: advertise.clone(),
        credential_sha256: credential_sha256(&credential),
        credential_ref: credential_ref.clone(),
        created_at_unix_ms: now_unix_ms(),
        expires_at_unix_ms,
        state: "active".to_owned(),
        revoked_at_unix_ms: None,
        provenance: BTreeMap::from([("created_by".to_owned(), "workcell authorise".to_owned())]),
    };
    let grant_ref = grant.grant_ref.clone();
    match grants.create(grant)? {
        CreateOutcome::Registered => {}
        CreateOutcome::Conflict { existing } => {
            return Err(WorkcellError::InvalidDemand(format!(
                "an active grant `{}` already holds this credential for client `{}`; revoke it first with `workcell revoke --grant {}`",
                existing.grant_ref, existing.client_label, existing.grant_ref
            )));
        }
    }

    if global.json {
        emit_json(json!({
            "ok": true,
            "grant_ref": grant_ref,
            "client": label,
            "operations": operations,
            "advertise": advertise,
            "expires_at_unix_ms": expires_at_unix_ms,
            "credential": credential,
            "credential_ref": credential_ref,
            "note": "the credential is shown once and stored nowhere; only its SHA-256 is kept",
        }));
    } else {
        println!("granted {grant_ref} to client `{label}`");
        println!("operations: {}", operations.join(", "));
        if !advertise.is_empty() {
            println!("advertised ports: {}", advertise.join(", "));
        }
        match expires_at_unix_ms {
            Some(expires_at) => println!("expires: {expires_at} (unix ms)"),
            None => println!("expires: never"),
        }
        println!("credential: {credential}");
        println!(
            "store this credential now; it is not shown again and only its SHA-256 is kept{}",
            match &credential_ref {
                Some(reference) => format!(" (a copy is in {reference})"),
                None => String::new(),
            }
        );
    }
    Ok(())
}

fn command_revoke(global: &GlobalArgs, args: &[String]) -> Result<(), WorkcellError> {
    let mut client = None;
    let mut grant_ref = None;
    let mut index = 0;
    while index < args.len() {
        match args[index].as_str() {
            "--client" => {
                client = Some(require_value(args, index, "--client")?.to_owned());
                index += 2;
            }
            "--grant" => {
                grant_ref = Some(require_value(args, index, "--grant")?.to_owned());
                index += 2;
            }
            other => {
                return Err(WorkcellError::InvalidDemand(format!(
                    "unknown revoke option `{other}`"
                )))
            }
        }
    }
    let target = match (client, grant_ref) {
        (Some(client), None) => client,
        (None, Some(grant_ref)) => grant_ref,
        _ => {
            return Err(WorkcellError::InvalidDemand(
                "usage: workcell revoke --client <label> | --grant <ref> (exactly one)".into(),
            ))
        }
    };
    let workcell_ref = parse_workcell_ref(&global.workcell_ref)?;
    let grants = ConnectionGrants::new(&global.state_root, workcell_ref);
    let revoked = grants.revoke(&target)?;
    if global.json {
        emit_json(json!({
            "ok": true,
            "revoked": revoked.iter().map(|grant| json!({
                "grant_ref": grant.grant_ref,
                "client": grant.client_label,
            })).collect::<Vec<_>>(),
        }));
    } else {
        for grant in &revoked {
            println!(
                "revoked {} (client `{}`); the grant record is kept as audit evidence",
                grant.grant_ref, grant.client_label
            );
        }
    }
    Ok(())
}

fn command_connect(global: &GlobalArgs, args: &[String]) -> Result<(), WorkcellError> {
    let mut endpoint = None;
    let mut label = None;
    let mut authorization = None;
    let mut store_credential = false;
    let mut index = 0;
    while index < args.len() {
        match args[index].as_str() {
            "--endpoint" => {
                endpoint = Some(require_value(args, index, "--endpoint")?.to_owned());
                index += 2;
            }
            "--connection" => {
                label = Some(require_value(args, index, "--connection")?.to_owned());
                index += 2;
            }
            "--authorization" => {
                authorization = Some(require_value(args, index, "--authorization")?.to_owned());
                index += 2;
            }
            "--store-credential" => {
                store_credential = true;
                index += 1;
            }
            other => {
                return Err(WorkcellError::InvalidDemand(format!(
                    "unknown connect option `{other}`"
                )))
            }
        }
    }
    // A declared machine carries the endpoint (and the credential
    // reference, resolved from the origin secret source below) so the
    // ordinary connect verb works with a label alone.
    let machine: Option<RemoteMachineDeclaration> = match (&endpoint, &label) {
        (None, Some(label)) => {
            let registry = RemoteMachineRegistry::new(&global.state_root);
            match registry.get(label)? {
                Some(machine) => Some(machine),
                None => {
                    return Err(WorkcellError::InvalidDemand(format!(
                        "no endpoint given and no machine `{label}` is declared; add it with `workcell machine add --label {label} --endpoint HOST:PORT`"
                    )))
                }
            }
        }
        _ => None,
    };
    let endpoint = match endpoint {
        Some(endpoint) => endpoint,
        None => machine
            .as_ref()
            .map(|machine| machine.endpoint.clone())
            .ok_or_else(|| {
                WorkcellError::InvalidDemand(
                    "usage: workcell connect --endpoint HOST:PORT [--connection <label>] [--authorization TOKEN] [--store-credential]".into(),
                )
            })?,
    };
    let label = label.unwrap_or_else(|| safe_filename(&endpoint));
    validate_label(&label)?;
    let existing = load_connection_record(&global.state_root, &label)?;
    let declared_credential_ref = machine
        .as_ref()
        .and_then(|machine| machine.credential_ref.as_deref());
    let credential = resolve_connection_credential(
        authorization.as_deref(),
        existing
            .as_ref()
            .and_then(|record| record.credential_ref.as_deref())
            .or(declared_credential_ref),
    )?;

    // Material operations are not chatty: preparing a real world (a sandbox
    // booting in a VM, an image pulling) legitimately runs for minutes. The
    // read window is operator-tunable because a fixed 10s default silently
    // caps how long a remote materialisation may take.
    let operation_timeout = env::var("WORKCELL_CONTROL_TIMEOUT_SECS")
        .ok()
        .and_then(|value| value.trim().parse::<u64>().ok())
        .map(std::time::Duration::from_secs)
        .unwrap_or(std::time::Duration::from_secs(300));
    let mut client = ControlClient::new(
        TcpControlTransport::new(endpoint.clone()).with_timeout(Some(operation_timeout)),
    );
    if let Some(token) = &credential {
        client = client.with_authorization(token);
    }

    // 1. Handshake. A refused or unreachable handshake still updates the
    //    local record honestly before the loud refusal is returned.
    let handshake = match client.handshake(&label) {
        Ok(value) => value,
        Err(ControlClientError::ProtocolIncompatible(message)) => {
            store_connection_record(
                &global.state_root,
                reconciled_record(&existing, |record| {
                    record.label = label.clone();
                    record.endpoint = endpoint.clone();
                    record.state = "incompatible".to_owned();
                    record.detail = Some(message.clone());
                }),
                &label,
            )?;
            return Err(WorkcellError::Unsupported(message));
        }
        Err(ControlClientError::TransportUnavailable(message)) => {
            store_connection_record(
                &global.state_root,
                reconciled_record(&existing, |record| {
                    record.label = label.clone();
                    record.endpoint = endpoint.clone();
                    record.state = "disconnected".to_owned();
                    record.detail = Some(format!("endpoint unreachable: {message}"));
                }),
                &label,
            )?;
            return Err(WorkcellError::Unavailable(format!(
                "connect `{label}`: endpoint {endpoint} is unreachable: {message}"
            )));
        }
        Err(other) => return Err(*control_client_error(other)),
    };

    // 2. Compatibility. The protocol version is the contract; software
    //    revisions are reported, never refused on and never updated.
    let protocol = handshake["protocol"].as_str().unwrap_or("unknown").to_owned();
    let remote_software = handshake["software"].as_str().map(str::to_owned);
    let compatibility = match check_compatibility(
        &protocol,
        remote_software.as_deref().unwrap_or("unknown"),
    ) {
        Ok(report) => report,
        Err(error) => {
            store_connection_record(
                &global.state_root,
                reconciled_record(&existing, |record| {
                    record.label = label.clone();
                    record.endpoint = endpoint.clone();
                    record.protocol = protocol.clone();
                    record.remote_software = remote_software.clone();
                    record.state = "incompatible".to_owned();
                    record.detail = Some(error.to_string());
                }),
                &label,
            )?;
            return Err(error);
        }
    };

    let authorised = handshake["authorised"].as_bool().unwrap_or(false);
    if !authorised {
        let reason = handshake["reason"]
            .as_str()
            .unwrap_or("the remote cell did not authorise this connection")
            .to_owned();
        store_connection_record(
            &global.state_root,
            reconciled_record(&existing, |record| {
                record.label = label.clone();
                record.endpoint = endpoint.clone();
                record.protocol = protocol.clone();
                record.remote_software = remote_software.clone();
                record.state = "refused".to_owned();
                record.detail = Some(reason.clone());
                record.granted_operations = Vec::new();
            }),
            &label,
        )?;
        return Err(WorkcellError::Unavailable(format!(
            "connection refused by the remote cell: {reason}"
        )));
    }

    // 3. Granted scope. A null grant means the cell granted full access
    //    through its operator token; otherwise the grant names the truth —
    //    including when it stops authorising.
    let grant_payload = handshake.get("grant").filter(|value| !value.is_null());
    let granted_operations: Vec<String> = grant_payload
        .and_then(|grant| grant["operations"].as_array())
        .map(|operations| {
            operations
                .iter()
                .filter_map(Value::as_str)
                .map(str::to_owned)
                .collect()
        })
        .unwrap_or_else(|| CONTROL_OPERATIONS.iter().map(|value| value.to_string()).collect());
    let expires_at_unix_ms: Option<u64> = grant_payload
        .and_then(|grant| grant["expires_at_unix_ms"].as_u64());

    // 4. Probes under the grant: identity and, when discovery is granted,
    //    what the cell advertises to this connection specifically.
    let status = client
        .status()
        .map_err(|error| *control_client_error(error))?;
    let remote_workcell_ref = status["workcell_ref"]
        .as_str()
        .map(str::to_owned)
        .or_else(|| handshake["workcell_ref"].as_str().map(str::to_owned));
    let discovery = if granted_operations.iter().any(|operation| operation == "discover") {
        Some(
            client
                .discover()
                .map_err(|error| *control_client_error(error))?,
        )
    } else {
        None
    };
    let advertised_ports: Vec<String> = discovery
        .as_ref()
        .and_then(|value| value["offers"].as_array())
        .map(|offers| {
            offers
                .iter()
                .filter_map(|offer| offer["port"].as_str())
                .map(str::to_owned)
                .collect::<std::collections::BTreeSet<_>>()
                .into_iter()
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    let advertised_offers = discovery
        .as_ref()
        .and_then(|value| value["offers"].as_array())
        .map(Vec::len)
        .unwrap_or(0);

    // 5. Optional keychain storage of the credential material.
    let mut credential_ref =
        existing.as_ref().and_then(|record| record.credential_ref.clone());
    if store_credential {
        let credential = credential.as_deref().ok_or_else(|| {
            WorkcellError::InvalidDemand(
                "--store-credential requires a credential (pass --authorization or set WORKCELL_CONTROL_TOKEN)".into(),
            )
        })?;
        credential_ref = Some(store_connection_credential(&label, credential)?);
    }

    // 6. Reconciliation notes: what changed since the last connection.
    //    Reconnecting reports; it never overwrites source or software.
    let mut notes = compatibility.notes.clone();
    if let Some(previous) = &existing {
        if let Some(previous_ref) = &previous.remote_workcell_ref {
            if Some(previous_ref.as_str()) != remote_workcell_ref.as_deref() {
                notes.push(format!(
                    "the endpoint now identifies as `{}`, previously `{previous_ref}`",
                    remote_workcell_ref.as_deref().unwrap_or("unknown"),
                ));
            }
        }
        if let Some(previous_software) = &previous.remote_software {
            if Some(previous_software.as_str()) != remote_software.as_deref() {
                notes.push(format!(
                    "remote software changed since the last connection: `{previous_software}` -> `{}`; reported only, nothing was updated",
                    remote_software.as_deref().unwrap_or("unknown"),
                ));
            }
        }
    }
    // A declared machine records which operations the operator expects this
    // cell to serve. The grant, not the declaration, is the authority — but
    // drift between the two is exactly what connect exists to surface, so it
    // is named in both directions instead of leaving the field decorative.
    if let Some(machine) = &machine {
        if !machine.operations.is_empty() {
            let granted: std::collections::BTreeSet<&str> = granted_operations
                .iter()
                .map(String::as_str)
                .collect();
            let declared: std::collections::BTreeSet<&str> =
                machine.operations.iter().map(String::as_str).collect();
            let unexpected: Vec<&str> = granted.difference(&declared).copied().collect();
            let withheld: Vec<&str> = declared.difference(&granted).copied().collect();
            if !unexpected.is_empty() {
                notes.push(format!(
                    "the cell grants operations the `{label}` declaration does not expect: {}; the grant is the authority",
                    unexpected.join(", ")
                ));
            }
            if !withheld.is_empty() {
                notes.push(format!(
                    "the `{label}` declaration expects operations the grant does not permit: {}",
                    withheld.join(", ")
                ));
            }
        }
    }

    // 7. Durable client-side receipt.
    let now = now_unix_ms();
    let record = ConnectionRecord {
        connection_ref: format!("connection:{label}"),
        label: label.clone(),
        endpoint: endpoint.clone(),
        protocol,
        remote_workcell_ref: remote_workcell_ref.clone(),
        remote_software: remote_software.clone(),
        local_software: software_version(),
        granted_operations: granted_operations.clone(),
        credential_ref: credential_ref.clone(),
        state: "connected".to_owned(),
        detail: if notes.is_empty() {
            None
        } else {
            Some(notes.join("; "))
        },
        connected_at_unix_ms: existing
            .as_ref()
            .and_then(|record| record.connected_at_unix_ms)
            .or(Some(now)),
        expires_at_unix_ms,
        last_reconciled_at_unix_ms: now,
        provenance: existing
            .as_ref()
            .map(|record| record.provenance.clone())
            .unwrap_or_else(|| {
                BTreeMap::from([("created_by".to_owned(), "workcell connect".to_owned())])
            }),
    };
    store_connection_record(&global.state_root, record, &label)?;

    // 8. Report.
    if global.json {
        emit_json(json!({
            "ok": true,
            "connection": {
                "label": label,
                "endpoint": endpoint,
                "state": "connected",
                "protocol": compatibility.server_protocol,
                "remote_workcell_ref": remote_workcell_ref,
                "granted_operations": granted_operations,
                "expires_at_unix_ms": expires_at_unix_ms,
                "advertised_ports": advertised_ports,
                "advertised_offers": advertised_offers,
                "credential_ref": credential_ref,
            },
            "compatibility": {
                "client_protocol": compatibility.client_protocol,
                "server_protocol": compatibility.server_protocol,
                "client_software": compatibility.client_software,
                "server_software": compatibility.server_software,
            },
            "notes": notes,
        }));
    } else {
        println!(
            "{} `{label}` -> {endpoint}",
            if existing.is_some() { "reconnected" } else { "connected" },
        );
        println!(
            "compatibility: protocol {} on both cells",
            compatibility.server_protocol
        );
        println!(
            "software: local `{}` / remote `{}`",
            compatibility.client_software, compatibility.server_software
        );
        if let Some(workcell_ref) = &remote_workcell_ref {
            println!("remote workcell: {workcell_ref}");
        }
        println!("granted operations: {}", granted_operations.join(", "));
        match expires_at_unix_ms {
            Some(expires_at) => println!("grant expires: {expires_at} (unix ms)"),
            None => println!("grant expires: never"),
        }
        if granted_operations.iter().any(|operation| operation == "discover") {
            println!(
                "advertised capabilities: {advertised_offers} offer(s) across ports: {}",
                if advertised_ports.is_empty() {
                    "none".to_owned()
                } else {
                    advertised_ports.join(", ")
                }
            );
        } else {
            println!("advertised capabilities: discovery is not granted by this connection");
        }
        println!(
            "execution location: operations through this connection run on {} at {endpoint}, not on this machine",
            remote_workcell_ref.as_deref().unwrap_or("the remote cell"),
        );
        for note in &notes {
            println!("note: {note}");
        }
    }
    Ok(())
}

fn command_connections(global: &GlobalArgs, args: &[String]) -> Result<(), WorkcellError> {
    let Some(subcommand) = args.first().map(String::as_str) else {
        return Err(WorkcellError::InvalidDemand(
            "usage: workcell connections <list|show|disconnect> [label]".into(),
        ));
    };
    match subcommand {
        "list" => {
            let records = list_connection_records(&global.state_root)?;
            if global.json {
                emit_json(json!({
                    "ok": true,
                    "connections": records.iter().map(connection_status_json).collect::<Vec<_>>(),
                }));
            } else {
                if records.is_empty() {
                    println!("no cross-cell connections recorded");
                }
                for record in &records {
                    println!(
                        "{} [{}] -> {} ({}){}",
                        record.label,
                        record.state,
                        record.endpoint,
                        record.remote_workcell_ref.as_deref().unwrap_or("remote identity unknown"),
                        expiry_note(record.expires_at_unix_ms),
                    );
                }
            }
            Ok(())
        }
        "show" => {
            let Some(label) = args.get(1) else {
                return Err(WorkcellError::InvalidDemand(
                    "usage: workcell connections show <label>".into(),
                ));
            };
            let record = load_connection_record(&global.state_root, label)?.ok_or_else(|| {
                WorkcellError::NotFound(format!("no connection record named `{label}`"))
            })?;
            if global.json {
                emit_json(json!({ "ok": true, "connection": connection_value(&record)? }));
            } else {
                println!("{}", encode_connection(&record)?);
            }
            Ok(())
        }
        "disconnect" => {
            let Some(label) = args.get(1) else {
                return Err(WorkcellError::InvalidDemand(
                    "usage: workcell connections disconnect <label>".into(),
                ));
            };
            let existing = load_connection_record(&global.state_root, label)?.ok_or_else(|| {
                WorkcellError::NotFound(format!("no connection record named `{label}`"))
            })?;
            if existing.state != "connected" {
                if global.json {
                    emit_json(json!({
                        "ok": true,
                        "label": label,
                        "state": existing.state,
                        "changed": false,
                    }));
                } else {
                    println!("`{label}` is `{}`, not connected", existing.state);
                }
                return Ok(());
            }
            let record = reconciled_record(&Some(existing), |record| {
                record.state = "disconnected".to_owned();
                record.detail = Some("disconnected by operator".to_owned());
                record.last_reconciled_at_unix_ms = now_unix_ms();
            });
            store_connection_record(&global.state_root, record, label)?;
            if global.json {
                emit_json(json!({ "ok": true, "label": label, "state": "disconnected", "changed": true }));
            } else {
                println!("disconnected `{label}`; the connection record is kept");
            }
            Ok(())
        }
        other => Err(WorkcellError::InvalidDemand(format!(
            "unknown connections subcommand `{other}`; expected list|show|disconnect"
        ))),
    }
}

/// Apply reconciliation changes to an existing record, or start a fresh one
/// when no record exists yet.
fn reconciled_record(
    existing: &Option<ConnectionRecord>,
    apply: impl FnOnce(&mut ConnectionRecord),
) -> ConnectionRecord {
    let mut record = existing
        .clone()
        .unwrap_or_else(|| ConnectionRecord {
            connection_ref: String::new(),
            label: String::new(),
            endpoint: String::new(),
            protocol: CONTROL_PROTOCOL_VERSION.to_owned(),
            remote_workcell_ref: None,
            remote_software: None,
            local_software: software_version(),
            granted_operations: Vec::new(),
            credential_ref: None,
            state: "disconnected".to_owned(),
            detail: None,
            connected_at_unix_ms: None,
            expires_at_unix_ms: None,
            last_reconciled_at_unix_ms: now_unix_ms(),
            provenance: BTreeMap::from([(
                "created_by".to_owned(),
                "workcell connect".to_owned(),
            )]),
        });
    apply(&mut record);
    record.connection_ref = format!("connection:{}", record.label);
    record.local_software = software_version();
    record.last_reconciled_at_unix_ms = now_unix_ms();
    record
}

fn connection_record_path(state_root: &Path, label: &str) -> PathBuf {
    state_root.join("connections").join(format!("{label}.json"))
}

fn load_connection_record(
    state_root: &Path,
    label: &str,
) -> Result<Option<ConnectionRecord>, WorkcellError> {
    let path = connection_record_path(state_root, label);
    if !path.exists() {
        return Ok(None);
    }
    let encoded = fs::read_to_string(&path).map_err(|error| {
        WorkcellError::NotFound(format!(
            "read connection record `{}`: {error}",
            path.display()
        ))
    })?;
    decode_connection(&encoded)
        .map(Some)
        .map_err(|error| WorkcellError::OperationFailed(format!(
            "connection record `{}` is invalid: {error}",
            path.display()
        )))
}

fn store_connection_record(
    state_root: &Path,
    record: ConnectionRecord,
    label: &str,
) -> Result<(), WorkcellError> {
    let path = connection_record_path(state_root, label);
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|error| {
            WorkcellError::OperationFailed(format!(
                "create connections directory `{}`: {error}",
                parent.display()
            ))
        })?;
    }
    fs::write(&path, encode_connection(&record)?).map_err(|error| {
        WorkcellError::OperationFailed(format!(
            "write connection record `{}`: {error}",
            path.display()
        ))
    })
}

fn list_connection_records(state_root: &Path) -> Result<Vec<ConnectionRecord>, WorkcellError> {
    let directory = state_root.join("connections");
    if !directory.exists() {
        return Ok(Vec::new());
    }
    let mut records = Vec::new();
    let entries = fs::read_dir(&directory).map_err(|error| {
        WorkcellError::OperationFailed(format!("read connections directory: {error}"))
    })?;
    for entry in entries {
        let path = entry
            .map_err(|error| {
                WorkcellError::OperationFailed(format!("read connections entry: {error}"))
            })?
            .path();
        if path.extension().and_then(|value| value.to_str()) != Some("json") {
            continue;
        }
        if path.file_name().and_then(|value| value.to_str()) == Some(GRANTS_FILE) {
            continue;
        }
        let encoded = fs::read_to_string(&path).map_err(|error| {
            WorkcellError::OperationFailed(format!(
                "read connection record `{}`: {error}",
                path.display()
            ))
        })?;
        let record = decode_connection(&encoded).map_err(|error| {
            WorkcellError::OperationFailed(format!(
                "connection record `{}` is invalid: {error}",
                path.display()
            ))
        })?;
        records.push(record);
    }
    records.sort_by(|left, right| left.label.cmp(&right.label));
    Ok(records)
}

/// Credential resolution order for `connect`: explicit `--authorization`
/// wins, then the environment, then the keychain location recorded on the
/// connection receipt. Only locations are recorded, never material.
fn resolve_connection_credential(
    explicit: Option<&str>,
    record_credential_ref: Option<&str>,
) -> Result<Option<String>, WorkcellError> {
    if let Some(value) = explicit.filter(|value| !value.is_empty()) {
        return Ok(Some(value.to_owned()));
    }
    if let Ok(value) = env::var("WORKCELL_CONTROL_TOKEN") {
        if !value.is_empty() {
            return Ok(Some(value));
        }
    }
    if let Some(reference) = record_credential_ref {
        use epilogos_workcell_core::SecretProvider;
        let external = ExternalRef::new(reference).map_err(WorkcellError::from)?;
        // Dispatch on the recorded ref's scheme, so each platform resolves
        // from its own origin store and older keychain refs keep working.
        let material = if reference.starts_with("linux-secret-service://") {
            SecretServiceSecretProvider::new()?.resolve(&external)?
        } else {
            KeychainSecretProvider::new(KeychainAclPolicy::ThisDeviceUnlocked)?
                .resolve(&external)?
        };
        return Ok(Some(material.value.expose_for_materialisation().to_owned()));
    }
    Ok(None)
}

/// Keep the connection credential in this cell's origin secret source under
/// a stable location: the macOS Keychain on macOS, the Linux Secret Service
/// on Linux. One store law on every platform — material never lands in a
/// config or state file, and the ref names the source it lives in.
#[cfg(not(target_os = "linux"))]
fn store_connection_credential(label: &str, credential: &str) -> Result<String, WorkcellError> {
    let reference = format!("keychain://workcell-connection/{label}");
    let external = ExternalRef::new(&reference).map_err(WorkcellError::from)?;
    let provider = KeychainSecretProvider::new(KeychainAclPolicy::ThisDeviceUnlocked)?;
    store_keychain_material(&provider, &external, credential.as_bytes())?;
    Ok(reference)
}

#[cfg(target_os = "linux")]
fn store_connection_credential(label: &str, credential: &str) -> Result<String, WorkcellError> {
    let reference = format!("linux-secret-service://workcell-connection/{label}");
    let external = ExternalRef::new(&reference).map_err(WorkcellError::from)?;
    let provider = SecretServiceSecretProvider::new()?;
    store_secret_service_material(&provider, &external, credential.as_bytes())?;
    Ok(reference)
}

fn control_client_error(error: ControlClientError) -> Box<WorkcellError> {
    match error {
        ControlClientError::TransportUnavailable(message) => {
            Box::new(WorkcellError::Unavailable(message))
        }
        ControlClientError::ProtocolIncompatible(message) => {
            Box::new(WorkcellError::Unsupported(message))
        }
        ControlClientError::AuthenticationFailed(message) => {
            Box::new(WorkcellError::Unavailable(message))
        }
        ControlClientError::Remote(error) => Box::new(error),
        ControlClientError::InvalidResponse(message) => {
            Box::new(WorkcellError::OperationFailed(message))
        }
    }
}

fn is_loopback_listener(address: &str) -> bool {
    address.starts_with("127.")
        || address.starts_with("localhost:")
        || address.starts_with("[::1]:")
}

fn now_unix_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_millis() as u64)
        .unwrap_or(0)
}

fn system_now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_millis() as u64)
        .unwrap_or(0)
}

fn system_prov(owner_ref: &str, path: &str, observed_at: u64) -> Value {
    json!({
        "owner_ref": owner_ref,
        "path": path,
        "observed_at_unix_ms": observed_at,
    })
}

fn system_axes(
    owner_ref: &str,
    path: &str,
    observed_at: u64,
    declared: Value,
    effective: Value,
    active: Value,
    materialisation_ref: Option<&str>,
) -> Value {
    let active_axis = match materialisation_ref {
        Some(reference) => json!({
            "value": active,
            "provenance": system_prov(owner_ref, path, observed_at),
            "materialisation_ref": reference,
        }),
        None => json!({
            "value": active,
            "provenance": system_prov(owner_ref, path, observed_at),
        }),
    };
    json!({
        "declared": {
            "value": declared,
            "provenance": system_prov(owner_ref, path, observed_at),
        },
        "effective": {
            "value": effective,
            "provenance": system_prov(owner_ref, path, observed_at),
        },
        "active": active_axis,
        "staged": {
            "value": {},
            "provenance": system_prov(owner_ref, path, observed_at),
            "stage_ref": null,
            "stage_state": "none",
        },
        "expected_effect": { "summary": null, "ref": null },
    })
}

#[allow(clippy::too_many_arguments)]
fn system_setting(
    key: &str,
    title: &str,
    kind: &str,
    owner_ref: &str,
    path: &str,
    observed_at: u64,
    declared: Value,
    effective: Value,
    active: Value,
    materialisation_ref: Option<&str>,
    mutable: bool,
    native_path: Option<&str>,
    bootstrap: bool,
    drift_state: &str,
    drift_between: &[&str],
    remediation: Option<&str>,
) -> Value {
    json!({
        "key": key,
        "title": title,
        "kind": kind,
        "axes": system_axes(
            owner_ref, path, observed_at, declared, effective, active, materialisation_ref
        ),
        "mutable": mutable,
        "native_path": native_path,
        "bootstrap": bootstrap,
        "drift": {
            "state": drift_state,
            "between": drift_between,
            "remediation_action_ref": remediation,
        },
    })
}

fn system_unavailable(reason: &str) -> Value {
    json!({ "state": "unavailable", "reason": reason })
}

/// One collapsed-local faculty whose availability is observed, not asserted.
/// The same reading feeds both the section axis values and the degradation list,
/// so a faculty that later lands changes both without a second edit.
struct FacultyDetection {
    subject_ref: &'static str,
    state: &'static str,
    reason: &'static str,
}

/// Single named faculty-detection point for the four faculties collapsed-local
/// does not implement today. `fabric.provider` and `fabric.tailscale` are
/// probe-gated on the discovery inventory (a later-registered fabric/Tailscale
/// provider flips them to available with no code change here);
/// `hardware.accelerator` and `hardware.host_enumeration` are structurally
/// absent — collapsed-local has no such observation faculty — and their absence
/// is named once, here, so the day either faculty ships the reading changes in
/// this one place rather than across four scattered literals.
fn detect_faculties(discovery: &Discovery) -> Vec<FacultyDetection> {
    let has_fabric_provider = discovery.offers.iter().any(|offer| {
        offer.provider_ref.as_str().contains("fabric") || offer.port.as_str().contains("fabric")
    });
    let has_tailscale_provider = discovery
        .offers
        .iter()
        .any(|offer| offer.provider_ref.as_str().contains("tailscale"));

    let mut detections = Vec::new();
    if !has_fabric_provider {
        detections.push(FacultyDetection {
            subject_ref: "fabric.provider",
            state: "unavailable",
            reason: "no Fabric/network provider is registered; host-process execution does not claim or enforce cross-host network reachability",
        });
    }
    if !has_tailscale_provider {
        detections.push(FacultyDetection {
            subject_ref: "fabric.tailscale",
            state: "unavailable",
            reason: "no Tailscale provider is registered and no tailnet is detected",
        });
    }
    detections.push(FacultyDetection {
        subject_ref: "hardware.accelerator",
        state: "unavailable",
        reason: "no accelerator-observation faculty exists in collapsed-local; a GPU is never invented",
    });
    detections.push(FacultyDetection {
        subject_ref: "hardware.host_enumeration",
        state: "unavailable",
        reason: "no host hardware enumeration faculty exists at the Workcell level",
    });
    detections
}

/// Axis value for a detected faculty: an honest unavailable reading when the
/// faculty is absent, and a plain available reading once it has landed.
fn faculty_axis_value(detections: &[FacultyDetection], subject_ref: &str) -> Value {
    match detections
        .iter()
        .find(|detection| detection.subject_ref == subject_ref)
    {
        Some(detection) => system_unavailable(detection.reason),
        None => json!({ "state": "available" }),
    }
}

fn system_hex_digest(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

/// Zero every `*_unix_ms` field in the descriptor, recursively, so the canonical
/// reading body is independent of when the reading was taken (§4.5). The live
/// descriptor keeps its real timestamps; only the hashed body is zeroed.
fn zero_unix_ms(value: &mut Value) {
    match value {
        Value::Object(map) => {
            let keys: Vec<String> = map
                .keys()
                .filter(|key| key.ends_with("_unix_ms"))
                .cloned()
                .collect();
            for key in keys {
                map.insert(key, json!(0u64));
            }
            for (_, child) in map.iter_mut() {
                zero_unix_ms(child);
            }
        }
        Value::Array(items) => {
            for item in items.iter_mut() {
                zero_unix_ms(item);
            }
        }
        _ => {}
    }
}

fn system_action(
    action_ref: &str,
    title: &str,
    args: Vec<Value>,
    subject_kinds: Vec<&str>,
    availability: &str,
    explain: &[&str],
    history: &[&str],
) -> Value {
    json!({
        "action_ref": action_ref,
        "title": title,
        "args": args,
        "availability": availability,
        "unavailable_reason": null,
        "subject_kinds": subject_kinds,
        "authority": {
            "requires": [],
            "granted_by": "local-user-or-agent",
            "evidence_ref": null,
        },
        "exposure": { "ui": false, "agent": true, "headless": true },
        "explain": { "ref": action_ref, "command": explain },
        "history": { "ref": action_ref, "command": history },
    })
}

/// Emit this Workcell's System settings disclosure (`oi.product-settings-disclosure/v2`).
///
/// The reading is assembled from the collapsed-local operational domain: `discover()`
/// (providers/offers/capabilities), the instance registry and a read-only live scan
/// (instances/processes), the state-root service declarations (services), artifact
/// channels (storage), and the fixed composition facts (fabric/hardware/model-serving
/// faculties). Logical Workcell identity is never collapsed into a provider, process,
/// or material binding: every axis carries its own value and owner-namespace provenance.
///
/// The canonical reading body (over which `owner.reading_digest` is computed) is
/// the descriptor with every `*_unix_ms` field zeroed — `disclosed_at_unix_ms`,
/// `owner.observed_at_unix_ms` and every `axes.*.provenance.observed_at_unix_ms` —
/// and `owner.reading_digest` set to null (§4.5). serde_json default map ordering
/// (sorted keys) makes the serialization deterministic, and because no live clock
/// value survives in the hashed body, two readings of an unchanged world produce
/// the same digest: a changed digest means a changed reading, never a changed clock.
fn command_system(global: &GlobalArgs) -> Result<(), WorkcellError> {
    let descriptor = system_descriptor(global)?;
    if global.json {
        emit_json(descriptor);
    } else {
        let availability = descriptor["availability"]["state"].as_str().unwrap_or("unknown");
        println!(
            "Workcell System disclosure (oi.product-settings-disclosure/v2)\n  product: workcell\n  availability: {availability}\n  sections: {}\n  actions: {}\n  degradations: {}\n  obligations: {}\n  reading digest: {}",
            descriptor["sections"].as_array().map_or(0, Vec::len),
            descriptor["actions"].as_array().map_or(0, Vec::len),
            descriptor["degradations"].as_array().map_or(0, Vec::len),
            descriptor["obligations"].as_array().map_or(0, Vec::len),
            descriptor["owner"]["reading_digest"].as_str().unwrap_or("null"),
        );
    }
    Ok(())
}

/// Assemble this Workcell's `oi.product-settings-disclosure/v2` descriptor.
/// Both the local `workcell system` face and the serving Control Service's
/// `system` operation read through this one function, so a disclosure served
/// over the control endpoint is byte-for-byte the reading the host itself
/// would print.
/// The descriptor for a caller-supplied state root — the shape the combined
/// CLI's remote selector needs when merging a remote reading beside the
/// local one.
pub(super) fn system_descriptor_from(
    json: bool,
    state_root: &Path,
    workspace_source: Option<PathBuf>,
    services: Option<PathBuf>,
) -> Result<Value, WorkcellError> {
    system_descriptor(&GlobalArgs {
        json,
        state_root: state_root.to_path_buf(),
        workcell_ref: DEFAULT_WORKCELL_REF.to_owned(),
        receipt: None,
        workspace_source,
        services,
        remaining: Vec::new(),
    })
}

fn system_descriptor(global: &GlobalArgs) -> Result<Value, WorkcellError> {
    use epilogos_workcell_runtime::{
        scan_inputs_live, InstanceRegistry, AIKIT_GATEWAY_APPLICATION_PROTOCOL,
        AIKIT_GATEWAY_SOURCE_REVISION, HERMES_SOURCE_REVISION, OPENCLAW_SOURCE_REVISION,
    };
    use sha2::{Digest, Sha256};

    let workcell_ref = parse_workcell_ref(&global.workcell_ref)?;
    let workcell = new_local(global, workcell_ref.clone(), BTreeSet::new())?;
    let discovery = workcell.discover()?;

    let now = system_now_ms();
    let owner_ref = discovery.workcell_ref.as_str().to_owned();
    let detections = detect_faculties(&discovery);
    let discovery_path = format!("{owner_ref}:discovery");
    let registry_path = format!("{owner_ref}:instances:registry");
    let scan_path = format!("{owner_ref}:instances:scan");
    let reference_path = format!("{owner_ref}:reference-services");

    // Provider inventory, offers, capabilities and aggregate capacity.
    let mut providers_by_ref: BTreeMap<
        String,
        Vec<&epilogos_workcell_core::OperationalOffer>,
    > = BTreeMap::new();
    for offer in &discovery.offers {
        providers_by_ref
            .entry(offer.provider_ref.to_string())
            .or_default()
            .push(offer);
    }
    let declared_providers: Vec<Value> = [
        "provider:collapsed-local-workspace",
        "provider:collapsed-local-host-process",
        "provider:collapsed-local-artifacts",
        "provider:collapsed-local-managed-services",
        "provider:collapsed-local-target-services",
    ]
    .iter()
    .map(|provider| json!({ "provider_ref": provider }))
    .collect();
    let inventory_effective: Vec<Value> = providers_by_ref
        .iter()
        .map(|(provider, offers)| {
            let ports = offers
                .iter()
                .map(|offer| offer.port.as_str())
                .collect::<BTreeSet<_>>();
            json!({
                "provider_ref": provider,
                "ports": ports,
                "offers": offers.len(),
            })
        })
        .collect();
    let inventory_active: Vec<Value> = providers_by_ref
        .iter()
        .map(|(provider, offers)| {
            let available = offers
                .iter()
                .filter(|offer| offer.availability == Availability::Available)
                .count();
            json!({ "provider_ref": provider, "available_offers": available })
        })
        .collect();
    let offers_effective: Vec<Value> = discovery
        .offers
        .iter()
        .map(|offer| {
            json!({
                "offer_ref": offer.offer_ref.as_str(),
                "provider_ref": offer.provider_ref.as_str(),
                "port": offer.port,
                "affordances": offer.affordances,
                "connections": offer.connections,
                "exposures": offer.exposures,
                "isolation_trust": offer.isolation_trust,
                "availability": availability(&offer.availability),
                "health": health(&offer.health),
            })
        })
        .collect();
    let offers_active: Vec<Value> = discovery
        .offers
        .iter()
        .filter(|offer| offer.availability == Availability::Available)
        .map(|offer| {
            json!({
                "offer_ref": offer.offer_ref.as_str(),
                "provider_ref": offer.provider_ref.as_str(),
                "port": offer.port,
            })
        })
        .collect();
    let mut capabilities: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
    for offer in &discovery.offers {
        for affordance in &offer.affordances {
            capabilities
                .entry(affordance.clone())
                .or_default()
                .insert(offer.provider_ref.to_string());
        }
    }
    let capabilities_effective: Vec<Value> = capabilities
        .iter()
        .map(|(affordance, providers)| json!({ "affordance": affordance, "providers": providers }))
        .collect();
    let aggregate_capacity: Vec<Value> = discovery
        .capacity
        .iter()
        .map(|(key, value)| json!({ "key": key, "amount": value.amount, "unit": value.unit }))
        .collect();

    // Instances: registry (read-only) and live scan inputs (read-only).
    let registry = InstanceRegistry::new(&global.state_root, workcell_ref.clone());
    let registry_records = registry.list().unwrap_or_default();
    let declared_instances: Vec<Value> = registry_records
        .iter()
        .filter(|record| {
            record.get("evidence_grade").and_then(Value::as_str) == Some("declared-unverified")
        })
        .cloned()
        .collect();
    let live_instances: Vec<Value> = registry_records
        .iter()
        .filter(|record| record.get("liveness").and_then(Value::as_str) == Some("live"))
        .cloned()
        .collect();
    let scan_inputs = scan_inputs_live("actuation");
    let (scan_effective, scan_active) = match &scan_inputs {
        Ok(inputs) => (
            json!({ "state": "supplied", "faculty": "actuation detection + pid table + gateway seam" }),
            json!({
                "live_processes": inputs.processes.len(),
                "gateway_answering": inputs.gateway_answering,
            }),
        ),
        Err(reason) => (
            system_unavailable(reason),
            system_unavailable("live scan inputs could not be gathered"),
        ),
    };

    // Services.
    let service_offers: Vec<&epilogos_workcell_core::OperationalOffer> = discovery
        .offers
        .iter()
        .filter(|offer| offer.port == ProviderPortKind::Service.as_str())
        .collect();
    let services_declared: Vec<Value> = service_offers
        .iter()
        .map(|offer| {
            json!({
                "logical_ref": offer.connections.first().map(String::as_str).unwrap_or(""),
                "provider_ref": offer.provider_ref.as_str(),
                "availability": availability(&offer.availability),
                "health": health(&offer.health),
                "lifetime": offer.metadata.get("lifetime").map(String::as_str).unwrap_or("unknown"),
                "endpoint": offer.metadata.get("endpoint").map(String::as_str),
            })
        })
        .collect();
    let services_active: Vec<Value> = service_offers
        .iter()
        .filter(|offer| offer.health == HealthState::Healthy)
        .map(|offer| {
            json!({
                "logical_ref": offer.connections.first().map(String::as_str).unwrap_or(""),
                "provider_ref": offer.provider_ref.as_str(),
            })
        })
        .collect();

    // Storage / artifact channels.
    let artifact_channels: Vec<Value> = discovery
        .offers
        .iter()
        .filter(|offer| offer.port == ProviderPortKind::ArtifactStorage.as_str())
        .map(|offer| {
            json!({
                "channel": offer.metadata.get("logical_channel").cloned(),
                "provider_ref": offer.provider_ref.as_str(),
            })
        })
        .collect();

    // Local / remote presence.
    let remote_endpoint = env::var("WORKCELL_CONTROL_ENDPOINT").ok();
    let remote_presence = match &remote_endpoint {
        Some(endpoint) => json!({ "state": "available", "endpoint": endpoint }),
        None => system_unavailable(
            "no WORKCELL_CONTROL_ENDPOINT is set; no remote Workcell is configured",
        ),
    };

    // Model-serving materialisation: reference services (declared capability) and
    // any currently-declared model-serving service (active).
    let model_targets = ["aikit-gateway", "hermes", "openclaw", "ollama", "llama.cpp", "vllm"];
    let model_serving_active: Vec<Value> = service_offers
        .iter()
        .filter(|offer| {
            let target = offer.metadata.get("target").map(String::as_str);
            let program = offer.metadata.get("program").map(String::as_str);
            target.is_some_and(|t| model_targets.contains(&t))
                || program.is_some_and(|p| model_targets.iter().any(|t| p.starts_with(*t)))
        })
        .map(|offer| {
            json!({
                "logical_ref": offer.connections.first().map(String::as_str).unwrap_or(""),
                "target": offer.metadata.get("target"),
                "program": offer.metadata.get("program"),
            })
        })
        .collect();

    // Drift on the two settings that have a distinct authored value.
    let identity_drift = if global.workcell_ref.as_str() == discovery.workcell_ref.as_str() {
        "none"
    } else {
        "diverged"
    };
    let state_root_declared = default_state_root().display().to_string();
    let state_root_effective = global.state_root.display().to_string();
    let state_root_drift = if state_root_declared == state_root_effective {
        "none"
    } else {
        "diverged"
    };

    let sections = vec![
        json!({
            "id": "workcells",
            "title": "Workcells / instances",
            "settings": [
                system_setting(
                    "workcells.current", "Workcell identity", "scalar",
                    &owner_ref, &discovery_path, now,
                    json!({ "workcell_ref": global.workcell_ref }),
                    json!({ "workcell_ref": discovery.workcell_ref.as_str() }),
                    json!({ "workcell_ref": discovery.workcell_ref.as_str() }),
                    None, false, Some("workcell --workcell-ref REF"), true,
                    identity_drift, &["declared", "effective"], None,
                ),
                system_setting(
                    "workcells.instances", "Registered harness instances", "table",
                    &owner_ref, &registry_path, now,
                    json!({ "records": declared_instances }),
                    json!({ "records": registry_records }),
                    json!({ "records": live_instances }),
                    Some(&registry_path), false,
                    Some("workcell instances register|declare"), false,
                    "none", &["declared", "effective"], None,
                ),
                system_setting(
                    "workcells.live_scan", "Live instance scan", "presence",
                    &owner_ref, &scan_path, now,
                    json!({ "state": "available", "faculty": "actuation detection + pid table + gateway seam" }),
                    scan_effective, scan_active,
                    None, false, Some("workcell instances scan"), false,
                    "none", &["declared", "effective"], None,
                ),
            ],
        }),
        json!({
            "id": "providers",
            "title": "Providers / offers / capabilities",
            "settings": [
                system_setting(
                    "providers.inventory", "Provider inventory", "table",
                    &owner_ref, &discovery_path, now,
                    json!({ "composition": "collapsed-local", "providers": declared_providers }),
                    json!({ "providers": inventory_effective }),
                    json!({ "providers": inventory_active }),
                    None, false, Some("workcell providers"), false,
                    "none", &["declared", "effective"], None,
                ),
                system_setting(
                    "providers.offers", "Operational offers", "table",
                    &owner_ref, &discovery_path, now,
                    json!({ "offers": offers_effective }),
                    json!({ "offers": offers_effective }),
                    json!({ "offers": offers_active }),
                    None, false, Some("workcell discover"), false,
                    "none", &["declared", "effective"], None,
                ),
                system_setting(
                    "providers.capabilities", "Material capabilities (affordances)", "table",
                    &owner_ref, &discovery_path, now,
                    json!({ "capabilities": capabilities_effective }),
                    json!({ "capabilities": capabilities_effective }),
                    json!({ "capabilities": capabilities_effective }),
                    None, false, Some("workcell discover"), false,
                    "none", &["declared", "effective"], None,
                ),
                system_setting(
                    "providers.capacity", "Workcell-wide aggregate capacity", "table",
                    &owner_ref, &discovery_path, now,
                    json!({ "capacity": aggregate_capacity }),
                    json!({ "capacity": aggregate_capacity }),
                    json!({ "capacity": aggregate_capacity }),
                    None, false, Some("workcell discover"), false,
                    "none", &["declared", "effective"], None,
                ),
            ],
        }),
        json!({
            "id": "processes-services",
            "title": "Processes / services",
            "settings": [
                system_setting(
                    "services.declared", "Declared logical services", "table",
                    &owner_ref, &discovery_path, now,
                    json!({ "services": services_declared }),
                    json!({ "services": services_declared }),
                    json!({ "services": services_active }),
                    None, false, Some("workcell --services PATH"), false,
                    "none", &["declared", "effective"], None,
                ),
                system_setting(
                    "processes.execution", "Host-process execution", "presence",
                    &owner_ref, &discovery_path, now,
                    json!({ "state": "available", "provider": "provider:collapsed-local-host-process" }),
                    json!({ "state": "available", "affordances": ["shell", "process-execution", "execution:host-process"] }),
                    json!({ "state": "available", "authority": "explicit material operation grant required" }),
                    None, false, Some("workcell plan|prepare --require shell"), false,
                    "none", &["declared", "effective"], None,
                ),
            ],
        }),
        json!({
            "id": "storage",
            "title": "Storage / artifacts",
            "settings": [
                system_setting(
                    "storage.state_root", "Workcell state root", "scalar",
                    &owner_ref, &format!("{owner_ref}:env"), now,
                    json!({ "path": state_root_declared }),
                    json!({ "path": state_root_effective }),
                    json!({ "path": state_root_effective }),
                    None, false, Some("workcell --state-root PATH"), true,
                    state_root_drift, &["declared", "effective"], None,
                ),
                system_setting(
                    "storage.artifact_channels", "Artifact channels", "table",
                    &owner_ref, &discovery_path, now,
                    json!({ "channels": artifact_channels }),
                    json!({ "channels": artifact_channels }),
                    json!({ "channels": artifact_channels }),
                    None, false, Some("workcell prepare --output CHANNEL"), false,
                    "none", &["declared", "effective"], None,
                ),
            ],
        }),
        json!({
            "id": "fabric",
            "title": "Fabric / reachability",
            "settings": [
                system_setting(
                    "fabric.local_placement", "Same-host (loopback) placement", "presence",
                    &owner_ref, &discovery_path, now,
                    json!({ "state": "available" }),
                    json!({ "state": "available", "note": "collapsed-local materialises execution and storage on the same host" }),
                    json!({ "state": "available" }),
                    None, false, None, false, "none", &["declared", "effective"], None,
                ),
                system_setting(
                    "fabric.provider", "Fabric / network provider", "presence",
                    &owner_ref, &discovery_path, now,
                    faculty_axis_value(&detections, "fabric.provider"),
                    faculty_axis_value(&detections, "fabric.provider"),
                    faculty_axis_value(&detections, "fabric.provider"),
                    None, false, None, false, "none", &["declared", "effective"], None,
                ),
                system_setting(
                    "fabric.tailscale", "Tailscale / tailnet", "presence",
                    &owner_ref, &discovery_path, now,
                    faculty_axis_value(&detections, "fabric.tailscale"),
                    faculty_axis_value(&detections, "fabric.tailscale"),
                    faculty_axis_value(&detections, "fabric.tailscale"),
                    None, false, None, false, "none", &["declared", "effective"], None,
                ),
                system_setting(
                    "fabric.remote_control", "Remote control endpoint", "presence",
                    &owner_ref, &format!("{owner_ref}:env"), now,
                    remote_presence.clone(), remote_presence.clone(), remote_presence.clone(),
                    None, false, Some("workcell --endpoint HOST:PORT"), false,
                    "none", &["declared", "effective"], None,
                ),
            ],
        }),
        json!({
            "id": "local-remote",
            "title": "Local / remote state",
            "settings": [
                system_setting(
                    "placement.backend", "Control backend", "scalar",
                    &owner_ref, &discovery_path, now,
                    json!({ "backend": "native-cli" }),
                    json!({ "backend": "native-cli" }),
                    json!({ "backend": "native-cli" }),
                    None, false, None, false, "none", &["declared", "effective"], None,
                ),
                system_setting(
                    "placement.local", "Local operational domain", "presence",
                    &owner_ref, &discovery_path, now,
                    json!({ "state": "available" }),
                    json!({ "state": "available" }),
                    json!({ "state": "available" }),
                    None, false, None, false, "none", &["declared", "effective"], None,
                ),
                system_setting(
                    "placement.remote", "Remote Workcell", "presence",
                    &owner_ref, &format!("{owner_ref}:env"), now,
                    remote_presence.clone(), remote_presence.clone(), remote_presence.clone(),
                    None, false, Some("workcell --endpoint HOST:PORT"), false,
                    "none", &["declared", "effective"], None,
                ),
            ],
        }),
        json!({
            "id": "model-serving",
            "title": "Model-serving materialisation",
            "settings": [
                system_setting(
                    "model-serving.reference_services", "Reference service materialisation", "table",
                    &owner_ref, &reference_path, now,
                    json!({ "services": [
                        { "target": "aikit-gateway", "shape": "managed host service (serve --ws)", "source_revision": AIKIT_GATEWAY_SOURCE_REVISION, "application_protocol": AIKIT_GATEWAY_APPLICATION_PROTOCOL },
                        { "target": "hermes", "shape": "target-owned external service (gateway start/stop/restart/status)", "source_revision": HERMES_SOURCE_REVISION },
                        { "target": "openclaw", "shape": "target-owned external service (gateway start/stop/health/status)", "source_revision": OPENCLAW_SOURCE_REVISION },
                    ] }),
                    json!({ "services": [
                        { "target": "aikit-gateway", "shape": "managed host service (serve --ws)", "source_revision": AIKIT_GATEWAY_SOURCE_REVISION, "application_protocol": AIKIT_GATEWAY_APPLICATION_PROTOCOL },
                        { "target": "hermes", "shape": "target-owned external service (gateway start/stop/restart/status)", "source_revision": HERMES_SOURCE_REVISION },
                        { "target": "openclaw", "shape": "target-owned external service (gateway start/stop/health/status)", "source_revision": OPENCLAW_SOURCE_REVISION },
                    ] }),
                    json!({ "services": model_serving_active }),
                    None, false, Some("workcell --services PATH"), false,
                    "none", &["declared", "effective"], None,
                ),
                system_setting(
                    "model-serving.engine_shapes", "Engine materialisation shapes", "table",
                    &owner_ref, &reference_path, now,
                    json!({ "engines": [
                        { "engine": "ollama", "shape": "ollama serve (managed service); model control separate from inference reachability", "standing": "source-pinned materialisation shape" },
                        { "engine": "llama.cpp", "shape": "llama-cli (one-shot) / llama-server (service)", "standing": "source-pinned materialisation shape" },
                        { "engine": "vllm", "shape": "vllm serve (service); accelerator-gated", "standing": "source-pinned materialisation shape" },
                    ] }),
                    json!({ "engines": [
                        { "engine": "ollama", "shape": "ollama serve (managed service); model control separate from inference reachability", "standing": "source-pinned materialisation shape" },
                        { "engine": "llama.cpp", "shape": "llama-cli (one-shot) / llama-server (service)", "standing": "source-pinned materialisation shape" },
                        { "engine": "vllm", "shape": "vllm serve (service); accelerator-gated", "standing": "source-pinned materialisation shape" },
                    ] }),
                    system_unavailable("no model-serving service is declared on this Workcell"),
                    None, false, None, false, "none", &["declared", "effective"], None,
                ),
            ],
        }),
        json!({
            "id": "hardware",
            "title": "Hardware / accelerator observations",
            "settings": [
                system_setting(
                    "hardware.accelerator", "Accelerator (GPU) observation", "presence",
                    &owner_ref, &discovery_path, now,
                    faculty_axis_value(&detections, "hardware.accelerator"),
                    faculty_axis_value(&detections, "hardware.accelerator"),
                    faculty_axis_value(&detections, "hardware.accelerator"),
                    None, false, None, false, "none", &["declared", "effective"], None,
                ),
                system_setting(
                    "hardware.resource_observation", "Bounded instance resource observation", "presence",
                    &owner_ref, &scan_path, now,
                    json!({ "state": "available", "metrics": ["cpu_time", "cpu_utilisation", "memory_rss"] }),
                    json!({ "state": "available", "metrics": ["cpu_time", "cpu_utilisation", "memory_rss"], "network": "unsupported" }),
                    json!({ "state": "available", "metrics": ["cpu_time", "cpu_utilisation", "memory_rss"], "network": "unsupported" }),
                    None, false, Some("workcell instances usage"), false,
                    "none", &["declared", "effective"], None,
                ),
                system_setting(
                    "hardware.host_enumeration", "Host hardware enumeration", "presence",
                    &owner_ref, &discovery_path, now,
                    faculty_axis_value(&detections, "hardware.host_enumeration"),
                    faculty_axis_value(&detections, "hardware.host_enumeration"),
                    faculty_axis_value(&detections, "hardware.host_enumeration"),
                    None, false, None, false, "none", &["declared", "effective"], None,
                ),
            ],
        }),
        json!({
            "id": "lifecycle",
            "title": "Lifecycle / reconcile / release",
            "settings": [
                system_setting(
                    "lifecycle.operations", "Control-plane operations", "table",
                    &owner_ref, &discovery_path, now,
                    json!({ "operations": [
                        { "operation": "discover", "availability": "available", "native_path": "workcell discover" },
                        { "operation": "plan", "availability": "available", "native_path": "workcell plan" },
                        { "operation": "prepare", "availability": "available", "native_path": "workcell prepare" },
                        { "operation": "observe", "availability": "available", "native_path": "workcell observe" },
                        { "operation": "expose", "availability": "available", "native_path": "workcell expose" },
                        { "operation": "collect", "availability": "available", "native_path": "workcell collect" },
                        { "operation": "release", "availability": "available", "native_path": "workcell release" },
                        { "operation": "reconcile", "availability": "available", "native_path": "workcell reconcile" },
                    ] }),
                    json!({ "operations": [
                        { "operation": "discover", "availability": "available", "native_path": "workcell discover" },
                        { "operation": "plan", "availability": "available", "native_path": "workcell plan" },
                        { "operation": "prepare", "availability": "available", "native_path": "workcell prepare" },
                        { "operation": "observe", "availability": "available", "native_path": "workcell observe" },
                        { "operation": "expose", "availability": "available", "native_path": "workcell expose" },
                        { "operation": "collect", "availability": "available", "native_path": "workcell collect" },
                        { "operation": "release", "availability": "available", "native_path": "workcell release" },
                        { "operation": "reconcile", "availability": "available", "native_path": "workcell reconcile" },
                    ] }),
                    json!({ "operations": [
                        { "operation": "discover", "availability": "available", "native_path": "workcell discover" },
                        { "operation": "plan", "availability": "available", "native_path": "workcell plan" },
                        { "operation": "prepare", "availability": "available", "native_path": "workcell prepare" },
                        { "operation": "observe", "availability": "available", "native_path": "workcell observe" },
                        { "operation": "expose", "availability": "available", "native_path": "workcell expose" },
                        { "operation": "collect", "availability": "available", "native_path": "workcell collect" },
                        { "operation": "release", "availability": "available", "native_path": "workcell release" },
                        { "operation": "reconcile", "availability": "available", "native_path": "workcell reconcile" },
                    ] }),
                    None, false, Some("workcell <discover|plan|prepare|observe|expose|collect|release|reconcile>"), false,
                    "none", &["declared", "effective"], None,
                ),
                system_setting(
                    "lifecycle.persisted_worlds", "Persisted material-world receipts", "scalar",
                    &owner_ref, &format!("{owner_ref}:state-root"), now,
                    json!({ "count": receipt_count(&global.state_root)? }),
                    json!({ "count": receipt_count(&global.state_root)? }),
                    json!({ "count": receipt_count(&global.state_root)? }),
                    None, false, Some("workcell --receipt PATH prepare"), false,
                    "none", &["declared", "effective"], None,
                ),
                system_setting(
                    "lifecycle.release_dispositions", "Release dispositions", "table",
                    &owner_ref, &discovery_path, now,
                    json!({ "dispositions": [
                        { "disposition": "release", "supported": true },
                        { "disposition": "preserve", "supported": true },
                        { "disposition": "suspend", "supported": false, "reason": "host-process execution does not support suspend" },
                        { "disposition": "snapshot", "supported": false, "reason": "host-process execution does not support snapshot" },
                    ] }),
                    json!({ "dispositions": [
                        { "disposition": "release", "supported": true },
                        { "disposition": "preserve", "supported": true },
                        { "disposition": "suspend", "supported": false, "reason": "host-process execution does not support suspend" },
                        { "disposition": "snapshot", "supported": false, "reason": "host-process execution does not support snapshot" },
                    ] }),
                    json!({ "dispositions": [
                        { "disposition": "release", "supported": true },
                        { "disposition": "preserve", "supported": true },
                        { "disposition": "suspend", "supported": false, "reason": "host-process execution does not support suspend" },
                        { "disposition": "snapshot", "supported": false, "reason": "host-process execution does not support snapshot" },
                    ] }),
                    None, false, Some("workcell --receipt PATH release"), false,
                    "none", &["declared", "effective"], None,
                ),
            ],
        }),
    ];

    let actions = vec![
        system_action("workcell.status", "Summarise this Workcell", vec![], vec!["workcell.material"], "disclosed", &["workcell", "status", "--json"], &["workcell", "status", "--json"]),
        system_action("workcell.discover", "Discover material offers", vec![], vec!["workcell.material"], "disclosed", &["workcell", "discover", "--json"], &["workcell", "discover", "--json"]),
        system_action("workcell.providers", "List provider inventory", vec![], vec!["workcell.provider"], "disclosed", &["workcell", "providers", "--json"], &["workcell", "providers", "--json"]),
        system_action("workcell.doctor", "Verify the zero-setup local baseline", vec![], vec!["workcell.material"], "disclosed", &["workcell", "doctor", "--json"], &["workcell", "doctor", "--json"]),
        system_action("workcell.material", "Compose a material reading for a prepared world", vec![json!({"name": "receipt", "kind": "path"})], vec!["workcell.world"], "disclosed", &["workcell", "material", "--json"], &["workcell", "material", "--json"]),
        system_action("workcell.plan", "Plan an ExecutionDemand", vec![json!({"name": "demand", "kind": "string"})], vec!["workcell.plan"], "disclosed", &["workcell", "plan", "--json"], &["workcell", "plan", "--json"]),
        system_action("workcell.prepare", "Prepare a material world", vec![json!({"name": "demand", "kind": "string"})], vec!["workcell.world"], "disclosed", &["workcell", "prepare", "--json"], &["workcell", "prepare", "--json"]),
        system_action("workcell.observe", "Observe a prepared world", vec![json!({"name": "receipt", "kind": "path"})], vec!["workcell.world"], "disclosed", &["workcell", "observe", "--json"], &["workcell", "observe", "--json"]),
        system_action("workcell.expose", "Resolve prepared exposure surfaces", vec![json!({"name": "receipt", "kind": "path"})], vec!["workcell.world"], "disclosed", &["workcell", "expose", "--json"], &["workcell", "expose", "--json"]),
        system_action("workcell.collect", "Collect prepared output channels", vec![json!({"name": "receipt", "kind": "path"})], vec!["workcell.world"], "disclosed", &["workcell", "collect", "--json"], &["workcell", "collect", "--json"]),
        system_action("workcell.release", "Release or preserve a prepared world", vec![json!({"name": "receipt", "kind": "path"})], vec!["workcell.world"], "disclosed", &["workcell", "release", "--json"], &["workcell", "release", "--json"]),
        system_action("workcell.reconcile", "Reconcile desired material state", vec![json!({"name": "desired", "kind": "string"})], vec!["workcell.world"], "disclosed", &["workcell", "reconcile", "--json"], &["workcell", "reconcile", "--json"]),
        system_action("workcell.instances.list", "List registered harness instances", vec![], vec!["workcell.instance"], "disclosed", &["workcell", "instances", "list", "--json"], &["workcell", "instances", "list", "--json"]),
        system_action("workcell.instances.scan", "Scan live harness instances", vec![], vec!["workcell.instance"], "disclosed", &["workcell", "instances", "scan", "--json"], &["workcell", "instances", "scan", "--json"]),
        system_action("workcell.instances.usage", "Observe bounded resource usage of a live instance", vec![json!({"name": "instance_ref", "kind": "string"})], vec!["workcell.instance"], "disclosed", &["workcell", "instances", "usage", "--json"], &["workcell", "instances", "usage", "--json"]),
        system_action("workcell.sandboxes.reconcile", "Reconcile OpenSandbox server-side sandboxes", vec![json!({"name": "server", "kind": "string"})], vec!["workcell.sandbox"], "disclosed", &["workcell", "sandboxes", "reconcile", "--json"], &["workcell", "sandboxes", "reconcile", "--json"]),
        system_action("workcell.intent", "Preview a lifecycle change through the O:I kernel seam", vec![], vec!["workcell.world"], "missing_native_obligation", &["workcell", "intent"], &["workcell", "intent"]),
        system_action("workcell.invoke", "Invoke a lifecycle change through the O:I kernel seam", vec![], vec!["workcell.world"], "missing_native_obligation", &["workcell", "invoke"], &["workcell", "invoke"]),
    ];

    let availability_state = match discovery.health {
        HealthState::Healthy => "available",
        HealthState::Degraded => "degraded",
        HealthState::Unavailable => "unavailable",
        HealthState::Unknown => "unknown",
    };
    let availability_reason = if availability_state == "available" {
        None
    } else {
        Some("collapsed-local discovery reports non-healthy state")
    };

    let mut degradations = detections
        .iter()
        .map(|detection| {
            json!({
                "subject_ref": detection.subject_ref,
                "state": detection.state,
                "reason": detection.reason,
                "native_error": null,
            })
        })
        .collect::<Vec<_>>();
    if remote_endpoint.is_none() {
        degradations.push(json!({ "subject_ref": "placement.remote", "state": "unavailable", "reason": "no WORKCELL_CONTROL_ENDPOINT is set; no remote Workcell is configured", "native_error": null }));
    }
    if model_serving_active.is_empty() {
        degradations.push(json!({ "subject_ref": "model-serving.active", "state": "unavailable", "reason": "no model-serving service is declared on this Workcell", "native_error": null }));
    }

    let obligations = vec![
        "workcell.intent / workcell.invoke: lifecycle intent and invoke are not yet disclosed through the O:I kernel seam; they render as named obligations, not controls",
        "remote Workcell settings disclosure through workcell.control/v1 is available only when the serving cell was started by `workcell serve` (which supplies the disclosure); a serving cell without one reports unavailable rather than a fabricated remote reading",
        "accelerator (GPU) observation: no faculty exists to observe accelerators in collapsed-local",
        "host hardware enumeration: no faculty exists to enumerate CPU/memory at the Workcell level",
        "generic cross-host network reachability: host-process execution does not claim or enforce it, and no fabric provider is registered",
    ];

    let mut descriptor = json!({
        "schema": "oi.product-settings-disclosure/v2",
        "product_id": "workcell",
        "contract_revision": "wave-5/system.1",
        "disclosed_at_unix_ms": now,
        "owner": {
            "owner_id": "workcell",
            "owner_ref": owner_ref,
            "owner_version": env!("CARGO_PKG_VERSION"),
            "reading_command": ["workcell", "system", "--json"],
            "reading_digest": null,
            "reading_digest_covers": "descriptor with every *_unix_ms field zeroed (disclosed_at_unix_ms, owner.observed_at_unix_ms, every axes.*.provenance.observed_at_unix_ms) and owner.reading_digest set to null",
            "observed_at_unix_ms": now,
        },
        "about": "Workcell is the materialisation centre of the O:I field: it turns provider-neutral demand into a reachable, inspectable material world. This disclosure reports the collapsed-local operational domain — Workcell identity, providers/offers/capabilities, processes/services, storage/artifacts, fabric/reachability, local/remote placement, model-serving materialisation, hardware observations and lifecycle — with logical identity kept distinct from provider, process and material binding.",
        "sections": sections,
        "actions": actions,
        "availability": { "state": availability_state, "reason": availability_reason },
        "degradations": degradations,
        "obligations": obligations,
    });

    let mut canonical = descriptor.clone();
    zero_unix_ms(&mut canonical);
    if let Some(owner) = canonical.get_mut("owner").and_then(Value::as_object_mut) {
        owner.insert("reading_digest".into(), Value::Null);
    }
    let canonical_body = serde_json::to_string(&canonical).unwrap_or_default();
    let digest = system_hex_digest(&Sha256::digest(canonical_body.as_bytes()));
    if let Some(owner) = descriptor.get_mut("owner").and_then(Value::as_object_mut) {
        owner.insert("reading_digest".into(), json!(digest));
    }
    Ok(descriptor)
}

// ---------------------------------------------------------------------------
// Configuration plane — Workcell's owner contribution and owner-native
// mutation transport (oi.configuration-contribution/v1; frozen by
// O-I docs/cradle/09-CONFIGURATION-PLANE.md, #299 Gate A / C0).
//
// One setting is contributed: the operator-declared provider/material policy
// (`services.declared`) that every composition of this collapsed-local Workcell
// already reads from the state root. Observed hardware, provider availability,
// instances and staged material are disclosure-plane facts (`workcell system
// --json`) and are deliberately not writable here. Provider choices offered
// through this plane are validated against this machine's real material
// possibility before they may enter declared state: a declaration naming a
// program this machine does not have is refused, never stored.
// ---------------------------------------------------------------------------

const CONFIG_CONTRIBUTION_SCHEMA: &str = "oi.configuration-contribution/v1";
const CONFIG_CONTRACT_REVISION: &str = "configuration-plane/contribution.1";
const CONFIG_SETTING_SERVICES: &str = "workcell:processes-services:services.declared";
const CONFIG_SETTING_SECTION: &str = "processes-services";
const CONFIG_SERVICES_NATIVE_REF: &str = "workcell:state-root:services.json";
const CONFIG_HISTORY_DIR: &str = "config";
const CONFIG_HISTORY_FILE: &str = "history.jsonl";

/// The frozen seed scope kinds (09 §5). An open registry: new kinds extend the
/// contract minor revision, they are never silently accepted here.
const CONFIG_SCOPE_KINDS: [&str; 12] = [
    "world",
    "ground",
    "project",
    "machine",
    "workcell",
    "agency",
    "agent",
    "session-space",
    "agent-session",
    "provider",
    "connector-relation",
    "invocation",
];
/// Singular kinds carry no scope_ref in the compact form.
const CONFIG_SINGULAR_SCOPE_KINDS: [&str; 3] = ["world", "ground", "machine"];

/// A structured configuration-plane failure: emitted as `oi.config-error/v1` on
/// stdout, then mapped onto the CLI's ordinary non-zero exit codes.
struct ConfigFailure {
    code: &'static str,
    message: String,
    setting_ref: Option<String>,
    scope_kind: Option<String>,
    retryable: bool,
}

impl ConfigFailure {
    fn new(code: &'static str, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
            setting_ref: None,
            scope_kind: None,
            retryable: false,
        }
    }

    fn for_setting(mut self, setting_ref: &str) -> Self {
        self.setting_ref = Some(setting_ref.to_owned());
        self
    }

    fn for_scope_kind(mut self, scope_kind: &str) -> Self {
        self.scope_kind = Some(scope_kind.to_owned());
        self
    }

    fn retryable(mut self) -> Self {
        self.retryable = true;
        self
    }

    fn emit(&self) {
        emit_json(json!({
            "schema": "oi.config-error/v1",
            "error_code": self.code,
            "message": self.message,
            "setting_ref": self.setting_ref,
            "scope_kind": self.scope_kind,
            "retryable": self.retryable,
            "detail_ref": null,
        }));
    }

    fn workcell_error(&self) -> WorkcellError {
        match self.code {
            "owner_unavailable" => WorkcellError::Unavailable(self.message.clone()),
            "internal" => WorkcellError::OperationFailed(self.message.clone()),
            _ => WorkcellError::InvalidDemand(self.message.clone()),
        }
    }
}

fn command_config(global: &GlobalArgs, args: &[String]) -> Result<(), WorkcellError> {
    if let Err(failure) = config_run(global, args) {
        failure.emit();
        return Err(failure.workcell_error());
    }
    Ok(())
}

fn config_run(global: &GlobalArgs, args: &[String]) -> Result<(), ConfigFailure> {
    let Some(verb) = args.first().map(String::as_str) else {
        return Err(ConfigFailure::new(
            "validation_failed",
            "usage: workcell config <validate|plan|apply|reset> --json ...",
        ));
    };
    let verb_args = &args[1..];
    match verb {
        "validate" => config_validate(global, verb_args),
        "plan" => config_plan(global, verb_args),
        "apply" => config_apply(global, verb_args),
        "reset" => config_reset(global, verb_args),
        other => Err(ConfigFailure::new(
            "validation_failed",
            format!("unknown config verb `{other}`; expected validate|plan|apply|reset"),
        )),
    }
}

/// Owner-minted unique identifier (plan ids, receipt ids, owner-side changesets).
/// Uniqueness comes from clock + pid + a process-local counter, hashed so the
/// identifier is opaque.
fn config_unique_id(prefix: &str) -> String {
    use std::sync::atomic::{AtomicU64, Ordering};
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_nanos())
        .unwrap_or(0);
    let material = format!(
        "{}-{}-{}",
        nanos,
        std::process::id(),
        COUNTER.fetch_add(1, Ordering::Relaxed)
    );
    use sha2::{Digest, Sha256};
    let digest = Sha256::digest(material.as_bytes());
    let short: String = digest.iter().take(8).map(|byte| format!("{byte:02x}")).collect();
    format!("{prefix}-{short}")
}

fn config_sha256_hex(value: &Value) -> String {
    use sha2::{Digest, Sha256};
    let body = serde_json::to_string(value).unwrap_or_default();
    system_hex_digest(&Sha256::digest(body.as_bytes()))
}

/// The frozen plan-digest convention (09 §6): sha256 hex over the canonical plan
/// body — the plan document with `plan_digest` removed, `plan_id` zeroed to the
/// empty string, `expires_at_unix_ms` zeroed and every `*_unix_ms` field zeroed.
/// Owner-minted and owner-verified; O:I treats the digest as opaque.
fn config_plan_digest(plan: &Value) -> String {
    let mut body = plan.clone();
    if let Some(object) = body.as_object_mut() {
        object.remove("plan_digest");
        object.insert("plan_id".into(), json!(""));
        object.insert("expires_at_unix_ms".into(), json!(0));
    }
    zero_unix_ms(&mut body);
    config_sha256_hex(&body)
}

fn config_scope_from_json(value: &Value) -> Result<(String, Option<String>, Value), ConfigFailure> {
    let kind = value
        .get("scope_kind")
        .and_then(Value::as_str)
        .ok_or_else(|| {
            ConfigFailure::new(
                "validation_failed",
                "plan scope must carry `scope_kind` and `scope_ref`",
            )
        })?;
    if !CONFIG_SCOPE_KINDS.contains(&kind) {
        return Err(ConfigFailure::new(
            "unknown_scope_kind",
            format!("unknown scope kind `{kind}`; the frozen scope registry is: {}", CONFIG_SCOPE_KINDS.join(", ")),
        )
        .for_scope_kind(kind));
    }
    let scope_ref = match value.get("scope_ref") {
        None | Some(Value::Null) => None,
        Some(Value::String(text)) => Some(text.clone()),
        Some(_) => {
            return Err(ConfigFailure::new(
                "validation_failed",
                "plan scope `scope_ref` must be a string or null",
            )
            .for_scope_kind(kind))
        }
    };
    config_check_scope_shape(kind, scope_ref.as_deref())?;
    Ok((kind.to_owned(), scope_ref, value.clone()))
}

/// Compact form `<scope_kind>:<scope_ref>` (CLI/grammar only); the ref is
/// omitted for singular kinds.
fn config_parse_scope_arg(spec: &str) -> Result<(String, Option<String>), ConfigFailure> {
    let (kind, scope_ref) = match spec.split_once(':') {
        Some((kind, rest)) => (kind, Some(rest.to_owned())),
        None => (spec, None),
    };
    if !CONFIG_SCOPE_KINDS.contains(&kind) {
        return Err(ConfigFailure::new(
            "unknown_scope_kind",
            format!(
                "unknown scope kind `{kind}`; the frozen scope registry is: {}",
                CONFIG_SCOPE_KINDS.join(", ")
            ),
        )
        .for_scope_kind(kind));
    }
    config_check_scope_shape(kind, scope_ref.as_deref())?;
    Ok((kind.to_owned(), scope_ref))
}

fn config_check_scope_shape(kind: &str, scope_ref: Option<&str>) -> Result<(), ConfigFailure> {
    let singular = CONFIG_SINGULAR_SCOPE_KINDS.contains(&kind);
    match scope_ref {
        Some(reference) if reference.trim().is_empty() => Err(ConfigFailure::new(
            "unsupported_scope",
            format!("scope kind `{kind}` requires a non-empty scope_ref"),
        )
        .for_scope_kind(kind)),
        Some(_) if singular => Err(ConfigFailure::new(
            "unsupported_scope",
            format!("scope kind `{kind}` is singular; its compact form carries no scope_ref"),
        )
        .for_scope_kind(kind)),
        Some(_) => Ok(()),
        None if singular => Ok(()),
        None => Err(ConfigFailure::new(
            "unsupported_scope",
            format!("scope kind `{kind}` requires the compact form `{kind}:<scope_ref>`"),
        )
        .for_scope_kind(kind)),
    }
}

/// The one contributed setting allows only the `workcell` scope kind (09 §5:
/// a scope kind outside the setting's allowed_scopes is an error, never a
/// fallback to another scope).
fn config_check_scope_allowed(kind: &str) -> Result<(), ConfigFailure> {
    if kind == "workcell" {
        Ok(())
    } else {
        Err(ConfigFailure::new(
            "unsupported_scope",
            format!(
                "scope kind `{kind}` is not in the allowed scopes of `{CONFIG_SETTING_SERVICES}` (workcell)"
            ),
        )
        .for_scope_kind(kind))
    }
}

fn config_scope_json(kind: &str, scope_ref: Option<&str>) -> Value {
    json!({ "scope_kind": kind, "scope_ref": scope_ref })
}

fn config_resolve_setting(setting_ref: &str) -> Result<(), ConfigFailure> {
    if setting_ref == CONFIG_SETTING_SERVICES {
        Ok(())
    } else {
        Err(ConfigFailure::new(
            "unsupported_setting",
            format!(
                "unknown setting `{setting_ref}`; Workcell contributes exactly `{CONFIG_SETTING_SERVICES}`"
            ),
        )
        .for_setting(setting_ref))
    }
}

/// The declared effect of changing the contributed setting (frozen effect
/// vocabulary, 09 §11). Applying the policy changes what the next discovery
/// offers; nothing material is started until a demand requires it.
fn config_expected_effect() -> Value {
    json!({
        "kind": "value-change",
        "summary": "The next discovery on this Workcell offers the declared services; nothing is started until a demand requires them.",
        "ref": "workcell discover --json",
    })
}

fn config_read_stdin_or_file(spec: &str, flag: &str) -> Result<String, ConfigFailure> {
    if spec == "-" {
        use std::io::Read;
        let mut raw = String::new();
        std::io::stdin()
            .read_to_string(&mut raw)
            .map_err(|error| ConfigFailure::new("validation_failed", format!("read {flag} from stdin: {error}")))?;
        Ok(raw)
    } else {
        fs::read_to_string(spec).map_err(|error| {
            ConfigFailure::new("validation_failed", format!("read {flag} `{spec}`: {error}"))
        })
    }
}

fn config_parse_json(raw: &str, flag: &str) -> Result<Value, ConfigFailure> {
    serde_json::from_str(raw)
        .map_err(|error| ConfigFailure::new("invalid_value", format!("parse {flag}: {error}")))
}

/// Native validation of a declared-services value. The owner's real parser
/// (`parse_service_declarations`) stays the single semantic implementation; the
/// material checks after it are the C3E law: a provider choice this machine
/// cannot honour is an `invalid_value` violation, never accepted declared state.
fn config_services_violations(value: &Value) -> Vec<Value> {
    let mut violations = Vec::new();
    let Some(entries) = value.as_array() else {
        violations.push(json!({
            "code": "invalid_value",
            "message": "the declared-services value must be a JSON array of workcell.service-declaration/v1 entries",
            "path": null,
        }));
        return violations;
    };

    let document = json!({
        "schema": epilogos_workcell_runtime::SERVICE_DECLARATION_SCHEMA,
        "services": entries,
    });
    let raw = serde_json::to_string(&document).unwrap_or_default();
    let declared = match epilogos_workcell_runtime::parse_service_declarations(&raw) {
        Ok(declared) => declared,
        Err(error) => {
            violations.push(json!({
                "code": "invalid_value",
                "message": error.to_string(),
                "path": null,
            }));
            return violations;
        }
    };

    for service in &declared.managed {
        let index = declared
            .managed
            .iter()
            .position(|candidate| candidate.logical_ref == service.logical_ref)
            .unwrap_or_default();
        if !config_program_available(&service.program) {
            violations.push(config_material_violation(
                &service.logical_ref,
                index,
                "program",
                &service.program,
            ));
        }
        if let Some(cwd) = &service.cwd {
            if !cwd.is_dir() {
                violations.push(json!({
                    "code": "invalid_value",
                    "message": format!(
                        "declared service `{}` cannot be honoured on this machine: working directory `{}` does not exist here",
                        service.logical_ref,
                        cwd.display()
                    ),
                    "path": format!("/{index}/cwd"),
                }));
            }
        }
    }
    for service in &declared.target_owned {
        let index = declared
            .target_owned
            .iter()
            .position(|candidate| candidate.logical_ref == service.logical_ref)
            .unwrap_or_default();
        let commands = [
            ("status", Some(&service.status)),
            ("readiness", service.readiness.as_ref()),
            ("start", service.start.as_ref()),
            ("stop", service.stop.as_ref()),
            ("restart", service.restart.as_ref()),
        ];
        for (role, command) in commands {
            let Some(command) = command else { continue };
            if !config_program_available(&command.program) {
                violations.push(config_material_violation(
                    &service.logical_ref,
                    index,
                    role,
                    &command.program,
                ));
            }
        }
    }
    violations
}

fn config_material_violation(
    logical_ref: &str,
    index: usize,
    role: &str,
    program: &str,
) -> Value {
    // A managed service declares `program` directly; a target-owned command
    // nests it under its role, and the JSON path follows the value's shape.
    let path = if role == "program" {
        format!("/{index}/program")
    } else {
        format!("/{index}/{role}/program")
    };
    json!({
        "code": "invalid_value",
        "message": format!(
            "declared service `{logical_ref}` cannot be honoured on this machine: {role} `{program}` does not exist here; this Workcell does not accept a declaration it cannot materialise or observe"
        ),
        "path": path,
    })
}

/// Material possibility on this machine: the program must exist — directly when
/// the declaration names a path, otherwise on PATH — and on Unix it must be
/// executable. This is evidence about this machine, gathered at validation time;
/// it is never stored as setting state.
fn config_program_available(program: &str) -> bool {
    let path = if program.contains('/') {
        Some(PathBuf::from(program))
    } else {
        env::var_os("PATH").as_deref().and_then(|paths| {
            env::split_paths(paths)
                .map(|dir| dir.join(program))
                .find(|candidate| candidate.is_file())
        })
    };
    let Some(path) = path else {
        return false;
    };
    if !path.is_file() {
        return false;
    }
    config_program_executable(&path)
}

/// Evidence about this machine, gathered at validation time and never stored as
/// setting state.
#[allow(unused_variables)]
fn config_program_executable(path: &Path) -> bool {
    #[cfg(unix)]
    let executable = {
        use std::os::unix::fs::PermissionsExt;
        fs::metadata(path)
            .map(|metadata| metadata.permissions().mode() & 0o111 != 0)
            .unwrap_or(false)
    };
    #[cfg(not(unix))]
    let executable = true;
    executable
}

fn config_validation_doc(setting_ref: &str, scope: &Value, violations: Vec<Value>) -> Value {
    let valid = violations.is_empty();
    json!({
        "schema": "oi.config-validation/v1",
        "setting_ref": setting_ref,
        "scope": scope,
        "valid": valid,
        "violations": violations,
        "expected_effect": config_expected_effect(),
    })
}

fn config_read_value(args: &[String]) -> Result<Value, ConfigFailure> {
    let mut value: Option<Value> = None;
    let mut index = 0;
    while index < args.len() {
        match args[index].as_str() {
            "--value" => {
                if value.is_some() {
                    return Err(ConfigFailure::new(
                        "validation_failed",
                        "--value and --value-file are mutually exclusive",
                    ));
                }
                let raw = require_value(args, index, "--value").map_err(|error| {
                    ConfigFailure::new("validation_failed", error.to_string())
                })?;
                value = Some(config_parse_json(raw, "--value")?);
                index += 2;
            }
            "--value-file" => {
                if value.is_some() {
                    return Err(ConfigFailure::new(
                        "validation_failed",
                        "--value and --value-file are mutually exclusive",
                    ));
                }
                let spec = require_value(args, index, "--value-file").map_err(|error| {
                    ConfigFailure::new("validation_failed", error.to_string())
                })?;
                let raw = config_read_stdin_or_file(spec, "--value-file")?;
                value = Some(config_parse_json(&raw, "--value-file")?);
                index += 2;
            }
            "--setting" | "--scope" => index += 2,
            other => {
                return Err(ConfigFailure::new(
                    "validation_failed",
                    format!("unknown config option `{other}`"),
                ))
            }
        }
    }
    value.ok_or_else(|| {
        ConfigFailure::new(
            "validation_failed",
            "this verb requires --value <json> or --value-file <path|->",
        )
    })
}

/// Parse the shared `--setting/--scope` request flags. Value flags are skipped
/// over (still flag/value pairs) so validate, plan and reset share one parser.
/// No explicit scope addresses this Workcell — the local Workcell the invocation
/// is standing on.
fn config_parse_setting_scope(
    global: &GlobalArgs,
    args: &[String],
) -> Result<(String, Value), ConfigFailure> {
    let mut setting_ref: Option<String> = None;
    let mut scope: Option<Value> = None;
    let mut index = 0;
    while index < args.len() {
        match args[index].as_str() {
            "--setting" => {
                setting_ref = Some(
                    require_value(args, index, "--setting").map_err(|error| {
                        ConfigFailure::new("validation_failed", error.to_string())
                    })?
                    .to_owned(),
                );
                index += 2;
            }
            "--scope" => {
                let spec = require_value(args, index, "--scope")
                    .map_err(|error| ConfigFailure::new("validation_failed", error.to_string()))?;
                let (kind, scope_ref) = config_parse_scope_arg(spec)?;
                config_check_scope_allowed(&kind)?;
                scope = Some(config_scope_json(&kind, scope_ref.as_deref()));
                index += 2;
            }
            "--value" | "--value-file" | "--changeset" => index += 2,
            other => {
                return Err(ConfigFailure::new(
                    "validation_failed",
                    format!("unknown config option `{other}`"),
                ))
            }
        }
    }
    let setting_ref = setting_ref.ok_or_else(|| {
        ConfigFailure::new(
            "validation_failed",
            "this verb requires --setting <setting_ref>",
        )
    })?;
    config_resolve_setting(&setting_ref)?;
    let scope =
        scope.unwrap_or_else(|| config_scope_json("workcell", Some(&global.workcell_ref)));
    Ok((setting_ref, scope))
}

fn config_validate(global: &GlobalArgs, args: &[String]) -> Result<(), ConfigFailure> {
    let (setting_ref, scope) = config_parse_setting_scope(global, args)?;
    let value = config_read_value(args)?;
    let violations = config_services_violations(&value);
    emit_json(config_validation_doc(&setting_ref, &scope, violations));
    Ok(())
}

fn config_plan(global: &GlobalArgs, args: &[String]) -> Result<(), ConfigFailure> {
    let (setting_ref, scope) = config_parse_setting_scope(global, args)?;
    let value = config_read_value(args)?;
    let violations = config_services_violations(&value);
    if let Some(first) = violations.first() {
        let message = first
            .get("message")
            .and_then(Value::as_str)
            .unwrap_or("the value cannot be honoured on this machine");
        return Err(ConfigFailure::new("invalid_value", message)
            .for_setting(&setting_ref)
            .for_scope_kind(scope["scope_kind"].as_str().unwrap_or("workcell")));
    }

    let plan_id = config_unique_id("wcplan");
    let count = value.as_array().map(Vec::len).unwrap_or(0);
    let mut plan = json!({
        "schema": "oi.config-plan/v1",
        "plan_id": plan_id,
        "plan_digest": "",
        "setting_ref": setting_ref,
        "scope": scope,
        "changes": [{
            "summary": format!(
                "Set the declared logical services of this Workcell ({count} service(s)); the next discovery offers them."
            ),
            "native_ref": CONFIG_SERVICES_NATIVE_REF,
            "before_ref": null,
            "after_ref": CONFIG_SERVICES_NATIVE_REF,
        }],
        "expected_effect": config_expected_effect(),
        "expires_at_unix_ms": null,
        "explain_ref": "workcell config-contribution --json",
        "authority": null,
        // Owner extension (09 §15: consumers must accept unknown fields): the
        // planned declared-state payload, so apply is driven by exactly what
        // was planned.
        "value": value,
    });
    let digest = config_plan_digest(&plan);
    plan["plan_digest"] = json!(digest);
    emit_json(plan);
    Ok(())
}

fn config_history_path(state_root: &Path) -> PathBuf {
    state_root.join(CONFIG_HISTORY_DIR).join(CONFIG_HISTORY_FILE)
}

/// The owner's own history is the record of record (09 §9); receipts reference
/// it through `native_ref`. One executed operation per line.
fn config_history_records(state_root: &Path) -> Result<Vec<Value>, ConfigFailure> {
    let path = config_history_path(state_root);
    if !path.is_file() {
        return Ok(Vec::new());
    }
    let raw = fs::read_to_string(&path).map_err(|error| {
        ConfigFailure::new(
            "owner_unavailable",
            format!("read config history `{}`: {error}", path.display()),
        )
        .retryable()
    })?;
    raw.lines()
        .filter(|line| !line.trim().is_empty())
        .map(|line| {
            serde_json::from_str(line).map_err(|error| {
                ConfigFailure::new(
                    "internal",
                    format!("config history `{}` has an unreadable line: {error}", path.display()),
                )
            })
        })
        .collect()
}

fn config_history_append(state_root: &Path, receipt: &Value) -> Result<(), ConfigFailure> {
    let path = config_history_path(state_root);
    let parent = path.parent().ok_or_else(|| {
        ConfigFailure::new("internal", "config history path has no parent directory")
    })?;
    fs::create_dir_all(parent).map_err(|error| {
        ConfigFailure::new(
            "owner_unavailable",
            format!(
                "create config history directory `{}`: {error}",
                parent.display()
            ),
        )
        .retryable()
    })?;
    let line = serde_json::to_string(receipt)
        .map_err(|error| ConfigFailure::new("internal", format!("serialise receipt: {error}")))?;
    use std::io::Write;
    let mut file = fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)
        .map_err(|error| {
            ConfigFailure::new(
                "owner_unavailable",
                format!("open config history `{}`: {error}", path.display()),
            )
            .retryable()
        })?;
    writeln!(file, "{line}").map_err(|error| {
        ConfigFailure::new(
            "owner_unavailable",
            format!("append config history `{}`: {error}", path.display()),
        )
        .retryable()
    })?;
    Ok(())
}

/// The frozen idempotency key (09 §9): (owner_ref, changeset_id, setting_ref,
/// scope, plan_digest). Enforcement is owner-side; a replay finds the executed
/// receipt and answers `no_op` naming it.
fn config_find_executed(
    state_root: &Path,
    changeset_id: &str,
    setting_ref: &str,
    scope: &Value,
    plan_digest: Option<&str>,
) -> Result<Option<Value>, ConfigFailure> {
    let digest_value = match plan_digest {
        Some(digest) => json!(digest),
        None => Value::Null,
    };
    for record in config_history_records(state_root)? {
        if record.get("outcome").and_then(Value::as_str) != Some("applied") {
            continue;
        }
        if record.get("owner_ref").and_then(Value::as_str) == Some("workcell")
            && record.get("changeset_id").and_then(Value::as_str) == Some(changeset_id)
            && record.get("setting_ref").and_then(Value::as_str) == Some(setting_ref)
            && record.get("scope") == Some(scope)
            && record.get("plan_digest") == Some(&digest_value)
        {
            return Ok(Some(record));
        }
    }
    Ok(None)
}

/// Write the declared-services document into the state root atomically. This is
/// the whole material effect of apply: the same file every composition of this
/// Workcell already reads.
fn config_write_services_document(
    state_root: &Path,
    services: &Value,
) -> Result<(), ConfigFailure> {
    fs::create_dir_all(state_root).map_err(|error| {
        ConfigFailure::new(
            "owner_unavailable",
            format!("create state root `{}`: {error}", state_root.display()),
        )
        .retryable()
    })?;
    let document = json!({
        "schema": epilogos_workcell_runtime::SERVICE_DECLARATION_SCHEMA,
        "services": services,
    });
    let body = serde_json::to_string_pretty(&document)
        .map_err(|error| ConfigFailure::new("internal", format!("serialise declaration: {error}")))?;
    let target = epilogos_workcell_runtime::default_service_declaration_path(state_root);
    let temporary = state_root.join(format!(".services.json.tmp-{}", std::process::id()));
    fs::write(&temporary, format!("{body}\n")).map_err(|error| {
        ConfigFailure::new(
            "owner_unavailable",
            format!("write declared services `{}`: {error}", temporary.display()),
        )
        .retryable()
    })?;
    fs::rename(&temporary, &target).map_err(|error| {
        ConfigFailure::new(
            "owner_unavailable",
            format!(
                "move declared services into place `{}`: {error}",
                target.display()
            ),
        )
        .retryable()
    })?;
    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn config_receipt(
    receipt_id: String,
    changeset_id: &str,
    plan_digest: Option<&str>,
    setting_ref: &str,
    scope: &Value,
    operation: &str,
    outcome: &str,
    native_ref: Option<String>,
    original_receipt_id: Option<String>,
) -> Value {
    json!({
        "schema": "oi.config-receipt/v1",
        "receipt_id": receipt_id,
        "owner_ref": "workcell",
        "changeset_id": changeset_id,
        "plan_digest": plan_digest,
        "setting_ref": setting_ref,
        "scope": scope,
        "operation": operation,
        "outcome": outcome,
        "applied_at_unix_ms": system_now_ms(),
        "native_ref": native_ref,
        "expected_effect": config_expected_effect(),
        "original_receipt_id": original_receipt_id,
        "error": null,
    })
}

fn config_apply(global: &GlobalArgs, args: &[String]) -> Result<(), ConfigFailure> {
    let mut plan_source: Option<String> = None;
    let mut changeset: Option<String> = None;
    let mut index = 0;
    while index < args.len() {
        match args[index].as_str() {
            "--plan-file" => {
                plan_source = Some(
                    require_value(args, index, "--plan-file").map_err(|error| {
                        ConfigFailure::new("validation_failed", error.to_string())
                    })?
                    .to_owned(),
                );
                index += 2;
            }
            "--changeset" => {
                changeset = Some(
                    require_value(args, index, "--changeset").map_err(|error| {
                        ConfigFailure::new("validation_failed", error.to_string())
                    })?
                    .to_owned(),
                );
                index += 2;
            }
            other => {
                return Err(ConfigFailure::new(
                    "validation_failed",
                    format!("unknown config apply option `{other}`"),
                ))
            }
        }
    }
    let spec = plan_source.ok_or_else(|| {
        ConfigFailure::new(
            "validation_failed",
            "apply requires --plan-file <path|-> (mint one with `workcell config plan --json`)",
        )
    })?;
    let raw = config_read_stdin_or_file(&spec, "--plan-file")?;
    let plan = config_parse_json(&raw, "--plan-file")?;
    if plan.get("schema").and_then(Value::as_str) != Some("oi.config-plan/v1") {
        return Err(ConfigFailure::new(
            "unsupported_schema",
            "the plan document is not `oi.config-plan/v1`",
        ));
    }
    let plan_id = plan
        .get("plan_id")
        .and_then(Value::as_str)
        .filter(|value| !value.trim().is_empty())
        .ok_or_else(|| ConfigFailure::new("validation_failed", "the plan carries no plan_id"))?;
    let digest = plan
        .get("plan_digest")
        .and_then(Value::as_str)
        .filter(|value| !value.trim().is_empty())
        .ok_or_else(|| ConfigFailure::new("validation_failed", "the plan carries no plan_digest"))?
        .to_owned();
    let setting_ref = plan
        .get("setting_ref")
        .and_then(Value::as_str)
        .ok_or_else(|| ConfigFailure::new("validation_failed", "the plan carries no setting_ref"))?
        .to_owned();
    config_resolve_setting(&setting_ref)?;
    let (kind, _scope_ref, scope) =
        config_scope_from_json(plan.get("scope").unwrap_or(&Value::Null))
            .map_err(|failure| failure.for_setting(&setting_ref))?;
    config_check_scope_allowed(&kind)
        .map_err(|failure| failure.for_setting(&setting_ref))?;

    // The plan is verified against its minted digest before anything runs:
    // a modified plan is not the plan the owner made.
    if config_plan_digest(&plan) != digest {
        return Err(ConfigFailure::new(
            "validation_failed",
            format!("plan `{plan_id}` does not match its plan_digest; it was modified after minting"),
        )
        .for_setting(&setting_ref));
    }

    let value = plan.get("value").cloned().unwrap_or(Value::Null);
    let violations = config_services_violations(&value);
    if let Some(first) = violations.first() {
        let message = first
            .get("message")
            .and_then(Value::as_str)
            .unwrap_or("the planned value cannot be honoured on this machine");
        return Err(ConfigFailure::new("invalid_value", message)
            .for_setting(&setting_ref)
            .for_scope_kind(&kind));
    }

    let changeset =
        changeset.unwrap_or_else(|| config_unique_id("cs-workcell"));
    if let Some(original) = config_find_executed(
        &global.state_root,
        &changeset,
        &setting_ref,
        &scope,
        Some(&digest),
    )? {
        let original_id = original
            .get("receipt_id")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_owned();
        let native_ref = original
            .get("native_ref")
            .and_then(Value::as_str)
            .map(str::to_owned);
        let receipt = config_receipt(
            config_unique_id("wcreceipt"),
            &changeset,
            Some(&digest),
            &setting_ref,
            &scope,
            "apply",
            "no_op",
            native_ref,
            Some(original_id),
        );
        config_history_append(&global.state_root, &receipt)?;
        emit_json(receipt);
        return Ok(());
    }

    config_write_services_document(&global.state_root, &value)?;
    // The receipt's native_ref names its own line in the owner's history, so
    // the line number is fixed before the record is appended.
    let line_no = config_history_records(&global.state_root)?.len() as u64 + 1;
    let mut receipt = config_receipt(
        config_unique_id("wcreceipt"),
        &changeset,
        Some(&digest),
        &setting_ref,
        &scope,
        "apply",
        "applied",
        None,
        None,
    );
    receipt["native_ref"] = json!(format!("workcell:config:history:{line_no}"));
    config_history_append(&global.state_root, &receipt)?;
    emit_json(receipt);
    Ok(())
}

fn config_reset(global: &GlobalArgs, args: &[String]) -> Result<(), ConfigFailure> {
    let (setting_ref, scope) = config_parse_setting_scope(global, args)?;
    let mut changeset: Option<String> = None;
    let mut index = 0;
    while index < args.len() {
        match args[index].as_str() {
            "--changeset" => {
                changeset = Some(
                    require_value(args, index, "--changeset").map_err(|error| {
                        ConfigFailure::new("validation_failed", error.to_string())
                    })?
                    .to_owned(),
                );
                index += 2;
            }
            "--setting" | "--scope" => index += 2,
            other => {
                return Err(ConfigFailure::new(
                    "validation_failed",
                    format!("unknown config reset option `{other}`"),
                ))
            }
        }
    }
    let changeset = changeset.unwrap_or_else(|| config_unique_id("cs-workcell"));

    if let Some(original) = config_find_executed(
        &global.state_root,
        &changeset,
        &setting_ref,
        &scope,
        None,
    )? {
        let original_id = original
            .get("receipt_id")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_owned();
        let native_ref = original
            .get("native_ref")
            .and_then(Value::as_str)
            .map(str::to_owned);
        let receipt = config_receipt(
            config_unique_id("wcreceipt"),
            &changeset,
            None,
            &setting_ref,
            &scope,
            "reset",
            "no_op",
            native_ref,
            Some(original_id),
        );
        config_history_append(&global.state_root, &receipt)?;
        emit_json(receipt);
        return Ok(());
    }

    // The owner baseline: no declared services. The document keeps the
    // declaration schema so the file stays an honest, parseable declaration.
    config_write_services_document(&global.state_root, &json!([]))?;
    let line_no = config_history_records(&global.state_root)?.len() as u64 + 1;
    let mut receipt = config_receipt(
        config_unique_id("wcreceipt"),
        &changeset,
        None,
        &setting_ref,
        &scope,
        "reset",
        "applied",
        None,
        None,
    );
    receipt["native_ref"] = json!(format!("workcell:config:history:{line_no}"));
    config_history_append(&global.state_root, &receipt)?;
    emit_json(receipt);
    Ok(())
}

/// Emit Workcell's configuration contribution (`oi.configuration-contribution/v1`)
/// — bare on stdout with `--json`. Availability is probed through the same
/// collapsed-local discovery the disclosure plane uses, never asserted.
fn command_config_contribution(global: &GlobalArgs) -> Result<(), WorkcellError> {
    use sha2::{Digest, Sha256};

    let workcell_ref = parse_workcell_ref(&global.workcell_ref)?;
    let workcell = new_local(global, workcell_ref, BTreeSet::new())?;
    let discovery = workcell.discover()?;

    let now = system_now_ms();
    let availability_state = match discovery.health {
        HealthState::Healthy => "available",
        HealthState::Degraded => "degraded",
        HealthState::Unavailable => "unavailable",
        HealthState::Unknown => "unknown",
    };
    let availability_reason = if availability_state == "available" {
        None
    } else {
        Some("collapsed-local discovery reports non-healthy state".to_owned())
    };

    let setting = json!({
        "setting_ref": CONFIG_SETTING_SERVICES,
        "section_ref": CONFIG_SETTING_SECTION,
        "title": "Declared logical services",
        "description": "The operator's provider/material policy for this Workcell: which logical services it offers, whether each is materialised as a managed host process or observed as a target-owned service, where its endpoint is and how readiness is decided. Values are validated against this machine's material possibility — a declaration naming a program that does not exist here is refused and never enters declared state. Native state: services.json in the Workcell state root.",
        "value_schema": {
            "type": "table",
            "columns": [
                { "name": "logical_ref", "type": "scalar" },
                { "name": "lifetime", "type": "scalar" },
                { "name": "endpoint", "type": "scalar" },
                { "name": "program", "type": "scalar" },
                { "name": "args", "type": "list" },
                { "name": "acquisition", "type": "scalar" },
                { "name": "readiness", "type": "table" }
            ]
        },
        "allowed_scopes": [{ "scope_kind": "workcell", "scope_ref": null }],
        "writable": true,
        "profileable": false,
        "sensitive": false,
        "default": [],
        "default_semantics": "constant",
        "effect": {
            "kind": "value-change",
            "summary": "The next discovery on this Workcell offers the declared services; nothing is started until a demand requires them.",
            "ref": "workcell discover --json"
        },
        "operations": { "validate": true, "plan": true, "apply": true, "reset": true },
        "native_ref": CONFIG_SERVICES_NATIVE_REF,
    });

    let mut contribution = json!({
        "schema": CONFIG_CONTRIBUTION_SCHEMA,
        "contract_revision": CONFIG_CONTRACT_REVISION,
        "owner": {
            "owner_ref": "workcell",
            "owner_kind": "product",
            "owner_version": env!("CARGO_PKG_VERSION"),
            "contribution_command": ["workcell", "config-contribution", "--json"],
            "disclosed_at_unix_ms": now,
            "reading_digest": null,
            "reading_digest_covers": "document with every *_unix_ms field zeroed and owner.reading_digest set to null",
        },
        "about": "Workcell's owned configuration surface: the operator-declared provider/material policy for this Workcell — which logical services it offers and how each may materialise. Provider choices are validated against this machine's real material possibility; observed hardware and provider availability remain disclosure facts (workcell system --json), never writable settings.",
        "sections": [{
            "id": CONFIG_SETTING_SECTION,
            "title": "Processes / services",
            "settings": [setting],
        }],
        "operations": {
            "transport": "cli/v1",
            "validate": { "availability": "disclosed", "reason": null },
            "plan": { "availability": "disclosed", "reason": null },
            "apply": { "availability": "disclosed", "reason": null },
            "reset": { "availability": "disclosed", "reason": null },
        },
        "availability": { "state": availability_state, "reason": availability_reason },
        "degradations": [],
        "obligations": [
            "multi-Workcell placement policy (epilogos-workcell-placement PlacementPolicy) is engine API only: nothing in the product persists or honours a placement-policy setting yet, so none is contributed here — placement/provider choices are instead validated for material possibility at workcell:processes-services:services.declared",
            "configuration verbs operate on this machine's Workcell state only; remote Workcell configuration through workcell.control/v1 is not implemented",
            "observed hardware, provider availability, harness instances and staged material are disclosure-plane facts (workcell system --json) and are deliberately absent as writable settings",
        ],
    });

    let mut canonical = contribution.clone();
    zero_unix_ms(&mut canonical);
    if let Some(owner) = canonical.get_mut("owner").and_then(Value::as_object_mut) {
        owner.insert("reading_digest".into(), Value::Null);
    }
    let canonical_body = serde_json::to_string(&canonical).unwrap_or_default();
    let digest = system_hex_digest(&Sha256::digest(canonical_body.as_bytes()));
    if let Some(owner) = contribution.get_mut("owner").and_then(Value::as_object_mut) {
        owner.insert("reading_digest".into(), json!(digest));
    }

    if global.json {
        emit_json(contribution);
    } else {
        println!(
            "Workcell configuration contribution (oi.configuration-contribution/v1)\n  owner: workcell ({})\n  sections: {}\n  settings: {}\n  availability: {}\n  obligations: {}\n  reading digest: {}",
            env!("CARGO_PKG_VERSION"),
            contribution["sections"].as_array().map_or(0, Vec::len),
            contribution["sections"]
                .as_array()
                .map(|sections| sections
                    .iter()
                    .map(|section| section["settings"].as_array().map_or(0, Vec::len))
                    .sum())
                .unwrap_or(0),
            contribution["availability"]["state"].as_str().unwrap_or("unknown"),
            contribution["obligations"].as_array().map_or(0, Vec::len),
            contribution["owner"]["reading_digest"].as_str().unwrap_or("null"),
        );
    }
    Ok(())
}

fn write_receipt(
    path: &Path,
    world: &epilogos_workcell_core::MaterialisedExecutionWorld,
) -> Result<(), WorkcellError> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|error| {
            WorkcellError::OperationFailed(format!(
                "create material-world receipt directory `{}`: {error}",
                parent.display()
            ))
        })?;
    }
    fs::write(path, encode_world(world)?).map_err(|error| {
        WorkcellError::OperationFailed(format!(
            "write material-world receipt `{}`: {error}",
            path.display()
        ))
    })
}

fn receipt_count(state_root: &Path) -> Result<usize, WorkcellError> {
    let worlds = state_root.join("worlds");
    if !worlds.exists() {
        return Ok(0);
    }
    let entries = fs::read_dir(&worlds)
        .map_err(|error| WorkcellError::OperationFailed(format!("read world receipts: {error}")))?;
    Ok(entries
        .filter_map(Result::ok)
        .filter(|entry| entry.path().extension().and_then(|value| value.to_str()) == Some("json"))
        .count())
}

fn provider_count(discovery: &Discovery) -> usize {
    discovery
        .offers
        .iter()
        .map(|offer| offer.provider_ref.as_str())
        .collect::<BTreeSet<_>>()
        .len()
}

fn discovery_json(discovery: &Discovery) -> Value {
    json!({
        "ok": true,
        "workcell_ref": discovery.workcell_ref.as_str(),
        "health": health(&discovery.health),
        "capacity": discovery.capacity.iter().map(|(key, value)| {
            (key.clone(), json!({"amount": value.amount, "unit": value.unit}))
        }).collect::<serde_json::Map<_, _>>(),
        "offers": discovery.offers.iter().map(|offer| json!({
            "offer_ref": offer.offer_ref.as_str(),
            "provider_ref": offer.provider_ref.as_str(),
            "port": offer.port,
            "affordances": offer.affordances,
            "connections": offer.connections,
            "exposures": offer.exposures,
            "isolation_trust": offer.isolation_trust,
            "availability": availability(&offer.availability),
            "health": health(&offer.health),
            "metadata": offer.metadata,
        })).collect::<Vec<_>>(),
    })
}

fn plan_json(plan: &MaterialisationPlan) -> Value {
    json!({
        "ok": plan.status != PlanStatus::Unsatisfiable,
        "plan_ref": plan.plan_ref.as_str(),
        "demand_ref": plan.demand_ref.as_str(),
        "status": plan_status(&plan.status),
        "planned_bindings": plan.planned_bindings.iter().map(|binding| json!({
            "logical_ref": binding.logical_ref,
            "requirement": binding.requirement,
            "necessity": necessity(binding.necessity),
            "provider_ref": binding.provider_ref.as_str(),
            "offer_ref": binding.offer_ref.as_str(),
        })).collect::<Vec<_>>(),
        "planned_exposures": plan.planned_exposures.iter().map(|binding| json!({
            "logical_ref": binding.logical_ref,
            "requirement": binding.requirement,
            "necessity": necessity(binding.necessity),
            "provider_ref": binding.provider_ref.as_str(),
            "offer_ref": binding.offer_ref.as_str(),
        })).collect::<Vec<_>>(),
        "planned_constraints": plan.planned_constraints.iter().map(|binding| json!({
            "logical_ref": binding.logical_ref,
            "requirement": binding.requirement,
            "necessity": necessity(binding.necessity),
            "provider_ref": binding.provider_ref.as_str(),
            "offer_ref": binding.offer_ref.as_str(),
        })).collect::<Vec<_>>(),
        "degradations": plan.degradations.iter().map(degradation_json).collect::<Vec<_>>(),
        "omissions": plan.omissions.iter().map(omission_json).collect::<Vec<_>>(),
        "explanation": plan.explanation,
    })
}

fn observation_json(bundle: &ObservationBundle) -> Value {
    json!({
        "ok": true,
        "world_ref": bundle.world_ref.as_str(),
        "observations": bundle.observations.iter().map(|observation| json!({
            "logical_ref": observation.logical_ref,
            "state": health(&observation.state),
            "detail": observation.detail,
        })).collect::<Vec<_>>(),
    })
}

fn exposure_json(bundle: &ExposureBundle) -> Value {
    json!({
        "ok": true,
        "world_ref": bundle.world_ref.as_str(),
        "surfaces": bundle.surfaces.iter().map(|surface| json!({
            "logical_ref": surface.logical_ref,
            "interaction": surface.interaction,
            "material": surface.material,
            "provenance": surface.provenance,
        })).collect::<Vec<_>>(),
        "degradations": bundle.degradations.iter().map(degradation_json).collect::<Vec<_>>(),
        "omissions": bundle.omissions.iter().map(omission_json).collect::<Vec<_>>(),
    })
}

fn collection_json(bundle: &CollectionBundle) -> Value {
    json!({
        "ok": true,
        "world_ref": bundle.world_ref.as_str(),
        "outputs": bundle.outputs.iter().map(|output| json!({
            "logical_ref": output.logical_ref,
            "material_locator": output.material_locator,
            "provenance": output.provenance,
        })).collect::<Vec<_>>(),
        "degradations": bundle.degradations.iter().map(degradation_json).collect::<Vec<_>>(),
        "omissions": bundle.omissions.iter().map(omission_json).collect::<Vec<_>>(),
    })
}

fn release_json(result: &ReleaseResult) -> Value {
    json!({
        "ok": true,
        "world_ref": result.world_ref.as_str(),
        "disposition": release_disposition(&result.disposition),
        "changed": result.changed,
    })
}

fn reconciliation_json(result: &ReconciliationResult) -> Value {
    json!({
        "ok": true,
        "deltas": result.deltas.iter().map(|delta| json!({
            "logical_ref": delta.logical_ref,
            "observed": delta.observed,
            "desired": delta.desired,
            "action": delta.action,
        })).collect::<Vec<_>>(),
    })
}

fn degradation_json(value: &Degradation) -> Value {
    json!({
        "requirement": value.requirement,
        "necessity": necessity(value.necessity),
        "reason": value.reason,
    })
}

fn omission_json(value: &PlanOmission) -> Value {
    json!({
        "requirement": value.requirement,
        "necessity": necessity(value.necessity),
        "reason": value.reason,
    })
}

fn print_plan(plan: &MaterialisationPlan) {
    println!("{} — {}", plan.plan_ref, plan_status(&plan.status));
    for binding in &plan.planned_bindings {
        println!(
            "{} -> {} [{}]",
            binding.logical_ref, binding.provider_ref, binding.requirement
        );
    }
    print_degradations(&plan.degradations, &plan.omissions);
}

fn print_degradations(degradations: &[Degradation], omissions: &[PlanOmission]) {
    for degradation in degradations {
        println!(
            "degraded {} ({}): {}",
            degradation.requirement,
            necessity(degradation.necessity),
            degradation.reason
        );
    }
    for omission in omissions {
        println!(
            "omitted {} ({}): {}",
            omission.requirement,
            necessity(omission.necessity),
            omission.reason
        );
    }
}

fn emit_json(value: Value) {
    println!("{value}");
}

fn health(value: &HealthState) -> &'static str {
    match value {
        HealthState::Healthy => "healthy",
        HealthState::Degraded => "degraded",
        HealthState::Unavailable => "unavailable",
        HealthState::Unknown => "unknown",
    }
}

fn availability(value: &Availability) -> &'static str {
    match value {
        Availability::Available => "available",
        Availability::Degraded => "degraded",
        Availability::Unavailable => "unavailable",
    }
}

fn plan_status(value: &PlanStatus) -> &'static str {
    match value {
        PlanStatus::Satisfiable => "satisfiable",
        PlanStatus::Degraded => "degraded",
        PlanStatus::Unsatisfiable => "unsatisfiable",
    }
}

fn necessity(value: RequirementNecessity) -> &'static str {
    match value {
        RequirementNecessity::Required => "required",
        RequirementNecessity::Preferred => "preferred",
        RequirementNecessity::Optional => "optional",
    }
}

fn release_disposition(value: &ReleaseDisposition) -> &'static str {
    match value {
        ReleaseDisposition::Released => "released",
        ReleaseDisposition::Preserved => "preserved",
        ReleaseDisposition::Suspended => "suspended",
        ReleaseDisposition::Snapshotted => "snapshotted",
    }
}

fn error_kind(error: &WorkcellError) -> &'static str {
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

fn exit_code(error: &WorkcellError) -> u8 {
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

// ---- Declared remote machines -----------------------------------------
//
// "Add a machine" as a native declaration: a label, an endpoint (cloud
// private fabric or local loopback alike — the connection protocol is
// independent of the path), and the connection credential as a secret
// *reference* resolved from this cell's origin store at connect time.

const MACHINE_USAGE: &str = "usage: workcell machine add --label LABEL --endpoint HOST:PORT [--credential-ref REF] [--allow OPERATION]... [--note TEXT] | list | remove --label LABEL";

fn command_machine(global: &GlobalArgs, args: &[String]) -> Result<(), WorkcellError> {
    let Some(subcommand) = args.first() else {
        return Err(WorkcellError::InvalidDemand(MACHINE_USAGE.into()));
    };
    let rest = &args[1..];
    match subcommand.as_str() {
        "add" => machine_add(global, rest),
        "list" => machine_list(global, rest),
        "remove" => machine_remove(global, rest),
        other => Err(WorkcellError::InvalidDemand(format!(
            "unknown machine subcommand `{other}`; {MACHINE_USAGE}"
        ))),
    }
}

fn machine_add(global: &GlobalArgs, args: &[String]) -> Result<(), WorkcellError> {
    let mut label: Option<String> = None;
    let mut endpoint: Option<String> = None;
    let mut credential_ref: Option<String> = None;
    let mut operations: Vec<String> = Vec::new();
    let mut note: Option<String> = None;
    let mut index = 0;
    while index < args.len() {
        match args[index].as_str() {
            "--label" => {
                label = Some(require_value(args, index, "--label")?.to_owned());
                index += 2;
            }
            "--endpoint" => {
                endpoint = Some(require_value(args, index, "--endpoint")?.to_owned());
                index += 2;
            }
            "--credential-ref" => {
                credential_ref = Some(require_value(args, index, "--credential-ref")?.to_owned());
                index += 2;
            }
            "--allow" => {
                operations.push(require_value(args, index, "--allow")?.to_owned());
                index += 2;
            }
            "--note" => {
                note = Some(require_value(args, index, "--note")?.to_owned());
                index += 2;
            }
            other => {
                return Err(WorkcellError::InvalidDemand(format!(
                    "unknown machine add option `{other}`"
                )))
            }
        }
    }
    let label = label.ok_or_else(|| {
        WorkcellError::InvalidDemand("machine add needs --label LABEL".into())
    })?;
    validate_label(&label)?;
    let endpoint = endpoint.ok_or_else(|| {
        WorkcellError::InvalidDemand(
            "machine add needs --endpoint HOST:PORT (cloud private address or local loopback)"
                .into(),
        )
    })?;
    for operation in &operations {
        if !CONTROL_OPERATIONS.contains(&operation.as_str()) {
            return Err(WorkcellError::InvalidDemand(format!(
                "unknown control operation `{operation}`; known operations are {}",
                CONTROL_OPERATIONS.join(", ")
            )));
        }
    }
    let registry = RemoteMachineRegistry::new(&global.state_root);
    registry.add(RemoteMachineDeclaration {
        label: label.clone(),
        endpoint: endpoint.clone(),
        credential_ref,
        operations: operations.clone(),
        note,
    })?;
    if global.json {
        emit_json(json!({
            "ok": true,
            "label": label,
            "endpoint": endpoint,
            "registry": registry.path().display().to_string(),
        }));
    } else {
        println!(
            "machine `{label}` declared at {} (endpoint {endpoint}); connect with `workcell connect --connection {label}`",
            registry.path().display()
        );
    }
    Ok(())
}

fn machine_list(global: &GlobalArgs, _args: &[String]) -> Result<(), WorkcellError> {
    let registry = RemoteMachineRegistry::new(&global.state_root);
    let machines = registry.list()?;
    if global.json {
        emit_json(json!({
            "ok": true,
            "machines": machines.iter().map(|machine| machine.to_json()).collect::<Vec<_>>(),
        }));
    } else {
        if machines.is_empty() {
            println!(
                "no machines declared in {}; add one with `workcell machine add --label LABEL --endpoint HOST:PORT`",
                registry.path().display()
            );
        }
        for machine in machines {
            println!(
                "{} -> {} operations=[{}] credential_ref={}",
                machine.label,
                machine.endpoint,
                machine.operations.join(","),
                machine.credential_ref.as_deref().unwrap_or("(none stored)")
            );
        }
    }
    Ok(())
}

fn machine_remove(global: &GlobalArgs, args: &[String]) -> Result<(), WorkcellError> {
    let mut label: Option<String> = None;
    let mut index = 0;
    while index < args.len() {
        match args[index].as_str() {
            "--label" => {
                label = Some(require_value(args, index, "--label")?.to_owned());
                index += 2;
            }
            other => {
                return Err(WorkcellError::InvalidDemand(format!(
                    "unknown machine remove option `{other}`"
                )))
            }
        }
    }
    let label = label.ok_or_else(|| {
        WorkcellError::InvalidDemand("machine remove needs --label LABEL".into())
    })?;
    let registry = RemoteMachineRegistry::new(&global.state_root);
    registry.remove(&label)?;
    if global.json {
        emit_json(json!({"ok": true, "label": label, "removed": true}));
    } else {
        println!("machine `{label}` declaration removed; any stored connection receipt is kept");
    }
    Ok(())
}

// ---- Secret origin and projection -------------------------------------
//
// One origin store per machine — the machine where the person put the
// secret. Everything in this command family keeps that law: exposure
// discovery reports location and presence only, vaulting moves material
// into the origin store without ever printing it, and projections record
// authorised relations (refs, classes, purpose, scope) — never material.

const SECRET_USAGE: &str = "usage: workcell secret scan | vault --from env:NAME|PATH [--select KEY] --ref REF [--provider secret-service|keychain] | project --name LABEL --to-sandbox --allocation-ref REF [--sandbox-provider REF] --credential-ref REF --source-provider REF --class CLASS --purpose P --scope S --by REF (workcell targets are not yet materialisable and are refused) | projections | deliver (--name LABEL | --ref PREF) --allocation SANDBOX-ID --route HOST=METHOD [--binding NAME] [--auth bearer|api-key:HEADER] | revoke-projection (--name LABEL | --ref PREF) [--sandbox SANDBOX-ID]";

fn command_secret(global: &GlobalArgs, args: &[String]) -> Result<(), WorkcellError> {
    let Some(subcommand) = args.first() else {
        return Err(WorkcellError::InvalidDemand(SECRET_USAGE.into()));
    };
    let rest = &args[1..];
    match subcommand.as_str() {
        "scan" => secret_scan(global, rest),
        "vault" => secret_vault(global, rest),
        "project" => secret_project(global, rest),
        "projections" => secret_projections(global, rest),
        "revoke-projection" => secret_revoke_projection(global, rest),
        "deliver" => secret_deliver(global, rest),
        other => Err(WorkcellError::InvalidDemand(format!(
            "unknown secret subcommand `{other}`; {SECRET_USAGE}"
        ))),
    }
}

/// The declared OpenSandbox execution deployment from this state root's
/// services file — the same declaration `discover`/`prepare` compose with.
fn declared_sandbox_deployment(
    global: &GlobalArgs,
    provider_ref: &str,
) -> Result<OpenSandboxConfig, WorkcellError> {
    let declared =
        epilogos_workcell_runtime::read_state_root_service_declarations(&global.state_root)?;
    declared
        .execution
        .into_iter()
        .find(|deployment| deployment.provider_ref.as_str() == provider_ref)
        .ok_or_else(|| {
            WorkcellError::Unavailable(format!(
                "no OpenSandbox deployment `{provider_ref}` is declared in {}`s services file; \
                 declare it under `execution` before delivering secrets to a sandbox",
                global.state_root.display()
            ))
        })
}

fn secret_deliver(global: &GlobalArgs, args: &[String]) -> Result<(), WorkcellError> {
    let mut name: Option<String> = None;
    let mut projection_ref: Option<String> = None;
    let mut allocation: Option<String> = None;
    let mut routes: Vec<String> = Vec::new();
    let mut binding: Option<String> = None;
    let mut credential_name: Option<String> = None;
    let mut paths: Vec<String> = Vec::new();
    let mut auth: Option<String> = None;
    let mut index = 0;
    while index < args.len() {
        match args[index].as_str() {
            "--name" => {
                name = Some(require_value(args, index, "--name")?.to_owned());
                index += 2;
            }
            "--ref" => {
                projection_ref = Some(require_value(args, index, "--ref")?.to_owned());
                index += 2;
            }
            "--allocation" => {
                allocation = Some(require_value(args, index, "--allocation")?.to_owned());
                index += 2;
            }
            "--route" => {
                routes.push(require_value(args, index, "--route")?.to_owned());
                index += 2;
            }
            "--binding" => {
                binding = Some(require_value(args, index, "--binding")?.to_owned());
                index += 2;
            }
            "--credential-name" => {
                credential_name = Some(require_value(args, index, "--credential-name")?.to_owned());
                index += 2;
            }
            "--paths" => {
                for path in require_value(args, index, "--paths")?.split(',') {
                    paths.push(path.to_owned());
                }
                index += 2;
            }
            "--auth" => {
                auth = Some(require_value(args, index, "--auth")?.to_owned());
                index += 2;
            }
            other => {
                return Err(WorkcellError::InvalidDemand(format!(
                    "unknown deliver option `{other}`"
                )))
            }
        }
    }
    let projection_ref = match (name, projection_ref) {
        (Some(name), None) => {
            validate_label(&name)?;
            format!("secret-projection:{name}")
        }
        (None, Some(reference)) => reference,
        (Some(_), Some(_)) => {
            return Err(WorkcellError::InvalidDemand(
                "pass --name LABEL or --ref PREF, not both".into(),
            ))
        }
        (None, None) => {
            return Err(WorkcellError::InvalidDemand(
                "deliver needs the projection: --name LABEL or --ref PREF".into(),
            ))
        }
    };
    let allocation = allocation.ok_or_else(|| {
        WorkcellError::InvalidDemand(
            "deliver needs the sandbox material id: --allocation <sandbox id from prepare>".into(),
        )
    })?;
    if routes.is_empty() {
        return Err(WorkcellError::InvalidDemand(
            "deliver needs at least one authorised route: --route HOST=METHOD".into(),
        ));
    }

    // Deny-before-write: a revoked or unknown projection delivers nothing.
    let workcell_ref = parse_workcell_ref(&global.workcell_ref)?;
    let ledger = SecretProjectionLedger::new(&global.state_root, workcell_ref);
    let record = match ledger.decision_for(&projection_ref)? {
        ProjectionDecision::Active(record) => *record,
        ProjectionDecision::Revoked(_) => {
            return Err(WorkcellError::UnsatisfiedDemand(format!(
                "secret projection `{projection_ref}` is revoked at the origin; the target receives nothing"
            )))
        }
        ProjectionDecision::Unknown => {
            return Err(WorkcellError::InvalidDemand(format!(
                "no secret projection `{projection_ref}` exists in this ledger"
            )))
        }
    };
    let projection = record.to_request()?;
    let (target_provider_ref, target_allocation_ref) = match &projection.target {
        SecretProjectionTarget::Sandbox {
            provider_ref,
            allocation_ref,
        } => (provider_ref.as_str().to_owned(), allocation_ref.clone()),
        SecretProjectionTarget::Workcell { .. } => {
            return Err(WorkcellError::InvalidDemand(
                "deliver projects to sandbox allocations; this projection names a workcell target"
                    .into(),
            ))
        }
    };
    if target_allocation_ref != allocation {
        return Err(WorkcellError::UnsatisfiedDemand(format!(
            "projection targets sandbox allocation `{target_allocation_ref}`, not `{allocation}`"
        )));
    }

    let config = declared_sandbox_deployment(global, &target_provider_ref)?;
    let broker = OpenSandboxCredentialBroker::new(config, StdHttpOpenSandboxTransport)?;

    // The origin provider is named by the projection's recorded source; the
    // material is resolved in-process and never printed.
    use epilogos_workcell_core::SecretProvider;
    enum OriginProvider {
        SecretService(SecretServiceSecretProvider),
        Keychain(KeychainSecretProvider),
        OnePassword(OnePasswordSecretProvider<OnePasswordCli>),
    }
    impl SecretProvider for OriginProvider {
        fn provider_ref(&self) -> &epilogos_workcell_core::ProviderRef {
            match self {
                Self::SecretService(provider) => provider.provider_ref(),
                Self::Keychain(provider) => provider.provider_ref(),
                Self::OnePassword(provider) => provider.provider_ref(),
            }
        }

        fn resolve(
            &self,
            credential_ref: &ExternalRef,
        ) -> epilogos_workcell_core::Result<epilogos_workcell_core::ProviderSecretMaterial> {
            match self {
                Self::SecretService(provider) => provider.resolve(credential_ref),
                Self::Keychain(provider) => provider.resolve(credential_ref),
                Self::OnePassword(provider) => provider.resolve(credential_ref),
            }
        }
    }
    let origin_provider = if record.source_provider_ref.contains("secret-service") {
        OriginProvider::SecretService(SecretServiceSecretProvider::new()?)
    } else if record.source_provider_ref.contains("keychain") {
        OriginProvider::Keychain(KeychainSecretProvider::new(
            KeychainAclPolicy::ThisDeviceUnlocked,
        )?)
    } else if record.source_provider_ref.contains("onepassword") {
        OriginProvider::OnePassword(OnePasswordSecretProvider::new(OnePasswordCli)?)
    } else {
        return Err(WorkcellError::InvalidDemand(format!(
            "unknown origin secret source `{}`; deliver resolves from secret-service, keychain or onepassword",
            record.source_provider_ref
        )));
    };

    let binding_name = binding.unwrap_or_else(|| {
        record
            .credential_ref
            .rsplit('/')
            .next()
            .unwrap_or("credential")
            .to_owned()
    });
    let credential_name = credential_name.unwrap_or_else(|| binding_name.clone());
    let request_paths = if paths.is_empty() {
        vec!["/".to_owned()]
    } else {
        paths
    };
    let binding_auth = match auth.as_deref() {
        None | Some("bearer") => OpenSandboxCredentialAuth::Bearer,
        Some(value) if value.starts_with("api-key:") => OpenSandboxCredentialAuth::ApiKey {
            header_name: value.trim_start_matches("api-key:").to_owned(),
        },
        Some(other) => {
            return Err(WorkcellError::InvalidDemand(format!(
                "unknown auth `{other}`; use bearer or api-key:HEADER"
            )))
        }
    };

    let class = SecretMaterialisationClass::CredentialBroker;
    let request = SecretMaterialisationRequest {
        credential_ref: ExternalRef::new(&record.credential_ref).map_err(WorkcellError::from)?,
        provider_ref: origin_provider.provider_ref().clone(),
        binding_ref: BindingRef::new(format!("binding:{binding_name}"))
            .map_err(WorkcellError::from)?,
        consumer_ref: ExternalRef::new("agent-session:secret-deliver")
            .map_err(WorkcellError::from)?,
        workload_ref: None,
        class: class.clone(),
        purpose: record.purpose.clone(),
        destination: "opensandbox-egress".into(),
        scope: record.scope.clone(),
    };
    let mut broker_routes = Vec::new();
    for route in &routes {
        let (host, method) = route.split_once('=').ok_or_else(|| {
            WorkcellError::InvalidDemand(format!(
                "route `{route}` must be HOST=METHOD (e.g. api.github.com=GET)"
            ))
        })?;
        broker_routes.push(BrokerRoute {
            destination_host: host.trim().to_owned(),
            method: method.trim().to_uppercase(),
            purpose: record.purpose.clone(),
            scope: record.scope.clone(),
        });
    }
    let policy = BrokerPolicy::new(broker_routes.clone())?;
    let route = broker_routes[0].clone();
    let handle = broker_handle(&request)?;
    let binding = OpenSandboxCredentialBindingSpec::https(
        credential_name.clone(),
        binding_name.clone(),
        request_paths,
        binding_auth,
    )?;
    let allocation_stub = ProviderAllocation {
        provider_ref: epilogos_workcell_core::ProviderRef::new(&target_provider_ref)
            .map_err(WorkcellError::from)?,
        port: ProviderPortKind::Execution,
        material_ref: allocation.clone(),
        health: HealthState::Healthy,
        properties: BTreeMap::new(),
        provenance: BTreeMap::new(),
    };

    let receipt = project_credential_to_sandbox(
        &broker,
        &projection,
        SecretRevocationState::Active,
        &allocation_stub,
        &origin_provider,
        &request,
        &policy,
        &handle,
        &route,
        &binding,
    )?;

    let vault_revision = receipt
        .provenance
        .get("sandbox.vault_revision")
        .cloned()
        .unwrap_or_default();
    if global.json {
        emit_json(json!({
            "ok": true,
            "projection_ref": projection_ref,
            "sandbox": allocation,
            "credential_name": credential_name,
            "binding_name": binding_name,
            "vault_revision": vault_revision,
            "secret_visibility": "use-without-read",
        }));
    } else {
        println!(
            "delivered `{projection_ref}` into sandbox `{allocation}` as vault credential `{credential_name}` (binding `{binding_name}`, revision {vault_revision}); use-without-read — the material was resolved in-process and never printed"
        );
    }
    Ok(())
}

fn secret_scan(global: &GlobalArgs, _args: &[String]) -> Result<(), WorkcellError> {
    let home = env::var("HOME").map_err(|_| {
        WorkcellError::Unavailable(
            "cannot determine the home directory (HOME is unset); the standard exposure scan needs it"
                .into(),
        )
    })?;
    let env_pairs: Vec<(String, String)> = env::vars().collect();
    let report = secret_scan::run_standard_scan(
        Path::new(&home),
        env_pairs
            .iter()
            .map(|(key, value)| (key.as_str(), value.as_str())),
    );
    if global.json {
        emit_json(report.to_json());
    } else {
        print!("{}", report.render_plain());
        if !report.findings.is_empty() {
            println!(
                "vault a candidate into the origin store:\n  workcell secret vault --from <location> [--select <json.key>] --ref <provider ref>"
            );
        }
    }
    Ok(())
}

fn secret_vault(global: &GlobalArgs, args: &[String]) -> Result<(), WorkcellError> {
    let mut from: Option<String> = None;
    let mut select: Option<String> = None;
    let mut provider_name: Option<String> = None;
    let mut reference: Option<String> = None;
    let mut index = 0;
    while index < args.len() {
        match args[index].as_str() {
            "--from" => {
                from = Some(require_value(args, index, "--from")?.to_owned());
                index += 2;
            }
            "--select" => {
                select = Some(require_value(args, index, "--select")?.to_owned());
                index += 2;
            }
            "--provider" => {
                provider_name = Some(require_value(args, index, "--provider")?.to_owned());
                index += 2;
            }
            "--ref" => {
                reference = Some(require_value(args, index, "--ref")?.to_owned());
                index += 2;
            }
            other => {
                return Err(WorkcellError::InvalidDemand(format!(
                    "unknown vault option `{other}`"
                )))
            }
        }
    }
    let from = from.ok_or_else(|| {
        WorkcellError::InvalidDemand(
            "vault needs a source: --from env:NAME or --from /path/to/file".into(),
        )
    })?;
    let reference = reference.ok_or_else(|| {
        WorkcellError::InvalidDemand(
            "vault needs a destination ref: --ref linux-secret-service://<service>/<account> (or keychain://… on macOS)".into(),
        )
    })?;
    let provider = provider_name.unwrap_or_else(default_origin_provider);

    let material = read_vault_material(&from, select.as_deref())?;
    let stored = store_in_origin(&provider, &reference, &material)?;
    if global.json {
        emit_json(json!({
            "ok": true,
            "stored": stored,
            "provider": provider,
            "bytes": material.len(),
        }));
    } else {
        println!(
            "stored {} bytes into `{provider}` as {stored}; the material was never printed",
            material.len()
        );
    }
    Ok(())
}

fn read_vault_material(from: &str, select: Option<&str>) -> Result<Vec<u8>, WorkcellError> {
    if let Some(name) = from.strip_prefix("env:") {
        if select.is_some() {
            return Err(WorkcellError::InvalidDemand(
                "--select applies to file sources only".into(),
            ));
        }
        let value = env::var(name).map_err(|_| {
            WorkcellError::Unavailable(format!(
                "environment variable `{name}` is not set; nothing was vaulted"
            ))
        })?;
        if value.trim().is_empty() {
            return Err(WorkcellError::InvalidDemand(
                "refusing to vault empty material".into(),
            ));
        }
        return Ok(value.into_bytes());
    }
    let text = fs::read_to_string(from).map_err(|error| {
        WorkcellError::Unavailable(format!("could not read `{from}`: {error}; nothing was vaulted"))
    })?;
    if let Some(select) = select {
        return Ok(select_json_path(&text, select)?.into_bytes());
    }
    // Whole-file material (a PEM key, a token file) is legitimate vault
    // material. A JSON object without --select is refused: the real
    // material sits in one field, and storing the whole file would bury it.
    if serde_json::from_str::<Value>(&text)
        .map(|value| value.is_object())
        .unwrap_or(false)
    {
        return Err(WorkcellError::InvalidDemand(format!(
            "`{from}` is a JSON object; pass --select <dotted.key> to name the field that holds the material"
        )));
    }
    let trimmed = text.trim();
    if trimmed.is_empty() {
        return Err(WorkcellError::InvalidDemand(
            "refusing to vault empty material".into(),
        ));
    }
    Ok(trimmed.as_bytes().to_vec())
}

fn select_json_path(text: &str, select: &str) -> Result<String, WorkcellError> {
    let value: Value = serde_json::from_str(text)
        .map_err(|error| WorkcellError::Unavailable(format!("the source is not valid JSON: {error}")))?;
    let mut current = &value;
    for segment in select.split('.') {
        let (key, index) = match segment.split_once('[') {
            Some((key, rest)) => {
                let index = rest.trim_end_matches(']').parse::<usize>().ok();
                (key, index)
            }
            None => (segment, None),
        };
        current = current.get(key).ok_or_else(|| {
            WorkcellError::Unavailable(format!(
                "`{select}` does not name a field in the source; nothing was vaulted"
            ))
        })?;
        if let Some(index) = index {
            current = current.get(index).ok_or_else(|| {
                WorkcellError::Unavailable(format!(
                    "`{select}` does not name an array element in the source; nothing was vaulted"
                ))
            })?;
        }
    }
    match current.as_str() {
        Some(text) if !text.trim().is_empty() => Ok(text.to_owned()),
        _ => Err(WorkcellError::Unavailable(format!(
            "`{select}` does not name a non-empty string field; nothing was vaulted"
        ))),
    }
}

fn default_origin_provider() -> String {
    #[cfg(target_os = "linux")]
    {
        "secret-service".to_owned()
    }
    #[cfg(not(target_os = "linux"))]
    {
        "keychain".to_owned()
    }
}

fn store_in_origin(
    provider_name: &str,
    reference: &str,
    material: &[u8],
) -> Result<String, WorkcellError> {
    let external = ExternalRef::new(reference).map_err(WorkcellError::from)?;
    match provider_name {
        "secret-service" => {
            let provider = SecretServiceSecretProvider::new()?;
            store_secret_service_material(&provider, &external, material)?;
        }
        "keychain" => {
            let provider = KeychainSecretProvider::new(KeychainAclPolicy::ThisDeviceUnlocked)?;
            store_keychain_material(&provider, &external, material)?;
        }
        other => {
            return Err(WorkcellError::InvalidDemand(format!(
                "unknown secret provider `{other}`; known providers: secret-service, keychain"
            )))
        }
    }
    Ok(reference.to_owned())
}

#[allow(clippy::too_many_arguments)]
fn secret_project(global: &GlobalArgs, args: &[String]) -> Result<(), WorkcellError> {
    let mut name: Option<String> = None;
    let mut to_workcell: Option<String> = None;
    let mut connection: Option<String> = None;
    let mut to_sandbox = false;
    let mut allocation_ref: Option<String> = None;
    let mut sandbox_provider: Option<String> = None;
    let mut credential_ref: Option<String> = None;
    let mut source_provider: Option<String> = None;
    let mut class: Option<String> = None;
    let mut purpose: Option<String> = None;
    let mut scope: Option<String> = None;
    let mut requested_by: Option<String> = None;
    let mut index = 0;
    while index < args.len() {
        match args[index].as_str() {
            "--name" => {
                name = Some(require_value(args, index, "--name")?.to_owned());
                index += 2;
            }
            "--to-workcell" => {
                to_workcell = Some(require_value(args, index, "--to-workcell")?.to_owned());
                index += 2;
            }
            "--connection" => {
                connection = Some(require_value(args, index, "--connection")?.to_owned());
                index += 2;
            }
            "--to-sandbox" => {
                to_sandbox = true;
                index += 1;
            }
            "--allocation-ref" => {
                allocation_ref = Some(require_value(args, index, "--allocation-ref")?.to_owned());
                index += 2;
            }
            "--sandbox-provider" => {
                sandbox_provider =
                    Some(require_value(args, index, "--sandbox-provider")?.to_owned());
                index += 2;
            }
            "--credential-ref" => {
                credential_ref = Some(require_value(args, index, "--credential-ref")?.to_owned());
                index += 2;
            }
            "--source-provider" => {
                source_provider =
                    Some(require_value(args, index, "--source-provider")?.to_owned());
                index += 2;
            }
            "--class" => {
                class = Some(require_value(args, index, "--class")?.to_owned());
                index += 2;
            }
            "--purpose" => {
                purpose = Some(require_value(args, index, "--purpose")?.to_owned());
                index += 2;
            }
            "--scope" => {
                scope = Some(require_value(args, index, "--scope")?.to_owned());
                index += 2;
            }
            "--by" => {
                requested_by = Some(require_value(args, index, "--by")?.to_owned());
                index += 2;
            }
            other => {
                return Err(WorkcellError::InvalidDemand(format!(
                    "unknown project option `{other}`"
                )))
            }
        }
    }
    // A workcell-target projection would grant a materialisation no code path
    // can perform: `secret deliver` refuses workcell targets, the sandbox
    // projection seam refuses them, and the control plane has no workcell
    // materialisation operation. Recording one would mint a durable grant the
    // world cannot ever honour, so the creation path refuses here. The ledger
    // types are untouched; sandbox-target projections remain the
    // materialisable path.
    if to_workcell.is_some() {
        return Err(WorkcellError::Unsupported(
            "the projection target is not yet materialisable: no code path can carry a \
             projection to a workcell target (delivery, the control plane and the sandbox \
             projection seam all refuse it), so recording one would create a durable grant \
             that can never be materialised. Sandbox-target projections (`--to-sandbox`) \
             remain the materialisable path. Already-recorded projections can be inspected \
             with `workcell secret projections` and released with \
             `workcell secret revoke-projection`"
                .into(),
        ));
    }
    let name = name.ok_or_else(|| {
        WorkcellError::InvalidDemand("project needs --name LABEL for the projection ref".into())
    })?;
    validate_label(&name)?;
    let credential_ref = credential_ref.ok_or_else(|| {
        WorkcellError::InvalidDemand("project needs --credential-ref REF".into())
    })?;
    let source_provider = source_provider.ok_or_else(|| {
        WorkcellError::InvalidDemand("project needs --source-provider REF".into())
    })?;
    let class = class.ok_or_else(|| {
        WorkcellError::InvalidDemand(
            "project needs --class CLASS (e.g. credential-broker, file, process-env)".into(),
        )
    })?;
    let class = SecretMaterialisationClass::parse(&class).ok_or_else(|| {
        WorkcellError::InvalidDemand(format!(
            "unknown materialisation class `{class}`; known classes: process-env, one-shot-child-process, fd-or-pipe, file, provider-native-lease, credential-broker, short-lived-federated-credential"
        ))
    })?;
    let purpose = purpose.ok_or_else(|| {
        WorkcellError::InvalidDemand("project needs --purpose PURPOSE".into())
    })?;
    let scope =
        scope.ok_or_else(|| WorkcellError::InvalidDemand("project needs --scope SCOPE".into()))?;
    let requested_by = requested_by.ok_or_else(|| {
        WorkcellError::InvalidDemand("project needs --by REQUESTER-REF".into())
    })?;

    let target = match (to_workcell, to_sandbox) {
        (Some(workcell_ref), false) => {
            SecretProjectionTarget::Workcell {
                workcell_ref: parse_workcell_ref(&workcell_ref)?,
                connection_label: connection.ok_or_else(|| {
                    WorkcellError::InvalidDemand(
                        "a workcell projection needs --connection LABEL (the authorised cross-cell relation)"
                            .into(),
                    )
                })?,
            }
        }
        (None, true) => SecretProjectionTarget::Sandbox {
            provider_ref: epilogos_workcell_core::ProviderRef::new(
                sandbox_provider.as_deref().unwrap_or("provider:opensandbox"),
            )
            .map_err(WorkcellError::from)?,
            allocation_ref: allocation_ref.ok_or_else(|| {
                WorkcellError::InvalidDemand(
                    "a sandbox projection needs --allocation-ref REF".into(),
                )
            })?,
        },
        (Some(_), true) => {
            return Err(WorkcellError::InvalidDemand(
                "a projection has one target: --to-workcell or --to-sandbox, not both".into(),
            ))
        }
        (None, false) => {
            return Err(WorkcellError::InvalidDemand(
                "project needs a target: --to-workcell REF --connection LABEL, or --to-sandbox --allocation-ref REF"
                    .into(),
            ))
        }
    };

    let request = SecretProjectionRequest {
        credential_ref: ExternalRef::new(&credential_ref).map_err(WorkcellError::from)?,
        source_provider_ref: epilogos_workcell_core::ProviderRef::new(&source_provider)
            .map_err(WorkcellError::from)?,
        target,
        class,
        purpose,
        scope,
        requested_by: ExternalRef::new(&requested_by).map_err(WorkcellError::from)?,
    };
    request.validate()?;

    let workcell_ref = parse_workcell_ref(&global.workcell_ref)?;
    let ledger = SecretProjectionLedger::new(&global.state_root, workcell_ref);
    let projection_ref = format!("secret-projection:{name}");
    let record =
        SecretProjectionRecord::from_request(projection_ref.clone(), &request, now_unix_ms())?;
    ledger.record(record)?;
    if global.json {
        emit_json(json!({
            "ok": true,
            "projection_ref": projection_ref,
            "ledger": ledger.path().display().to_string(),
        }));
    } else {
        println!(
            "projection `{projection_ref}` recorded in {}; refs only — the credential stays in its origin store",
            ledger.path().display()
        );
    }
    Ok(())
}

fn secret_projections(global: &GlobalArgs, _args: &[String]) -> Result<(), WorkcellError> {
    let workcell_ref = parse_workcell_ref(&global.workcell_ref)?;
    let ledger = SecretProjectionLedger::new(&global.state_root, workcell_ref);
    let records = ledger.list()?;
    if global.json {
        emit_json(json!({
            "ok": true,
            "projections": records.iter().map(|record| record.to_json()).collect::<Vec<_>>(),
        }));
    } else {
        if records.is_empty() {
            println!("no secret projections recorded in {}", ledger.path().display());
        }
        for record in records {
            let target = match (&record.target_workcell_ref, &record.target_provider_ref) {
                (Some(workcell), _) => format!(
                    "workcell {workcell} (connection `{}`)",
                    record.target_connection_label.clone().unwrap_or_default()
                ),
                (None, Some(provider)) => format!(
                    "sandbox {provider}/{}",
                    record.target_allocation_ref.clone().unwrap_or_default()
                ),
                _ => "unknown target".to_owned(),
            };
            println!(
                "{} -> {} class={} purpose={} scope={} state={}",
                record.projection_ref, target, record.class, record.purpose, record.scope, record.state
            );
        }
    }
    Ok(())
}

fn secret_revoke_projection(global: &GlobalArgs, args: &[String]) -> Result<(), WorkcellError> {
    let mut name: Option<String> = None;
    let mut projection_ref: Option<String> = None;
    let mut sandbox: Option<String> = None;
    let mut index = 0;
    while index < args.len() {
        match args[index].as_str() {
            "--name" => {
                name = Some(require_value(args, index, "--name")?.to_owned());
                index += 2;
            }
            "--ref" => {
                projection_ref = Some(require_value(args, index, "--ref")?.to_owned());
                index += 2;
            }
            "--sandbox" => {
                sandbox = Some(require_value(args, index, "--sandbox")?.to_owned());
                index += 2;
            }
            other => {
                return Err(WorkcellError::InvalidDemand(format!(
                    "unknown revoke-projection option `{other}`"
                )))
            }
        }
    }
    let projection_ref = match (name, projection_ref) {
        (Some(name), None) => format!("secret-projection:{name}"),
        (None, Some(reference)) => reference,
        (Some(_), Some(_)) => {
            return Err(WorkcellError::InvalidDemand(
                "pass --name LABEL or --ref PREF, not both".into(),
            ))
        }
        (None, None) => {
            return Err(WorkcellError::InvalidDemand(
                "revoke-projection needs --name LABEL or --ref PREF".into(),
            ))
        }
    };
    let workcell_ref = parse_workcell_ref(&global.workcell_ref)?;
    let ledger = SecretProjectionLedger::new(&global.state_root, workcell_ref);
    let (was_active, record) = match ledger.decision_for(&projection_ref)? {
        ProjectionDecision::Unknown => {
            return Err(WorkcellError::InvalidDemand(format!(
                "no secret projection `{projection_ref}` exists in this ledger"
            )))
        }
        ProjectionDecision::Revoked(record) => (false, Some(*record)),
        ProjectionDecision::Active(record) => {
            ledger.revoke(&projection_ref, now_unix_ms())?;
            (true, Some(*record))
        }
    };

    // Delete the vault at the projected target so injected material stops
    // being injected at its next outbound flow — revocation that reaches the
    // target, not a ledger entry alone.
    let mut vault_deleted = false;
    if let Some(allocation) = &sandbox {
        let record = record.ok_or_else(|| {
            WorkcellError::InvalidDemand(format!(
                "no secret projection `{projection_ref}` exists in this ledger"
            ))
        })?;
        let (target_provider_ref, _) = match &record.to_request()?.target {
            SecretProjectionTarget::Sandbox {
                provider_ref,
                allocation_ref,
            } => (provider_ref.as_str().to_owned(), allocation_ref.clone()),
            SecretProjectionTarget::Workcell { .. } => {
                return Err(WorkcellError::InvalidDemand(
                    "--sandbox deletes a sandbox vault; this projection names a workcell target"
                        .into(),
                ))
            }
        };
        let config = declared_sandbox_deployment(global, &target_provider_ref)?;
        let broker = OpenSandboxCredentialBroker::new(config, StdHttpOpenSandboxTransport)?;
        let allocation_stub = ProviderAllocation {
            provider_ref: epilogos_workcell_core::ProviderRef::new(&target_provider_ref)
                .map_err(WorkcellError::from)?,
            port: ProviderPortKind::Execution,
            material_ref: allocation.clone(),
            health: HealthState::Healthy,
            properties: BTreeMap::new(),
            provenance: BTreeMap::new(),
        };
        broker.delete_vault(&allocation_stub)?;
        vault_deleted = true;
    }

    if global.json {
        emit_json(json!({
            "ok": true,
            "projection_ref": projection_ref,
            "state": "revoked",
            "vault_deleted": vault_deleted,
            "note": if !was_active && !vault_deleted { "already revoked" } else { "" },
        }));
    } else {
        let vault_note = if vault_deleted {
            format!(
                "; the sandbox `{}` vault was deleted, so injected material is no longer injected",
                sandbox.clone().unwrap_or_default()
            )
        } else {
            String::new()
        };
        if !was_active && !vault_deleted {
            println!("{projection_ref} was already revoked; the record is kept as audit evidence");
        } else {
            println!(
                "{projection_ref} revoked; the projected target is refused at its next use{vault_note}, and the record is kept as audit evidence"
            );
        }
    }
    Ok(())
}

fn print_help() {
    println!("CAW material operations: inspect / recover --receipt FILE; plan / prepare --demand-json FILE (full native demand including storage). Write restrictions: workcell-write-boundary capabilities / inspect / run. These do not create semantic sessions or execute Factory work.");
    println!(
        "Workcell — provider-neutral material execution control\n\n\
Usage:\n  workcell [global options] <command> [command options]\n\n\
Commands:\n  status       Summarise this local Workcell\n  discover     Discover material offers\n  plan         Plan an ExecutionDemand\n  prepare      Prepare a material world and persist a receipt\n  observe      Observe a prepared world from its receipt\n  expose       Resolve prepared exposure surfaces\n  collect      Collect prepared output channels\n  release      Release or preserve a prepared world\n  reconcile    Reconcile desired material state\n  correlate-projection\n               Carry an AIKit git-projection verdict as a correlated observation (Workcell runs no git)\n  instances    Live harness instances and bounded resource usage
  places       Census of persistent process places (tmux, herdr) — read-only
  place        Place request/release over those places (request --provider auto|herdr|tmux --name SLUG; release --place-ref REF --pid N --start-marker PS_LSTART)
  sandboxes    OpenSandbox server-side material (reconcile)\n  connections  Cross-cell connection records (list/show/disconnect)\n  providers    List provider inventory\n  system       Emit this Workcell's System settings disclosure (oi.product-settings-disclosure/v2)\n  config-contribution\n               Emit Workcell's configuration contribution (oi.configuration-contribution/v1)\n  config       Owner-native configuration transport: validate | plan | apply | reset\n  doctor       Verify the zero-setup local baseline\n\n\
Cross-cell connection lifecycle:\n  workcell serve --listen HOST:PORT [--authorization TOKEN]\n      Serve this cell's control plane; grants are enforced per request.\n      Non-loopback listeners require a token or at least one active grant.\n  workcell authorise --client <label> --allow <operation>... [--advertise <port>...] [--expires-in <duration>] [--store-credential]\n      Grant a connecting client named operations; the credential is shown once.\n      `--expires-in 30m` (s|m|h|d) makes the grant expire at the next use,\n      refused as expired; without it the grant never expires.\n  workcell revoke --client <label> | --grant <ref>\n      Revoke grants; takes effect at the connecting client's next use.\n  workcell connect --endpoint HOST:PORT [--connection <label>] [--authorization TOKEN] [--store-credential]\n      Establish or reconnect the client side; reports both cells' protocol\n      and software, refuses unsupported combinations loudly.\n\n\
Secret origin and projection:\n  workcell secret scan [--json]
      Detect credential material outside a secret provider (env vars, shell\n      rc files, known auth files); reports location + presence only.
  workcell secret vault --from env:NAME|PATH [--select KEY] --ref REF [--provider secret-service|keychain]
      Move material into this machine's origin store; the value is never printed.
  workcell machine add --label LABEL --endpoint HOST:PORT [--credential-ref REF]
      Declare a remote machine natively; then `workcell connect --connection LABEL`
      needs no flags — the endpoint and credential reference come from the
      declaration, and the material resolves from the origin secret source.
  workcell machine list | remove --label LABEL
  workcell secret project --name LABEL (--to-workcell REF --connection LABEL | --to-sandbox --allocation-ref REF)
      --credential-ref REF --source-provider REF --class CLASS --purpose P --scope S --by REF
      Record an authorised projection at the origin (refs only, never material).
  workcell secret projections | revoke-projection (--name LABEL | --ref PREF)
      Read the origin's projection ledger; revocation reaches the projected\n      target at its next use.\n\n\
Global options:\n  --json                     Structured machine/agent output\n  --state-root PATH          Local Workcell state (default: $WORKCELL_HOME or ~/.workcell)\n  --workcell-ref REF         Workcell identity for new local operations\n  --receipt PATH             Material-world receipt for prepare/resume\n  --workspace-source PATH    Physical local source binding; never semantic identity\n  --services PATH            Operator-declared logical services (default: <state-root>/services.json)\n\n\
Demand options for plan/prepare:\n  --demand-ref REF\n  --require VALUE | --prefer VALUE | --optional VALUE\n  --workspace writable|read-only [--workspace-ref REF] [--revision REV]\n  --project-runtime MODE\n  --connect VALUE | --prefer-connect VALUE | --optional-connect VALUE\n  --expose VALUE | --prefer-expose VALUE | --optional-expose VALUE\n  --output VALUE | --prefer-output VALUE | --optional-output VALUE\n  --resource key[=amount[:unit]]\n  --subject role=opaque-ref\n  --persistence SCOPE\n  --isolation VALUE\n  --retention release|preserve|suspend-if-supported|snapshot-if-supported\n  --extension key=value\n\n\
Reconcile:\n  workcell --receipt WORLD.json reconcile --desired logical-ref=state\n\n\
Projection correlation:\n  workcell --receipt WORLD.json correlate-projection --projection AIKIT.json --subject checkout:KEY\n      Carry an AIKit worktree-projection verdict (aikit worktree project --json)\n      as a correlated observation on the world's checkout subject. Workcell runs\n      no git and re-derives nothing — the verdict is AIKit's, carried verbatim."
    );
}
