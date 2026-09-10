use std::{
    collections::{BTreeMap, BTreeSet},
    env, fs,
    path::{Path, PathBuf},
    process::ExitCode,
    time::{SystemTime, UNIX_EPOCH},
};

use epilogos_workcell_core::{
    AffordanceRequirement, Availability, CollectionBundle, Degradation, DemandRef,
    DesiredMaterialState, Discovery, ExecutionDemand, ExposureBundle, ExposureRequirement,
    ExternalRef, HealthState, IsolationTrustRequirement, LogicalConnectionRequirement,
    MaterialisationPlan, ObservationBundle, OutputRequirement, PersistenceScope, PlanOmission,
    PlanStatus, ProjectRuntimeRequirement, ProviderPortKind, ReconciliationResult,
    ReleaseDisposition, ReleaseResult, RequirementNecessity, ResourceRequirement,
    RetentionExpectation, Tiered, WorkcellControlPlane, WorkcellError, WorkcellRef,
    WorkspaceAccess, WorkspaceRequirement,
};
use epilogos_workcell_runtime::{CollapsedLocalConfig, CollapsedLocalWorkcell};
use epilogos_workcell_wire::{decode_world, encode_world, world_value};
use serde_json::{json, Value};

const DEFAULT_WORKCELL_REF: &str = "workcell:local";
const DEFAULT_DEMAND_REF: &str = "demand:cli";

#[derive(Debug)]
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
        "expose" => command_expose(&global),
        "material" => command_material(&global),
        "collect" => command_collect(&global),
        "release" => command_release(&global),
        "reconcile" => command_reconcile(&global, command_args),
        "instances" => command_instances(&global, command_args),
        "sandboxes" => command_sandboxes(&global, command_args),
        "system" => command_system(&global),
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
    if global.json {
        emit_json(json!({
            "ok": true,
            "workcell_ref": discovery.workcell_ref.as_str(),
            "health": health(&discovery.health),
            "providers": provider_count(&discovery),
            "offers": discovery.offers.len(),
            "persisted_world_receipts": receipts,
            "state_root": global.state_root,
        }));
    } else {
        println!("Workcell {}", discovery.workcell_ref);
        println!("health: {}", health(&discovery.health));
        println!("providers: {}", provider_count(&discovery));
        println!("offers: {}", discovery.offers.len());
        println!("persisted worlds: {receipts}");
        println!("state root: {}", global.state_root.display());
    }
    Ok(())
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

fn parse_demand(args: &[String]) -> Result<ExecutionDemand, WorkcellError> {
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
    let mut api_key = None;
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
            "--api-key" => {
                api_key = Some(require_value(args, index, "--api-key")?.to_owned());
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
            "usage: workcell sandboxes reconcile --server <url> [--api-key-env <ENV>] [--api-key <key>] [--release-orphans] [--release <id>...] [--include-snapshots]".into(),
        ));
    };
    // Resolution order: literal key wins; then the named environment variable;
    // otherwise no key is sent (servers without auth accept that).
    let api_key = api_key.or_else(|| {
        std::env::var(&api_key_env)
            .ok()
            .filter(|value| !value.is_empty())
    });

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
        "remote Workcell settings disclosure through workcell.control/v1 is not yet implemented; `workcell --endpoint ... system` reports unavailable rather than a fabricated remote reading",
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

fn print_help() {
    println!(
        "Workcell — provider-neutral material execution control\n\n\
Usage:\n  workcell [global options] <command> [command options]\n\n\
Commands:\n  status       Summarise this local Workcell\n  discover     Discover material offers\n  plan         Plan an ExecutionDemand\n  prepare      Prepare a material world and persist a receipt\n  observe      Observe a prepared world from its receipt\n  expose       Resolve prepared exposure surfaces\n  collect      Collect prepared output channels\n  release      Release or preserve a prepared world\n  reconcile    Reconcile desired material state\n  instances    Live harness instances and bounded resource usage
  sandboxes    OpenSandbox server-side material (reconcile)\n  providers    List provider inventory\n  system       Emit this Workcell's System settings disclosure (oi.product-settings-disclosure/v2)\n  doctor       Verify the zero-setup local baseline\n\n\
Global options:\n  --json                     Structured machine/agent output\n  --state-root PATH          Local Workcell state (default: $WORKCELL_HOME or ~/.workcell)\n  --workcell-ref REF         Workcell identity for new local operations\n  --receipt PATH             Material-world receipt for prepare/resume\n  --workspace-source PATH    Physical local source binding; never semantic identity\n  --services PATH            Operator-declared logical services (default: <state-root>/services.json)\n\n\
Demand options for plan/prepare:\n  --demand-ref REF\n  --require VALUE | --prefer VALUE | --optional VALUE\n  --workspace writable|read-only [--workspace-ref REF] [--revision REV]\n  --project-runtime MODE\n  --connect VALUE | --prefer-connect VALUE | --optional-connect VALUE\n  --expose VALUE | --prefer-expose VALUE | --optional-expose VALUE\n  --output VALUE | --prefer-output VALUE | --optional-output VALUE\n  --resource key[=amount[:unit]]\n  --subject role=opaque-ref\n  --persistence SCOPE\n  --isolation VALUE\n  --retention release|preserve|suspend-if-supported|snapshot-if-supported\n  --extension key=value\n\n\
Reconcile:\n  workcell --receipt WORLD.json reconcile --desired logical-ref=state"
    );
}
