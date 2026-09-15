//! Place census: the persistent process *places* on this machine.
//!
//! The harness-instance census (`instance_scan`) sees pids; it cannot see the
//! rooms those pids run in. A tmux session or a Herdr workspace outlives its
//! processes and is itself material evidence: an addressable location a later
//! operator can observe, request, and release. This module inventories those
//! places from the local tmux and Herdr providers.
//!
//! Place law: a place is material evidence and addressable location — never
//! caller identity. A recycled pane id or session name is a new place;
//! identity claims travel in persisted context, not in pane names. Where the
//! census observes a place key bound to more than one process generation, it
//! names the finding and refuses to merge it into a continuity claim — the
//! same start-marker discipline `instance_scan` applies to pids.
//!
//! The census is READ-ONLY. No tmux or Herdr command on this path may create,
//! kill, or modify anything: only `tmux -V`, `tmux list-panes`, `herdr
//! --version`, `herdr workspace list`, `herdr pane list` and `herdr pane
//! process-info` are ever spawned here. A provider that is installed but not
//! running is a disclosed degraded state (`present-no-server`,
//! `present-server-unreachable`) with its exact stderr — honest absence, not
//! an error and never an empty set read as proof.

use std::collections::BTreeMap;
use std::process::{Command, Stdio};
use std::time::{SystemTime, UNIX_EPOCH};

use serde_json::{json, Value};

use crate::instance_scan::{read_pid_table, ObservedProcess, PID_ALIASES};

pub const PLACE_CENSUS_VERSION: &str = "workcell.place-census/v1";

pub const TMUX_PROVIDER: &str = "tmux";
pub const HERDR_PROVIDER: &str = "herdr";

/// Provider statuses. Every state is either a completed enumeration (`ok`) or
/// a named, evidenced degradation — never a silent empty set.
pub const PLACE_PROVIDER_OK: &str = "ok";
pub const PLACE_PROVIDER_ABSENT: &str = "absent";
pub const PLACE_PROVIDER_NO_SERVER: &str = "present-no-server";
pub const PLACE_PROVIDER_SERVER_UNREACHABLE: &str = "present-server-unreachable";
pub const PLACE_PROVIDER_CLI_UNPARSED: &str = "present-cli-unparsed";
pub const PLACE_PROVIDER_ERROR: &str = "error";

/// Pane classifications. `other` is the point of the census: panes running
/// processes Workcell/Factory did not launch are seen and reported, never
/// killed and never hidden.
pub const PLACE_CLASS_HARNESS: &str = "harness";
pub const PLACE_CLASS_SELF: &str = "self";
pub const PLACE_CLASS_OTHER: &str = "other";
pub const PLACE_CLASS_UNOBSERVED: &str = "unobserved";

/// Command stems owned by this suite. A pane running one of these was
/// launched by Workcell/Factory itself, not by an outside operator.
pub const SELF_BINARY_STEMS: [&str; 2] = ["workcell", "factory"];

/// The tmux `list-panes` format: one tab-separated row per pane, default
/// socket only (`-a` across all sessions). Field order is the parser contract.
/// tmux 3.7 escapes control characters in format expansion (a literal tab
/// byte comes back as the two characters `\t` — found live on Omarchy during
/// the commissioned TM02-R re-test), so the row separator is `:`: no field
/// but an arbitrary user window name can carry one, and a window name with a
/// colon is skipped and counted as malformed, never fatal.
pub const TMUX_PANE_FORMAT: &str = "#{session_name}:#{session_created}:#{window_index}:#{window_name}:#{pane_id}:#{pane_pid}:#{pane_current_command}:#{pane_dead}:#{pane_tty}";

/// One parsed tmux pane row. Every field is a defensive `Option`/default:
/// tmux renders what it has, and a pane with no live pid (a dead pane) is
/// still a place observation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TmuxPaneRow {
    pub session_name: String,
    pub session_created: String,
    pub window_index: String,
    pub window_name: String,
    pub pane_id: String,
    pub pane_pid: Option<u32>,
    pub pane_current_command: String,
    pub pane_dead: bool,
    pub pane_tty: String,
}

/// One parsed Herdr workspace (`herdr workspace list`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HerdrWorkspaceRow {
    pub workspace_id: String,
    pub label: String,
    pub active_tab_id: Option<String>,
    pub tab_count: Option<u64>,
    pub pane_count: Option<u64>,
}

/// One parsed Herdr pane (`herdr pane list`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HerdrPaneRow {
    pub pane_id: String,
    pub workspace_id: Option<String>,
    pub tab_id: Option<String>,
    pub cwd: Option<String>,
}

/// One parsed Herdr pane process reading (`herdr pane process-info`).
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct HerdrProcessInfo {
    pub shell_pid: Option<u32>,
    pub foreground_name: Option<String>,
}

