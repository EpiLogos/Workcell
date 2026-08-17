use std::{
    collections::{BTreeMap, BTreeSet},
    env, fs,
    path::{Path, PathBuf},
    process::ExitCode,
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
        "collect" => command_collect(&global),
        "release" => command_release(&global),
        "reconcile" => command_reconcile(&global, command_args),
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

    if global.json {
        emit_json(json!({
            "ok": true,
            "state_root_writable": true,
            "shell": shell,
            "writable_workspace": writable_workspace,
            "filesystem_artifacts": filesystem_artifacts,
            "optional_external_providers_required": false,
        }));
    } else {
        println!("doctor: healthy");
        println!("  state root writable: yes");
        println!("  host-process execution: yes");
        println!("  writable local workspace: yes");
        println!("  local artifact storage: yes");
        println!("  Docker/Arrakis/Tailscale required: no");
    }
    Ok(())
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
    let mut config = CollapsedLocalConfig::new(world.workcell_ref.clone(), &global.state_root);
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
    let mut config = CollapsedLocalConfig::new(workcell_ref, &global.state_root);
    if let Some(source) = &global.workspace_source {
        config = config.with_workspace_source(source);
    }
    let mut channels = BTreeSet::from(["logs:run".to_owned(), "artifacts:run".to_owned()]);
    channels.extend(additional_channels);
    config.artifact_channels = channels.into_iter().collect();
    CollapsedLocalWorkcell::new(config)
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
Commands:\n  status       Summarise this local Workcell\n  discover     Discover material offers\n  plan         Plan an ExecutionDemand\n  prepare      Prepare a material world and persist a receipt\n  observe      Observe a prepared world from its receipt\n  expose       Resolve prepared exposure surfaces\n  collect      Collect prepared output channels\n  release      Release or preserve a prepared world\n  reconcile    Reconcile desired material state\n  providers    List provider inventory\n  doctor       Verify the zero-setup local baseline\n\n\
Global options:\n  --json                     Structured machine/agent output\n  --state-root PATH          Local Workcell state (default: $WORKCELL_HOME or ~/.workcell)\n  --workcell-ref REF         Workcell identity for new local operations\n  --receipt PATH             Material-world receipt for prepare/resume\n  --workspace-source PATH    Physical local source binding; never semantic identity\n\n\
Demand options for plan/prepare:\n  --demand-ref REF\n  --require VALUE | --prefer VALUE | --optional VALUE\n  --workspace writable|read-only [--workspace-ref REF] [--revision REV]\n  --project-runtime MODE\n  --connect VALUE | --prefer-connect VALUE | --optional-connect VALUE\n  --expose VALUE | --prefer-expose VALUE | --optional-expose VALUE\n  --output VALUE | --prefer-output VALUE | --optional-output VALUE\n  --resource key[=amount[:unit]]\n  --subject role=opaque-ref\n  --persistence SCOPE\n  --isolation VALUE\n  --retention release|preserve|suspend-if-supported|snapshot-if-supported\n  --extension key=value\n\n\
Reconcile:\n  workcell --receipt WORLD.json reconcile --desired logical-ref=state"
    );
}