/// One census row: a place observation joined with the process generation
/// evidence (`ps lstart` start marker) of the pid it currently hosts.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PaneObservation {
    pub provider: &'static str,
    /// Place key that is stable within this census (e.g. tmux
    /// `<session>/<pane_id>`, herdr `<workspace_id>/<pane_id>`). A reused key
    /// with a different process generation is a named finding, never a
    /// continuity claim.
    pub place_key: String,
    pub session_name: Option<String>,
    pub session_created: Option<String>,
    pub window_index: Option<String>,
    pub window_name: Option<String>,
    pub pane_id: String,
    pub pane_pid: Option<u32>,
    pub pane_command: Option<String>,
    pub pane_dead: bool,
    pub pane_tty: Option<String>,
    pub workspace_id: Option<String>,
    pub workspace_label: Option<String>,
    pub tab_id: Option<String>,
    pub process_start_marker: Option<String>,
    pub classification: &'static str,
    pub harness_slug: Option<String>,
}

/// Provider-level census result: status, provenance, and how the parse went.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProviderCensus {
    pub provider: &'static str,
    pub status: &'static str,
    pub version: Option<String>,
    /// Exact stderr or an observed-shape note for degraded states.
    pub detail: Option<String>,
    pub malformed_rows: usize,
    pub pane_count: usize,
}

/// A place key observed bound to two different process generations in one
/// census. Named for the owner; never merged.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlaceReuseFinding {
    pub provider: &'static str,
    pub place_key: String,
    pub first_pane_pid: u32,
    pub second_pane_pid: u32,
    pub reason: &'static str,
}

/// One completed place census.
#[derive(Debug, Clone)]
pub struct PlaceCensus {
    pub machine: String,
    pub generated_utc: String,
    pub providers: Vec<ProviderCensus>,
    pub panes: Vec<PaneObservation>,
    pub reuse_findings: Vec<PlaceReuseFinding>,
}

/// Run one full place census against the live host. Read-only by law.
pub fn scan_places_live() -> PlaceCensus {
    let machine = machine_name();
    let generated_utc = utc_now_rfc3339();

    let pid_table = read_pid_table().unwrap_or_default();

    let (tmux_provider, tmux_panes) = scan_tmux_provider();
    let (herdr_provider, herdr_panes) = scan_herdr_provider();

    let mut panes = tmux_panes;
    panes.extend(herdr_panes);
    join_process_evidence(&mut panes, &pid_table);

    let providers = vec![tmux_provider, herdr_provider];
    assemble_census(machine, generated_utc, providers, panes)
}

/// Pure assembly: join, classify, find place-key reuse. Separated from
/// `scan_places_live` so the law is testable without a host.
pub fn assemble_census(
    machine: String,
    generated_utc: String,
    providers: Vec<ProviderCensus>,
    mut panes: Vec<PaneObservation>,
) -> PlaceCensus {
    panes.sort_by(|a, b| (&a.provider, &a.place_key).cmp(&(&b.provider, &b.place_key)));

    // Place-key reuse: the same key bound to two different pids in one
    // census is a recycled place, named with both generations — the same
    // discipline instance_scan applies to recycled pids.
    let mut by_key: BTreeMap<(&'static str, &str), Vec<&PaneObservation>> = BTreeMap::new();
    for pane in &panes {
        by_key
            .entry((pane.provider, pane.place_key.as_str()))
            .or_default()
            .push(pane);
    }
    let mut reuse_findings = Vec::new();
    for ((provider, key), group) in by_key {
        let mut pids: Vec<u32> = group.iter().filter_map(|pane| pane.pane_pid).collect();
        pids.sort_unstable();
        pids.dedup();
        for pair in pids.windows(2) {
            reuse_findings.push(PlaceReuseFinding {
                provider,
                place_key: key.to_owned(),
                first_pane_pid: pair[0],
                second_pane_pid: pair[1],
                reason: "the same place key is bound to two process generations; \
                         a recycled pane id or session name is a new place",
            });
        }
    }

    PlaceCensus {
        machine,
        generated_utc,
        providers,
        panes,
        reuse_findings,
    }
}

/// Join `ps` start-marker evidence into the pane observations and classify
/// each pane's process.
pub fn join_process_evidence(panes: &mut [PaneObservation], pid_table: &[ObservedProcess]) {
    let by_pid: BTreeMap<u32, &ObservedProcess> = pid_table
        .iter()
        .map(|process| (process.pid, process))
        .collect();
    for pane in panes.iter_mut() {
        let Some(pid) = pane.pane_pid else {
            pane.classification = PLACE_CLASS_UNOBSERVED;
            continue;
        };
        let Some(process) = by_pid.get(&pid) else {
            // The pane's pid is not in the pid table (a dead pane between
            // enumerations, or a provider that reports stale pids). The pane
            // stays visible, unobserved — never dropped.
            pane.classification = PLACE_CLASS_UNOBSERVED;
            continue;
        };
        pane.process_start_marker = (!process.start_marker.is_empty())
            .then(|| process.start_marker.clone())
            .or(pane.process_start_marker.clone());
        let (classification, harness_slug) = classify_command(&process.comm);
        pane.classification = classification;
        pane.harness_slug = harness_slug;
    }
}

/// Classify one `ps comm` value. The stem is the executable basename: tmux
/// and Herdr both report bare command names, and full paths classify the
/// same way.
pub fn classify_command(comm: &str) -> (&'static str, Option<String>) {
    let stem = comm.rsplit('/').next().unwrap_or(comm);
    if SELF_BINARY_STEMS.contains(&stem) {
        return (PLACE_CLASS_SELF, None);
    }
    if let Some((_, slug)) = PID_ALIASES.iter().find(|(alias, _)| alias == &stem) {
        return (PLACE_CLASS_HARNESS, Some((*slug).to_owned()));
    }
    (PLACE_CLASS_OTHER, None)
}

// ---------------------------------------------------------------------------
// tmux provider
// ---------------------------------------------------------------------------

/// Enumerate the tmux default socket. Read-only: `tmux -V` then
/// `tmux list-panes -a`. A missing server is `present-no-server` with the
/// exact stderr — tmux prints it and exits nonzero without creating one.
pub fn scan_tmux_provider() -> (ProviderCensus, Vec<PaneObservation>) {
    let version = match tmux_version() {
        Ok(version) => version,
        Err(detail) => {
            return (
                ProviderCensus {
                    provider: TMUX_PROVIDER,
                    status: PLACE_PROVIDER_ERROR,
                    version: None,
                    detail: Some(detail),
                    malformed_rows: 0,
                    pane_count: 0,
                },
                Vec::new(),
            )
        }
    };
    let Some(version) = version else {
        return (
            ProviderCensus {
                provider: TMUX_PROVIDER,
                status: PLACE_PROVIDER_ABSENT,
                version: None,
                detail: None,
                malformed_rows: 0,
                pane_count: 0,
            },
            Vec::new(),
        );
    };

    let version = Some(version);

    match run_tmux_list_panes() {
        Ok(stdout) => {
            let (rows, malformed) = parse_tmux_list_panes(&stdout);
            let pane_count = rows.len();
            (
                ProviderCensus {
                    provider: TMUX_PROVIDER,
                    status: PLACE_PROVIDER_OK,
                    version: version.clone(),
                    detail: None,
                    malformed_rows: malformed,
                    pane_count,
                },
                tmux_rows_to_observations(rows),
            )
        }
        Err(TmuxListError::NoServer(stderr)) => (
            ProviderCensus {
                provider: TMUX_PROVIDER,
                status: PLACE_PROVIDER_NO_SERVER,
                version: version.clone(),
                detail: Some(stderr),
                malformed_rows: 0,
                pane_count: 0,
            },
            Vec::new(),
        ),
        Err(TmuxListError::Failed(stderr)) => (
            ProviderCensus {
                provider: TMUX_PROVIDER,
                status: PLACE_PROVIDER_ERROR,
                version,
                detail: Some(stderr),
                malformed_rows: 0,
                pane_count: 0,
            },
            Vec::new(),
        ),
    }
}

/// `tmux -V` (e.g. `tmux 3.6a`). `Ok(None)` = tmux not installed; `Err` =
/// installed but the probe failed (named, with the exact stderr).
fn tmux_version() -> std::result::Result<Option<String>, String> {
    let output = match Command::new("tmux").arg("-V").stdin(Stdio::null()).output() {
        Ok(output) => output,
        // A missing binary is absence, not failure: the census says so.
        Err(_) => return Ok(None),
    };
    // `tmux -V` never needs a server; any success carries the version.
    if !output.status.success() {
        return Err(format!(
            "`tmux -V` exited {} with stderr: {}",
            output.status,
            String::from_utf8_lossy(&output.stderr).trim()
        ));
    }
    Ok(Some(
        String::from_utf8_lossy(&output.stdout).trim().to_owned(),
    ))
}

pub(crate) enum TmuxListError {
    /// tmux named the absence of a server. The exact stderr is evidence.
    NoServer(String),
    /// Any other failure.
    Failed(String),
}

/// `tmux list-panes -a -F <format>` against the default socket. Read-only:
/// listing never starts a server; with none running tmux exits nonzero
/// naming it.
pub(crate) fn run_tmux_list_panes() -> std::result::Result<String, TmuxListError> {
    let output = Command::new("tmux")
        .args(["list-panes", "-a", "-F", TMUX_PANE_FORMAT])
        .stdin(Stdio::null())
        .output()
        .map_err(|error| TmuxListError::Failed(format!("run `tmux list-panes -a`: {error}")))?;
    let stderr = String::from_utf8_lossy(&output.stderr).trim().to_owned();
    if !output.status.success() {
        if stderr.contains("no server running") {
            return Err(TmuxListError::NoServer(stderr));
        }
        return Err(TmuxListError::Failed(stderr));
    }
    Ok(String::from_utf8_lossy(&output.stdout).into_owned())
}

/// Parse `tmux list-panes -a -F TMUX_PANE_FORMAT` output. One row per line,
/// nine tab-separated fields. A malformed row is skipped and counted, never
/// fatal — a partially read census with a named gap beats no census.
pub fn parse_tmux_list_panes(output: &str) -> (Vec<TmuxPaneRow>, usize) {
    let mut rows = Vec::new();
    let mut malformed = 0usize;
    for line in output.lines() {
        if line.trim().is_empty() {
            continue;
        }
        let fields: Vec<&str> = line.split(':').collect();
        if fields.len() != 9 {
            malformed += 1;
            continue;
        }
        // An empty pid field is a live-but-pidless row (a dead pane); a
        // non-numeric, non-empty pid field is malformed evidence for the row.
        let pane_pid = match fields[5].trim() {
            "" => None,
            trimmed => match trimmed.parse::<u32>() {
                Ok(pid) => Some(pid),
                Err(_) => {
                    malformed += 1;
                    continue;
                }
            },
        };
        rows.push(TmuxPaneRow {
            session_name: fields[0].to_owned(),
            session_created: fields[1].to_owned(),
            window_index: fields[2].to_owned(),
            window_name: fields[3].to_owned(),
            pane_id: fields[4].to_owned(),
            pane_pid,
            pane_current_command: fields[6].to_owned(),
            pane_dead: fields[7].trim() == "1",
            pane_tty: fields[8].to_owned(),
        });
    }
    (rows, malformed)
}

pub(crate) fn tmux_rows_to_observations(rows: Vec<TmuxPaneRow>) -> Vec<PaneObservation> {
    rows.into_iter()
        .map(|row| {
            let place_key = format!("{}/{}", row.session_name, row.pane_id);
            PaneObservation {
                provider: TMUX_PROVIDER,
                place_key,
                session_name: Some(row.session_name),
                session_created: (!row.session_created.is_empty()).then_some(row.session_created),
                window_index: (!row.window_index.is_empty()).then_some(row.window_index),
                window_name: (!row.window_name.is_empty()).then_some(row.window_name),
                pane_id: row.pane_id,
                pane_pid: row.pane_pid,
                pane_command: (!row.pane_current_command.is_empty())
                    .then_some(row.pane_current_command),
                pane_dead: row.pane_dead,
                pane_tty: (!row.pane_tty.is_empty()).then_some(row.pane_tty),
                workspace_id: None,
                workspace_label: None,
                tab_id: None,
                process_start_marker: None,
                classification: PLACE_CLASS_UNOBSERVED,
                harness_slug: None,
            }
        })
        .collect()
}

// ---------------------------------------------------------------------------
// herdr provider
// ---------------------------------------------------------------------------

/// Enumerate the Herdr server's workspaces and panes. Read-only: `herdr
/// --version`, `herdr workspace list`, `herdr pane list` and one `herdr pane
/// process-info` per pane, all over its socket API. A CLI that is installed
/// but whose server does not answer is `present-server-unreachable` with the
/// exact stderr.
pub fn scan_herdr_provider() -> (ProviderCensus, Vec<PaneObservation>) {
    let version = match run_herdr_version() {
        Ok(version) => version,
        Err(detail) => {
            return (
                ProviderCensus {
                    provider: HERDR_PROVIDER,
                    status: PLACE_PROVIDER_ABSENT,
                    version: None,
                    detail: Some(detail),
                    malformed_rows: 0,
                    pane_count: 0,
                },
                Vec::new(),
            )
        }
    };

    let workspaces_raw = match run_herdr_json_command(&["workspace", "list"]) {
        Ok(stdout) => stdout,
        Err(detail) => {
            return (
                ProviderCensus {
                    provider: HERDR_PROVIDER,
                    status: PLACE_PROVIDER_SERVER_UNREACHABLE,
                    version: Some(version),
                    detail: Some(detail),
                    malformed_rows: 0,
                    pane_count: 0,
                },
                Vec::new(),
            )
        }
    };
    let workspaces = match parse_herdr_workspace_list(&workspaces_raw) {
        Ok(workspaces) => workspaces,
        Err(detail) => {
            return (
                ProviderCensus {
                    provider: HERDR_PROVIDER,
                    status: PLACE_PROVIDER_CLI_UNPARSED,
                    version: Some(version),
                    detail: Some(detail),
                    malformed_rows: 0,
                    pane_count: 0,
                },
                Vec::new(),
            )
        }
    };

    let panes_raw = match run_herdr_json_command(&["pane", "list"]) {
        Ok(stdout) => stdout,
        Err(detail) => {
            return (
                ProviderCensus {
                    provider: HERDR_PROVIDER,
                    status: PLACE_PROVIDER_SERVER_UNREACHABLE,
                    version: Some(version),
                    detail: Some(detail),
                    malformed_rows: 0,
                    pane_count: 0,
                },
                Vec::new(),
            )
        }
    };
    let herdr_panes = match parse_herdr_pane_list(&panes_raw) {
        Ok(panes) => panes,
        Err(detail) => {
            return (
                ProviderCensus {
                    provider: HERDR_PROVIDER,
                    status: PLACE_PROVIDER_CLI_UNPARSED,
                    version: Some(version),
                    detail: Some(detail),
                    malformed_rows: 0,
                    pane_count: 0,
                },
                Vec::new(),
            )
        }
    };

    let labels: BTreeMap<String, String> = workspaces
        .iter()
        .map(|workspace| (workspace.workspace_id.clone(), workspace.label.clone()))
        .collect();

    let mut malformed = 0usize;
    let mut observations = Vec::new();
    for pane in herdr_panes {
        let workspace_id = pane.workspace_id.clone();
        let process_info =
            match run_herdr_json_command(&["pane", "process-info", "--pane", &pane.pane_id]) {
                Ok(stdout) => parse_herdr_process_info(&stdout).unwrap_or_else(|_| {
                    malformed += 1;
                    HerdrProcessInfo::default()
                }),
                Err(_) => {
                    // One pane refusing its process reading is a named gap for
                    // that pane, not a failed provider.
                    malformed += 1;
                    HerdrProcessInfo::default()
                }
            };
        let place_key = format!(
            "{}/{}",
            workspace_id.clone().unwrap_or_default(),
            pane.pane_id
        );
        observations.push(PaneObservation {
            provider: HERDR_PROVIDER,
            place_key,
            session_name: None,
            session_created: None,
            window_index: None,
            window_name: None,
            pane_id: pane.pane_id.clone(),
            pane_pid: process_info.shell_pid,
            pane_command: process_info.foreground_name,
            pane_dead: false,
            pane_tty: None,
            workspace_id: workspace_id.clone(),
            workspace_label: workspace_id.as_ref().and_then(|id| labels.get(id).cloned()),
            tab_id: pane.tab_id,
            process_start_marker: None,
            classification: PLACE_CLASS_UNOBSERVED,
            harness_slug: None,
        });
    }

    (
        ProviderCensus {
            provider: HERDR_PROVIDER,
            status: PLACE_PROVIDER_OK,
            version: Some(version),
            detail: None,
            malformed_rows: malformed,
            pane_count: observations.len(),
        },
        observations,
    )
}

/// `herdr --version`. A failure to spawn or a nonzero exit means the CLI is
/// not usable on this machine: `absent`.
fn run_herdr_version() -> std::result::Result<String, String> {
    let output = Command::new("herdr")
        .arg("--version")
        .stdin(Stdio::null())
        .output()
        .map_err(|error| format!("run `herdr --version`: {error}"))?;
    if !output.status.success() {
        return Err(format!(
            "`herdr --version` exited {} with stderr: {}",
            output.status,
            String::from_utf8_lossy(&output.stderr).trim()
        ));
    }
    Ok(String::from_utf8_lossy(&output.stdout).trim().to_owned())
}

fn run_herdr_json_command(args: &[&str]) -> std::result::Result<String, String> {
    let output = Command::new("herdr")
        .args(args)
        .stdin(Stdio::null())
        .output()
        .map_err(|error| format!("run `herdr {}`: {error}", args.join(" ")))?;
    let stderr = String::from_utf8_lossy(&output.stderr).trim().to_owned();
    if !output.status.success() {
        return Err(format!(
            "`herdr {}` exited {} with stderr: {}",
            args.join(" "),
            output.status,
            stderr
        ));
    }
    Ok(String::from_utf8_lossy(&output.stdout).into_owned())
}

/// Parse `herdr workspace list`: a JSON envelope with
/// `result.workspaces[]`. Anything else is a named unparsed finding.
pub fn parse_herdr_workspace_list(
    output: &str,
) -> std::result::Result<Vec<HerdrWorkspaceRow>, String> {
    let value: Value = serde_json::from_str(output.trim())
        .map_err(|error| format!("parse `herdr workspace list` output as JSON: {error}"))?;
    let workspaces = value
        .pointer("/result/workspaces")
        .and_then(Value::as_array)
        .ok_or_else(|| {
            "`herdr workspace list` output lacks result.workspaces; observed shape not machine-readable"
                .to_owned()
        })?;
    let mut rows = Vec::new();
    for workspace in workspaces {
        let Some(workspace_id) = workspace.get("workspace_id").and_then(Value::as_str) else {
            continue; // a workspace without an id cannot be addressed; skip it
        };
        rows.push(HerdrWorkspaceRow {
            workspace_id: workspace_id.to_owned(),
            label: workspace
                .get("label")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_owned(),
            active_tab_id: workspace
                .get("active_tab_id")
                .and_then(Value::as_str)
                .map(str::to_owned),
            tab_count: workspace.get("tab_count").and_then(Value::as_u64),
            pane_count: workspace.get("pane_count").and_then(Value::as_u64),
        });
    }
    Ok(rows)
}

/// Parse `herdr pane list`: a JSON envelope with `result.panes[]`.
pub fn parse_herdr_pane_list(output: &str) -> std::result::Result<Vec<HerdrPaneRow>, String> {
    let value: Value = serde_json::from_str(output.trim())
        .map_err(|error| format!("parse `herdr pane list` output as JSON: {error}"))?;
    let panes = value
        .pointer("/result/panes")
        .and_then(Value::as_array)
        .ok_or_else(|| {
            "`herdr pane list` output lacks result.panes; observed shape not machine-readable"
                .to_owned()
        })?;
    let mut rows = Vec::new();
    for pane in panes {
        let Some(pane_id) = pane.get("pane_id").and_then(Value::as_str) else {
            continue;
        };
        rows.push(HerdrPaneRow {
            pane_id: pane_id.to_owned(),
            workspace_id: pane
                .get("workspace_id")
                .and_then(Value::as_str)
                .map(str::to_owned),
            tab_id: pane
                .get("tab_id")
                .and_then(Value::as_str)
                .map(str::to_owned),
            cwd: pane.get("cwd").and_then(Value::as_str).map(str::to_owned),
        });
    }
    Ok(rows)
}

/// Parse `herdr pane process-info`: shell pid plus the foreground process
/// name — the closest Herdr analog of tmux's `pane_current_command`.
pub fn parse_herdr_process_info(output: &str) -> std::result::Result<HerdrProcessInfo, String> {
    let value: Value = serde_json::from_str(output.trim())
        .map_err(|error| format!("parse `herdr pane process-info` output as JSON: {error}"))?;
    let info = value
        .pointer("/result/process_info")
        .ok_or_else(|| "`herdr pane process-info` output lacks result.process_info".to_owned())?;
    let foreground_name = info
        .get("foreground_processes")
        .and_then(Value::as_array)
        .and_then(|processes| processes.first())
        .and_then(|process| process.get("name"))
        .and_then(Value::as_str)
        .map(str::to_owned);
    Ok(HerdrProcessInfo {
        shell_pid: info
            .get("shell_pid")
            .and_then(Value::as_u64)
            .and_then(|pid| u32::try_from(pid).ok()),
        foreground_name,
    })
}

// ---------------------------------------------------------------------------
// provenance
// ---------------------------------------------------------------------------

/// Host provenance for the census document. An unreadable hostname is named
/// `unknown` rather than guessed.
pub fn machine_name() -> String {
    #[cfg(unix)]
    {
        let mut buffer = [0u8; 256];
        // SAFETY: `buffer` is a valid 256-byte array for the name; gethostname
        // truncates and always NUL-terminates within the given length.
        let result = unsafe { libc::gethostname(buffer.as_mut_ptr().cast(), buffer.len()) };
        if result == 0 {
            let end = buffer
                .iter()
                .position(|byte| *byte == 0)
                .unwrap_or(buffer.len());
            let name = String::from_utf8_lossy(&buffer[..end]).trim().to_owned();
            if !name.is_empty() {
                return name;
            }
        }
    }
    "unknown".to_owned()
}

/// UTC wall-clock as RFC 3339 with second precision, derived from the Unix
/// epoch with the standard civil-from-days conversion (no wall-clock reads,
/// no dependencies).
pub fn utc_now_rfc3339() -> String {
    let seconds = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .unwrap_or(0);
    format_rfc3339_utc(seconds)
}

/// Render Unix seconds as `YYYY-MM-DDTHH:MM:SSZ`.
pub fn format_rfc3339_utc(seconds: u64) -> String {
    let days = seconds / 86_400;
    let seconds_of_day = seconds % 86_400;
    let (year, month, day) = civil_from_days(days as i64);
    format!(
        "{year:04}-{month:02}-{day:02}T{:02}:{:02}:{:02}Z",
        seconds_of_day / 3600,
        (seconds_of_day % 3600) / 60,
        seconds_of_day % 60
    )
}

/// Howard Hinnant's civil-from-days: days since 1970-01-01 to (y, m, d).
fn civil_from_days(days: i64) -> (i64, u32, u32) {
    let z = days + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    (if m <= 2 { y + 1 } else { y }, m, d)
}

/// Render the census as a JSON value (the `workcell places --json` shape).
pub fn census_json(census: &PlaceCensus) -> Value {
    json!({
        "schema": PLACE_CENSUS_VERSION,
        "ok": true,
        "machine": census.machine,
        "generated_utc": census.generated_utc,
        "providers": census.providers.iter().map(|provider| json!({
            "provider": provider.provider,
            "status": provider.status,
            "version": provider.version,
            "detail": provider.detail,
            "malformed_rows": provider.malformed_rows,
            "pane_count": provider.pane_count,
        })).collect::<Vec<_>>(),
        "panes": census.panes.iter().map(pane_observation_json).collect::<Vec<_>>(),
        "place_reuse_findings": census.reuse_findings.iter().map(|finding| json!({
            "provider": finding.provider,
            "place_key": finding.place_key,
            "first_pane_pid": finding.first_pane_pid,
            "second_pane_pid": finding.second_pane_pid,
            "reason": finding.reason,
        })).collect::<Vec<_>>(),
        "summary": {
            "panes": census.panes.len(),
            "harness": census.panes.iter().filter(|p| p.classification == PLACE_CLASS_HARNESS).count(),
            "self": census.panes.iter().filter(|p| p.classification == PLACE_CLASS_SELF).count(),
            "other": census.panes.iter().filter(|p| p.classification == PLACE_CLASS_OTHER).count(),
            "unobserved": census.panes.iter().filter(|p| p.classification == PLACE_CLASS_UNOBSERVED).count(),
            "dead": census.panes.iter().filter(|p| p.pane_dead).count(),
        },
    })
}

pub fn pane_observation_json(pane: &PaneObservation) -> Value {
    json!({
        "provider": pane.provider,
        "place_key": pane.place_key,
        "session_name": pane.session_name,
        "session_created": pane.session_created,
        "window_index": pane.window_index,
        "window_name": pane.window_name,
        "pane_id": pane.pane_id,
        "pane_pid": pane.pane_pid,
        "pane_command": pane.pane_command,
        "pane_dead": pane.pane_dead,
        "pane_tty": pane.pane_tty,
        "workspace_id": pane.workspace_id,
        "workspace_label": pane.workspace_label,
        "tab_id": pane.tab_id,
        "process_start_marker": pane.process_start_marker,
        "classification": pane.classification,
        "harness_slug": pane.harness_slug,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const MARKER_A: &str = "Thu Sep 14 00:01:50 2026";
    const MARKER_B: &str = "Thu Sep 14 00:09:09 2026";

    fn tmux_fixture() -> String {
        // Two live panes (one a harness alias), one dead pane with no pid,
        // one row whose window name contains spaces, and one malformed row.
        // Separator `:` — see TMUX_PANE_FORMAT for why not a tab.
        "main:1718000000:0:zsh:%0:4242:zsh:0:/dev/ttys004\n\
         main:1718000000:1:vim notes:%1:4243:claude:0:/dev/ttys005\n\
         workcell-test:1718000100:0:zsh:%2::zsh:1:\n\
         shared work:1718000200:0:logs & errors:%3:4244:htop:0:/dev/ttys006\n\
         this:row:is:broken\n"
            .to_string()
    }

    #[test]
    fn census_version_constant_is_the_published_contract() {
        // Contract-revision pinning: the census document version is a named
        // constant asserted in tests, like every other schema in this crate.
        assert_eq!(PLACE_CENSUS_VERSION, "workcell.place-census/v1");
    }

    #[test]
    fn tmux_parser_reads_panes_and_counts_malformed_rows() {
        let (rows, malformed) = parse_tmux_list_panes(&tmux_fixture());
        assert_eq!(
            malformed, 1,
            "the broken row is skipped and counted, never fatal"
        );
        assert_eq!(rows.len(), 4);

        let first = &rows[0];
        assert_eq!(first.session_name, "main");
        assert_eq!(first.pane_id, "%0");
        assert_eq!(first.pane_pid, Some(4242));
        assert_eq!(first.pane_current_command, "zsh");
        assert!(!first.pane_dead);

        let harness_pane = &rows[1];
        assert_eq!(harness_pane.pane_current_command, "claude");
        assert_eq!(
            harness_pane.window_name, "vim notes",
            "window names keep spaces"
        );

        let dead = &rows[2];
        assert!(dead.pane_dead);
        assert_eq!(
            dead.pane_pid, None,
            "a dead pane carries no live pid but stays visible"
        );
        assert_eq!(dead.session_name, "workcell-test");

        let spaced = &rows[3];
        assert_eq!(spaced.window_name, "logs & errors");
        assert_eq!(spaced.pane_tty, "/dev/ttys006");
    }

    #[test]
    fn tmux_parser_refuses_non_numeric_pid_rows_as_malformed() {
        let line = "main:1718000000:0:zsh:%0:not-a-pid:zsh:0:/dev/ttys004\n";
        let (rows, malformed) = parse_tmux_list_panes(line);
        assert_eq!(rows.len(), 0);
        assert_eq!(malformed, 1);
    }

    #[test]
    fn tmux37_control_char_escapes_never_count_as_separators() {
        // tmux 3.7 (Omarchy host, found live during TM02-R): a literal tab
        // byte in the format comes back escaped as the two characters `\t`,
        // so a tab-separated contract silently degrades to one field per
        // row. With the `:` contract such a degraded row is malformed and
        // counted, never silently accepted or fatal.
        let (rows, malformed) = parse_tmux_list_panes(
            "main\\t1718000000\\t0\\tzsh\\t%0\\t4242\\tzsh\\t0\\t/dev/ttys004\n",
        );
        assert_eq!(rows.len(), 0);
        assert_eq!(malformed, 1);
    }

    #[test]
    fn classification_names_harness_self_and_other() {
        let (harness, slug) = classify_command("claude");
        assert_eq!(harness, PLACE_CLASS_HARNESS);
        assert_eq!(slug.as_deref(), Some("claude-code"));

        let (self_class, slug) = classify_command("workcell");
        assert_eq!(self_class, PLACE_CLASS_SELF);
        assert_eq!(slug, None);

        let (self_path, _) = classify_command("/usr/local/bin/factory");
        assert_eq!(
            self_path, PLACE_CLASS_SELF,
            "full paths classify by basename"
        );

        let (other, slug) = classify_command("htop");
        assert_eq!(
            other, PLACE_CLASS_OTHER,
            "outside processes are seen, never hidden"
        );
        assert_eq!(slug, None);
    }

    fn tmux_observation(place_key: &str, pane_id: &str, pid: Option<u32>) -> PaneObservation {
        PaneObservation {
            provider: TMUX_PROVIDER,
            place_key: place_key.to_owned(),
            session_name: Some("main".into()),
            session_created: Some("1718000000".into()),
            window_index: Some("0".into()),
            window_name: Some("zsh".into()),
            pane_id: pane_id.to_owned(),
            pane_pid: pid,
            pane_command: Some("zsh".into()),
            pane_dead: false,
            pane_tty: Some("/dev/ttys004".into()),
            workspace_id: None,
            workspace_label: None,
            tab_id: None,
            process_start_marker: None,
            classification: PLACE_CLASS_UNOBSERVED,
            harness_slug: None,
        }
    }

    fn pid(pid_value: u32, comm: &str, marker: &str) -> ObservedProcess {
        ObservedProcess {
            pid: pid_value,
            start_marker: marker.to_owned(),
            comm: comm.to_owned(),
        }
    }

    #[test]
    fn process_evidence_is_joined_and_classified() {
        let mut panes = vec![
            tmux_observation("main/%0", "%0", Some(10)),
            tmux_observation("main/%1", "%1", Some(11)),
            tmux_observation("main/%2", "%2", None),
            tmux_observation("main/%3", "%3", Some(99)),
        ];
        let table = vec![
            pid(10, "zsh", MARKER_A),
            pid(11, "/usr/local/bin/claude", MARKER_A),
        ];
        join_process_evidence(&mut panes, &table);

        assert_eq!(panes[0].classification, PLACE_CLASS_OTHER);
        assert_eq!(panes[0].process_start_marker.as_deref(), Some(MARKER_A));

        assert_eq!(panes[1].classification, PLACE_CLASS_HARNESS);
        assert_eq!(panes[1].harness_slug.as_deref(), Some("claude-code"));

        assert_eq!(
            panes[2].classification, PLACE_CLASS_UNOBSERVED,
            "no pid, no claim"
        );

        assert_eq!(
            panes[3].classification, PLACE_CLASS_UNOBSERVED,
            "a pid the table does not name stays visible and unobserved"
        );
    }

    #[test]
    fn same_place_key_with_two_generations_is_a_named_reuse_finding() {
        // Place law: a recycled pane id or session name is a new place. The
        // census names both generations instead of merging them.
        let panes = vec![
            PaneObservation {
                process_start_marker: Some(MARKER_A.into()),
                ..tmux_observation("main/%0", "%0", Some(7))
            },
            PaneObservation {
                process_start_marker: Some(MARKER_B.into()),
                ..tmux_observation("main/%0", "%0", Some(8))
            },
        ];
        let census = assemble_census("host".into(), "2026-09-15T00:00:00Z".into(), vec![], panes);
        assert_eq!(census.reuse_findings.len(), 1);
        let finding = &census.reuse_findings[0];
        assert_eq!(finding.place_key, "main/%0");
        assert_eq!(finding.first_pane_pid, 7);
        assert_eq!(finding.second_pane_pid, 8);
        assert!(finding.reason.contains("new place"));
    }

    #[test]
    fn distinct_place_keys_with_distinct_pids_are_not_reuse() {
        let panes = vec![
            tmux_observation("main/%0", "%0", Some(7)),
            tmux_observation("main/%1", "%1", Some(8)),
        ];
        let census = assemble_census("host".into(), "2026-09-15T00:00:00Z".into(), vec![], panes);
        assert!(census.reuse_findings.is_empty());
    }

    #[test]
    fn census_json_carries_the_contract_shape() {
        let providers = vec![ProviderCensus {
            provider: TMUX_PROVIDER,
            status: PLACE_PROVIDER_NO_SERVER,
            version: Some("tmux 3.6a".into()),
            detail: Some("no server running on /tmp/tmux-501/default".into()),
            malformed_rows: 0,
            pane_count: 0,
        }];
        let census = assemble_census(
            "workcell-host".into(),
            "2026-09-15T12:00:00Z".into(),
            providers,
            vec![tmux_observation("main/%0", "%0", Some(4242))],
        );
        let value = census_json(&census);
        assert_eq!(value["schema"], PLACE_CENSUS_VERSION);
        assert_eq!(value["machine"], "workcell-host");
        assert_eq!(value["generated_utc"], "2026-09-15T12:00:00Z");
        assert_eq!(value["providers"][0]["status"], PLACE_PROVIDER_NO_SERVER);
        assert_eq!(
            value["providers"][0]["detail"], "no server running on /tmp/tmux-501/default",
            "degraded states carry the exact evidence"
        );
        assert_eq!(value["panes"][0]["pane_pid"], 4242);
        assert_eq!(value["summary"]["panes"], 1);
    }

    #[test]
    fn rfc3339_utc_renders_known_instants() {
        assert_eq!(format_rfc3339_utc(0), "1970-01-01T00:00:00Z");
        // 2026-09-15T00:00:00Z = 1789430400
        assert_eq!(format_rfc3339_utc(1_789_430_400), "2026-09-15T00:00:00Z");
        // Leap-year day: 2024-02-29T12:34:56Z = 1709210096
        assert_eq!(format_rfc3339_utc(1_709_210_096), "2024-02-29T12:34:56Z");
    }

    #[test]
    fn machine_name_is_non_empty() {
        // The host provenance must be a real name or the honest `unknown`;
        // this machine has a name.
        let name = machine_name();
        assert!(!name.is_empty());
        assert_ne!(name, "unknown", "this test host has a resolvable hostname");
    }
}
