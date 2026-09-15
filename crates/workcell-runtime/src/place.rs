//! Place request and release: claim a persistent process place and give it
//! back — with the same process-generation proof the instance registry uses
//! for pids.
//!
//! The census (`place_scan`) is read-only. This module is the only place
//! where Workcell asks a provider to change the machine, and every effect is
//! bounded:
//!
//! - a place name is validated `[a-z0-9-]{1,64}` before anything runs;
//! - the tmux request is exactly one `tmux new-session -d -s <name>`,
//!   followed by read-only observation of the created pane; there is no shell
//!   interpolation anywhere — every provider command is built as argv;
//! - the herdr request uses only what its proven CLI surface supports
//!   (`workspace create --label`, `workspace close`); anything the CLI does
//!   not prove is a typed `provider-cannot-create` refusal, never a guess;
//! - an existing place of the same name is REFUSED (`already-exists`), never
//!   silently adopted — adoption is an explicit later operation, mirroring
//!   the declare/adopt separation in `instance_registry`;
//! - release kills only after the live pid table proves the process still
//!   matches the granted `(pid, process_start_marker)` generation. A recycled
//!   pid is a stale binding, named and refused.
//!
//! Relation to the write-boundary discipline (`workcell-write-boundary
//! inspect/run`): that path is a Landlock confinement for launched workload
//! processes in the control plane (`HostProcessExecutionProvider`); it does
//! not apply to a one-shot CLI invoking tmux or herdr directly. The bound
//! here is the argv contract above: one named effect, no interpolation, no
//! second command beyond the read-back.

use std::process::{Command, Stdio};

use epilogos_workcell_core::WorkcellError;
use serde_json::{json, Value};

use crate::instance_scan::{read_pid_table, ObservedProcess};
use crate::place_scan::{
    parse_herdr_pane_list, parse_herdr_process_info, parse_herdr_workspace_list,
    parse_tmux_list_panes, tmux_rows_to_observations, utc_now_rfc3339, HerdrPaneRow, TmuxListError,
    HERDR_PROVIDER, TMUX_PROVIDER,
};

pub const PLACE_GRANT_VERSION: &str = "workcell.place-grant/v1";
pub const PLACE_REF_PREFIX: &str = "workcell:place:";

/// The tmux socket component of every place_ref this module mints. Only the
/// default socket is ever used, so the component is a constant, not an
/// observation.
pub const TMUX_DEFAULT_SOCKET: &str = "default";

/// Refusal kinds. Every refusal carries evidence; error paths explain.
pub const REFUSAL_ALREADY_EXISTS: &str = "already-exists";
pub const REFUSAL_STALE_BINDING: &str = "stale-binding";
pub const REFUSAL_PROVIDER_CANNOT_CREATE: &str = "provider-cannot-create";
pub const REFUSAL_NO_PROVIDER: &str = "no-provider";
pub const REFUSAL_PROVIDER_ERROR: &str = "provider-error";
pub const REFUSAL_INVALID_NAME: &str = "invalid-name";
pub const REFUSAL_INVALID_PLACE_REF: &str = "invalid-place-ref";
pub const REFUSAL_PLACE_GONE: &str = "place-gone";
pub const REFUSAL_PLACE_MISMATCH: &str = "place-mismatch";

/// A granted place: material evidence and an addressable location — never a
/// caller identity. The `(pane_pid, process_start_marker)` pair scopes the
/// grant to one process generation; a later release must prove that same
/// generation is still live before killing anything.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlaceGrant {
    pub place_ref: String,
    pub provider: &'static str,
    pub session_name: String,
    /// herdr only: the provider-native workspace id backing the place.
    pub workspace_id: Option<String>,
    pub pane_id: String,
    pub pane_pid: u32,
    pub process_start_marker: String,
    pub created_utc: String,
    pub grant_version: &'static str,
}

impl PlaceGrant {
    pub fn to_json(&self) -> Value {
        json!({
            "grant_version": self.grant_version,
            "place_ref": self.place_ref,
            "provider": self.provider,
            "session_name": self.session_name,
            "workspace_id": self.workspace_id,
            "pane_id": self.pane_id,
            "pane_pid": self.pane_pid,
            "process_start_marker": self.process_start_marker,
            "created_utc": self.created_utc,
        })
    }

    pub fn from_json(value: &Value) -> Result<Self, WorkcellError> {
        let grant_version = value
            .get("grant_version")
            .and_then(Value::as_str)
            .ok_or_else(|| {
                WorkcellError::InvalidDemand("place grant requires a string `grant_version`".into())
            })?;
        if grant_version != PLACE_GRANT_VERSION {
            return Err(WorkcellError::InvalidDemand(format!(
                "place grant version `{grant_version}` must be `{PLACE_GRANT_VERSION}`"
            )));
        }
        let place_ref = required_str(value, "place_ref")?;
        let provider = match value.get("provider").and_then(Value::as_str) {
            Some(HERDR_PROVIDER) => HERDR_PROVIDER,
            Some(TMUX_PROVIDER) => TMUX_PROVIDER,
            other => {
                return Err(WorkcellError::InvalidDemand(format!(
                    "place grant provider {other:?} must be `{TMUX_PROVIDER}` or `{HERDR_PROVIDER}`"
                )))
            }
        };
        let workspace_id = match value.get("workspace_id") {
            Some(Value::Null) | None => None,
            Some(Value::String(id)) => Some(id.clone()),
            Some(_) => {
                return Err(WorkcellError::InvalidDemand(
                    "place grant `workspace_id` must be a string or null".into(),
                ))
            }
        };
        Ok(Self {
            place_ref,
            provider,
            session_name: required_str(value, "session_name")?,
            workspace_id,
            pane_id: required_str(value, "pane_id")?,
            pane_pid: value
                .get("pane_pid")
                .and_then(Value::as_u64)
                .and_then(|pid| u32::try_from(pid).ok())
                .ok_or_else(|| {
                    WorkcellError::InvalidDemand("place grant requires a numeric `pane_pid`".into())
                })?,
            process_start_marker: required_str(value, "process_start_marker")?,
            created_utc: required_str(value, "created_utc")?,
            grant_version: PLACE_GRANT_VERSION,
        })
    }
}

fn required_str(value: &Value, field: &str) -> Result<String, WorkcellError> {
    value
        .get(field)
        .and_then(Value::as_str)
        .map(str::to_owned)
        .ok_or_else(|| {
            WorkcellError::InvalidDemand(format!("place grant requires a string `{field}`"))
        })
}

/// A typed refusal. `kind` is machine-readable; `message` explains; `evidence`
/// carries what was actually observed (stderr, output excerpts, lookups).
#[derive(Debug, Clone, PartialEq)]
pub struct PlaceRefusal {
    pub kind: &'static str,
    pub provider: Option<&'static str>,
    pub message: String,
    pub evidence: Value,
}

impl PlaceRefusal {
    pub fn to_json(&self) -> Value {
        json!({
            "ok": false,
            "refusal": {
                "kind": self.kind,
                "provider": self.provider,
                "message": self.message,
                "evidence": self.evidence,
            },
        })
    }

    pub fn to_error(&self) -> WorkcellError {
        match self.kind {
            REFUSAL_INVALID_NAME | REFUSAL_INVALID_PLACE_REF => {
                WorkcellError::InvalidDemand(self.message.clone())
            }
            REFUSAL_ALREADY_EXISTS => WorkcellError::UnsatisfiedDemand(self.message.clone()),
            REFUSAL_NO_PROVIDER => WorkcellError::Unavailable(self.message.clone()),
            _ => WorkcellError::OperationFailed(self.message.clone()),
        }
    }
}

/// Provider selection for a place request.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PlacePolicy {
    /// herdr first when available and its server answers; tmux fallback;
    /// neither → a refusal naming what was tried.
    Auto,
    Herdr,
    Tmux,
}

impl PlacePolicy {
    pub fn parse(value: &str) -> Result<Self, WorkcellError> {
        match value {
            "auto" => Ok(Self::Auto),
            "herdr" => Ok(Self::Herdr),
            "tmux" => Ok(Self::Tmux),
            other => Err(WorkcellError::InvalidDemand(format!(
                "place provider `{other}` must be auto, herdr or tmux"
            ))),
        }
    }
}

/// A place name: `[a-z0-9-]{1,64}`. The name becomes a tmux session name and
/// a herdr workspace label; the tight charset keeps both providers happy and
/// the argv contract trivially safe.
pub fn validate_place_name(name: &str) -> Result<(), WorkcellError> {
    if place_name_is_valid(name) {
        Ok(())
    } else {
        Err(WorkcellError::InvalidDemand(place_name_error(name)))
    }
}

fn place_name_is_valid(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 64
        && name
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
}

/// The plain validation message, shared with the typed refusal so the CLI
/// never renders a doubled error prefix.
fn place_name_error(name: &str) -> String {
    format!("place name `{name}` must match [a-z0-9-]{{1,64}}")
}

/// WorkcellError's Display renders `"<kind>: <detail>"`; refusals carry the
/// kind separately, so the detail travels alone.
fn strip_error_prefix(display: &str) -> String {
    display
        .split_once(": ")
        .map(|(_, detail)| detail.to_owned())
        .unwrap_or_else(|| display.to_owned())
}

/// `workcell:place:tmux:default:<session_name>`
pub fn tmux_place_ref(session_name: &str) -> String {
    format!("{PLACE_REF_PREFIX}{TMUX_PROVIDER}:{TMUX_DEFAULT_SOCKET}:{session_name}")
}

/// `workcell:place:herdr:<workspace_id>:<pane_id>`
pub fn herdr_place_ref(workspace_id: &str, pane_id: &str) -> String {
    format!("{PLACE_REF_PREFIX}{HERDR_PROVIDER}:{workspace_id}:{pane_id}")
}

/// Parse a place_ref into `(provider, components)`. The address splits on its
/// FIRST colon only: herdr pane ids themselves contain colons (`w1:p1`), so
/// the second component keeps the remainder verbatim (tmux: `[socket,
/// session_name]`; herdr: `[workspace_id, pane_id]`).
pub fn parse_place_ref(place_ref: &str) -> Result<(&'static str, Vec<String>), WorkcellError> {
    let rest = place_ref.strip_prefix(PLACE_REF_PREFIX).ok_or_else(|| {
        WorkcellError::InvalidDemand(format!(
            "place_ref `{place_ref}` must read `{PLACE_REF_PREFIX}<provider>:<address>`"
        ))
    })?;
    let (provider, address) = rest.split_once(':').ok_or_else(|| {
        WorkcellError::InvalidDemand(format!(
            "place_ref `{place_ref}` must read `{PLACE_REF_PREFIX}<provider>:<address>`"
        ))
    })?;
    let components: Vec<String> = address.splitn(2, ':').map(str::to_owned).collect();
    let provider = match provider {
        TMUX_PROVIDER => TMUX_PROVIDER,
        HERDR_PROVIDER => HERDR_PROVIDER,
        other => {
            return Err(WorkcellError::InvalidDemand(format!(
                "place_ref provider `{other}` must be `{TMUX_PROVIDER}` or `{HERDR_PROVIDER}`"
            )))
        }
    };
    Ok((provider, components))
}

/// Request a place under the given policy. Live path: spawns only the
/// provider commands the module doc bounds.
pub fn request_place_live(policy: PlacePolicy, name: &str) -> Result<PlaceGrant, PlaceRefusal> {
    validate_place_name(name).map_err(|_| PlaceRefusal {
        kind: REFUSAL_INVALID_NAME,
        provider: None,
        message: place_name_error(name),
        evidence: json!({ "name": name }),
    })?;
    match policy {
        PlacePolicy::Tmux => request_tmux_place(name),
        PlacePolicy::Herdr => request_herdr_place(name),
        PlacePolicy::Auto => {
            // auto law: herdr first when the CLI is available AND its server
            // answers; tmux fallback; otherwise a refusal naming both tries.
            let mut evidence = serde_json::Map::new();
            if herdr_server_reachable() {
                match request_herdr_place(name) {
                    Ok(grant) => return Ok(grant),
                    Err(refusal) => {
                        evidence.insert("herdr".into(), refusal.to_json()["refusal"].clone());
                    }
                }
            } else {
                evidence.insert(
                    "herdr".into(),
                    json!({ "refusal": { "kind": REFUSAL_NO_PROVIDER,
                        "message": "herdr CLI unavailable or its server did not answer" } }),
                );
            }
            if tmux_available() {
                match request_tmux_place(name) {
                    Ok(grant) => return Ok(grant),
                    Err(refusal) => {
                        evidence.insert("tmux".into(), refusal.to_json()["refusal"].clone());
                        return Err(PlaceRefusal {
                            kind: refusal.kind,
                            provider: Some(TMUX_PROVIDER),
                            message: refusal.message,
                            evidence: json!({ "auto_attempted": evidence }),
                        });
                    }
                }
            }
            Err(PlaceRefusal {
                kind: REFUSAL_NO_PROVIDER,
                provider: None,
                message: "no place provider could serve this request: herdr was unavailable or \
                          its server did not answer, and tmux is not installed"
                    .to_owned(),
                evidence: json!({ "auto_attempted": evidence }),
            })
        }
    }
}

fn tmux_available() -> bool {
    matches!(
        Command::new("tmux").arg("-V").stdin(Stdio::null()).output(),
        Ok(output) if output.status.success()
    )
}

/// Read-only herdr reachability probe: the server answers `workspace list`.
fn herdr_server_reachable() -> bool {
    Command::new("herdr")
        .args(["workspace", "list"])
        .stdin(Stdio::null())
        .output()
        .map(|output| output.status.success())
        .unwrap_or(false)
}

/// tmux request: refuse an existing session by name (`already-exists`), then
/// exactly one detached `new-session`, then read-only observation of the
/// created pane.
pub fn request_tmux_place(name: &str) -> Result<PlaceGrant, PlaceRefusal> {
    validate_place_name(name).map_err(|_| PlaceRefusal {
        kind: REFUSAL_INVALID_NAME,
        provider: Some(TMUX_PROVIDER),
        message: place_name_error(name),
        evidence: json!({ "name": name }),
    })?;

    // has-session is read-only. Exit 0 = the name is taken (refuse, never
    // adopt). "no server running" / "can't find session" both mean the name
    // is free. Any other failure is named and stops the request.
    let has = Command::new("tmux")
        .args(["has-session", "-t", name])
        .stdin(Stdio::null())
        .output()
        .map_err(|error| PlaceRefusal {
            kind: REFUSAL_PROVIDER_ERROR,
            provider: Some(TMUX_PROVIDER),
            message: format!("run `tmux has-session -t {name}`: {error}"),
            evidence: Value::Null,
        })?;
    let has_stderr = String::from_utf8_lossy(&has.stderr).trim().to_owned();
    if has.status.success() {
        return Err(PlaceRefusal {
            kind: REFUSAL_ALREADY_EXISTS,
            provider: Some(TMUX_PROVIDER),
            message: format!(
                "a tmux session named `{name}` already exists; adoption is a \
                              separate explicit operation, not a silent side effect"
            ),
            evidence: json!({
                "session_name": name,
                "has_session_stderr": has_stderr,
            }),
        });
    }
    let name_free =
        has_stderr.contains("no server running") || has_stderr.contains("can't find session");
    if !name_free {
        return Err(PlaceRefusal {
            kind: REFUSAL_PROVIDER_ERROR,
            provider: Some(TMUX_PROVIDER),
            message: format!("`tmux has-session -t {name}` failed unexpectedly: {has_stderr}"),
            evidence: json!({ "session_name": name, "stderr": has_stderr }),
        });
    }

    // The one bounded effect: a single detached session, argv-built.
    let created = Command::new("tmux")
        .args(["new-session", "-d", "-s", name])
        .stdin(Stdio::null())
        .output()
        .map_err(|error| PlaceRefusal {
            kind: REFUSAL_PROVIDER_ERROR,
            provider: Some(TMUX_PROVIDER),
            message: format!("run `tmux new-session -d -s {name}`: {error}"),
            evidence: json!({ "session_name": name }),
        })?;
    if !created.status.success() {
        let stderr = String::from_utf8_lossy(&created.stderr).trim().to_owned();
        // A duplicate racing between the probe and the create is the
        // already-exists refusal, evidenced by tmux's own duplicate error.
        let kind = if stderr.contains("duplicate session") {
            REFUSAL_ALREADY_EXISTS
        } else {
            REFUSAL_PROVIDER_ERROR
        };
        return Err(PlaceRefusal {
            kind,
            provider: Some(TMUX_PROVIDER),
            message: format!("`tmux new-session -d -s {name}` failed: {stderr}"),
            evidence: json!({ "session_name": name, "stderr": stderr }),
        });
    }

    // Read-only read-back of the created session's panes.
    let stdout = match run_tmux_session_panes(name) {
        Ok(stdout) => stdout,
        Err(TmuxListError::NoServer(stderr)) | Err(TmuxListError::Failed(stderr)) => {
            return Err(PlaceRefusal {
                kind: REFUSAL_PROVIDER_ERROR,
                provider: Some(TMUX_PROVIDER),
                message: format!(
                    "created tmux session `{name}` but reading its panes failed: {stderr}"
                ),
                evidence: json!({ "session_name": name, "stderr": stderr }),
            })
        }
    };
    let (rows, _) = parse_tmux_list_panes(&stdout);
    let observations = tmux_rows_to_observations(rows);
    let pane = observations.first().ok_or_else(|| PlaceRefusal {
        kind: REFUSAL_PROVIDER_ERROR,
        provider: Some(TMUX_PROVIDER),
        message: format!("created tmux session `{name}` but it reported no panes to observe"),
        evidence: json!({ "session_name": name }),
    })?;
    let pane_pid = pane.pane_pid.ok_or_else(|| PlaceRefusal {
        kind: REFUSAL_PROVIDER_ERROR,
        provider: Some(TMUX_PROVIDER),
        message: format!(
            "tmux session `{name}` pane {} reported no pid; a grant without start evidence \
             cannot be released responsibly",
            pane.pane_id
        ),
        evidence: json!({ "session_name": name, "pane_id": pane.pane_id }),
    })?;
    let start_marker = live_start_marker(pane_pid).ok_or_else(|| PlaceRefusal {
        kind: REFUSAL_PROVIDER_ERROR,
        provider: Some(TMUX_PROVIDER),
        message: format!(
            "could not read a process start marker for pid {pane_pid}; the local ps provider \
             is the evidence source for the generation proof"
        ),
        evidence: json!({ "session_name": name, "pane_pid": pane_pid }),
    })?;

    Ok(PlaceGrant {
        place_ref: tmux_place_ref(name),
        provider: TMUX_PROVIDER,
        session_name: name.to_owned(),
        workspace_id: None,
        pane_id: pane.pane_id.clone(),
        pane_pid,
        process_start_marker: start_marker,
        created_utc: utc_now_rfc3339(),
        grant_version: PLACE_GRANT_VERSION,
    })
}

/// Read-only pane listing scoped to one session (the request/release
/// read-back; the census uses the socket-wide `-a` form).
fn run_tmux_session_panes(session: &str) -> std::result::Result<String, TmuxListError> {
    let output = Command::new("tmux")
        .args([
            "list-panes",
            "-t",
            session,
            "-F",
            crate::place_scan::TMUX_PANE_FORMAT,
        ])
        .stdin(Stdio::null())
        .output()
        .map_err(|error| {
            TmuxListError::Failed(format!("run `tmux list-panes -t {session}`: {error}"))
        })?;
    let stderr = String::from_utf8_lossy(&output.stderr).trim().to_owned();
    if !output.status.success() {
        if stderr.contains("no server running") {
            return Err(TmuxListError::NoServer(stderr));
        }
        return Err(TmuxListError::Failed(stderr));
    }
    Ok(String::from_utf8_lossy(&output.stdout).into_owned())
}

/// herdr request: only what the proven CLI surface supports. `workspace
/// create --label` exists; if its output does not yield a workspace id, the
/// refusal is typed `provider-cannot-create` naming the missing capability —
/// never a fabricated grant.
pub fn request_herdr_place(name: &str) -> Result<PlaceGrant, PlaceRefusal> {
    validate_place_name(name).map_err(|_| PlaceRefusal {
        kind: REFUSAL_INVALID_NAME,
        provider: Some(HERDR_PROVIDER),
        message: place_name_error(name),
        evidence: json!({ "name": name }),
    })?;

    // already-exists parity with tmux: a workspace with this label refuses.
    let existing = Command::new("herdr")
        .args(["workspace", "list"])
        .stdin(Stdio::null())
        .output()
        .map_err(|error| PlaceRefusal {
            kind: REFUSAL_PROVIDER_ERROR,
            provider: Some(HERDR_PROVIDER),
            message: format!("run `herdr workspace list`: {error}"),
            evidence: Value::Null,
        })?;
    if !existing.status.success() {
        let stderr = String::from_utf8_lossy(&existing.stderr).trim().to_owned();
        return Err(PlaceRefusal {
            kind: REFUSAL_NO_PROVIDER,
            provider: Some(HERDR_PROVIDER),
            message: format!("the herdr server did not answer `workspace list`: {stderr}"),
            evidence: json!({ "stderr": stderr }),
        });
    }
    let existing_workspaces =
        parse_herdr_workspace_list(&String::from_utf8_lossy(&existing.stdout)).unwrap_or_default();
    if existing_workspaces
        .iter()
        .any(|workspace| workspace.label == name)
    {
        return Err(PlaceRefusal {
            kind: REFUSAL_ALREADY_EXISTS,
            provider: Some(HERDR_PROVIDER),
            message: format!(
                "a herdr workspace labelled `{name}` already exists; adoption is a \
                              separate explicit operation, not a silent side effect"
            ),
            evidence: json!({
                "label": name,
                "existing": existing_workspaces.iter()
                    .filter(|workspace| workspace.label == name)
                    .map(|workspace| json!({
                        "workspace_id": workspace.workspace_id,
                        "label": workspace.label,
                    }))
                    .collect::<Vec<_>>(),
            }),
        });
    }

    let created = Command::new("herdr")
        .args(["workspace", "create", "--label", name])
        .stdin(Stdio::null())
        .output()
        .map_err(|error| PlaceRefusal {
            kind: REFUSAL_PROVIDER_ERROR,
            provider: Some(HERDR_PROVIDER),
            message: format!("run `herdr workspace create --label {name}`: {error}"),
            evidence: Value::Null,
        })?;
    let stdout = String::from_utf8_lossy(&created.stdout).into_owned();
    let stderr = String::from_utf8_lossy(&created.stderr).trim().to_owned();
    if !created.status.success() {
        return Err(PlaceRefusal {
            kind: REFUSAL_PROVIDER_ERROR,
            provider: Some(HERDR_PROVIDER),
            message: format!("`herdr workspace create --label {name}` failed: {stderr}"),
            evidence: json!({ "stderr": stderr }),
        });
    }

    // Read the created workspace id out of the CLI's JSON envelope. Both the
    // `result.workspace.workspace_id` and `result.workspace_id` shapes are
    // accepted; anything else is a typed cannot-create refusal with the raw
    // output as evidence.
    let parsed: Value = serde_json::from_str(stdout.trim()).unwrap_or(Value::Null);
    let workspace_id = parsed
        .pointer("/result/workspace/workspace_id")
        .or_else(|| parsed.pointer("/result/workspace_id"))
        .and_then(Value::as_str)
        .map(str::to_owned);
    let Some(workspace_id) = workspace_id else {
        return Err(PlaceRefusal {
            kind: REFUSAL_PROVIDER_CANNOT_CREATE,
            provider: Some(HERDR_PROVIDER),
            message: "`herdr workspace create` succeeded but its output did not yield a \
                      workspace_id; Workcell will not mint a place grant it cannot address"
                .to_owned(),
            evidence: json!({
                "stdout_excerpt": truncate(&stdout, 2048),
                "stderr": stderr,
            }),
        });
    };

    // Observe the created workspace's first pane for the generation proof.
    let panes = herdr_workspace_panes(&workspace_id).map_err(|message| PlaceRefusal {
        kind: REFUSAL_PROVIDER_CANNOT_CREATE,
        provider: Some(HERDR_PROVIDER),
        message: format!(
            "created herdr workspace `{workspace_id}` but its panes could not be read: {message}. \
             Close the workspace manually or adopt it explicitly; Workcell leaves no untracked \
             place unmentioned"
        ),
        evidence: json!({ "workspace_id": workspace_id }),
    })?;
    let pane = panes.first().ok_or_else(|| PlaceRefusal {
        kind: REFUSAL_PROVIDER_CANNOT_CREATE,
        provider: Some(HERDR_PROVIDER),
        message: format!(
            "created herdr workspace `{workspace_id}` but it reported no panes; close it \
             manually or adopt it explicitly"
        ),
        evidence: json!({ "workspace_id": workspace_id }),
    })?;
    let process_info = herdr_pane_process(&pane.pane_id).ok_or_else(|| PlaceRefusal {
        kind: REFUSAL_PROVIDER_CANNOT_CREATE,
        provider: Some(HERDR_PROVIDER),
        message: format!(
            "created herdr workspace `{workspace_id}` but pane {} reported no process \
                 information; a grant without a pid cannot be released responsibly",
            pane.pane_id
        ),
        evidence: json!({ "workspace_id": workspace_id, "pane_id": pane.pane_id }),
    })?;
    let pane_pid = process_info.shell_pid.ok_or_else(|| PlaceRefusal {
        kind: REFUSAL_PROVIDER_CANNOT_CREATE,
        provider: Some(HERDR_PROVIDER),
        message: format!(
            "herdr pane {} reported no shell pid; a grant without a pid cannot be released \
             responsibly",
            pane.pane_id
        ),
        evidence: json!({ "workspace_id": workspace_id, "pane_id": pane.pane_id }),
    })?;
    let start_marker = live_start_marker(pane_pid).ok_or_else(|| PlaceRefusal {
        kind: REFUSAL_PROVIDER_ERROR,
        provider: Some(HERDR_PROVIDER),
        message: format!(
            "could not read a process start marker for pid {pane_pid}; the local ps provider \
             is the evidence source for the generation proof"
        ),
        evidence: json!({ "workspace_id": workspace_id, "pane_pid": pane_pid }),
    })?;

    Ok(PlaceGrant {
        place_ref: herdr_place_ref(&workspace_id, &pane.pane_id),
        provider: HERDR_PROVIDER,
        session_name: name.to_owned(),
        workspace_id: Some(workspace_id),
        pane_id: pane.pane_id.clone(),
        pane_pid,
        process_start_marker: start_marker,
        created_utc: utc_now_rfc3339(),
        grant_version: PLACE_GRANT_VERSION,
    })
}

fn herdr_workspace_panes(workspace_id: &str) -> Result<Vec<HerdrPaneRow>, String> {
    let output = Command::new("herdr")
        .args(["pane", "list"])
        .stdin(Stdio::null())
        .output()
        .map_err(|error| format!("run `herdr pane list`: {error}"))?;
    if !output.status.success() {
        return Err(format!(
            "`herdr pane list` exited {} with stderr: {}",
            output.status,
            String::from_utf8_lossy(&output.stderr).trim()
        ));
    }
    let panes = parse_herdr_pane_list(&String::from_utf8_lossy(&output.stdout))?;
    Ok(panes
        .into_iter()
        .filter(|pane| pane.workspace_id.as_deref() == Some(workspace_id))
        .collect())
}

fn herdr_pane_process(pane_id: &str) -> Option<crate::place_scan::HerdrProcessInfo> {
    let output = Command::new("herdr")
        .args(["pane", "process-info", "--pane", pane_id])
        .stdin(Stdio::null())
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    parse_herdr_process_info(&String::from_utf8_lossy(&output.stdout)).ok()
}

/// The live `ps lstart` marker for one pid, when the pid table names it.
fn live_start_marker(pid: u32) -> Option<String> {
    read_pid_table()
        .ok()?
        .into_iter()
        .find(|process| process.pid == pid)
        .map(|process| process.start_marker)
        .filter(|marker| !marker.is_empty())
}

// ---------------------------------------------------------------------------
// release
// ---------------------------------------------------------------------------

/// A release demand: which place, and the process generation the grant
/// recorded. Both parts are required — a place_ref alone proves nothing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlaceReleaseDemand {
    pub place_ref: String,
    pub pid: u32,
    pub start_marker: String,
}

/// The provider state the release decision needs, gathered live or supplied
/// by tests as fixtures.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProviderSnapshot {
    Tmux {
        /// Whether `tmux has-session` succeeded for the named session.
        session_exists: bool,
        /// The session's panes (pid, start-marker evidence comes from the
        /// pid table, not from tmux).
        pane_pids: Vec<u32>,
    },
    Herdr {
        /// The workspace's panes, already filtered to the granted workspace.
        pane_pids: Vec<u32>,
    },
}

/// The release decision. The kill happens only on `Release`, and only in the
/// live executor.
#[derive(Debug, Clone, PartialEq)]
pub enum PlaceReleaseDecision {
    Release {
        provider: &'static str,
        session_name: Option<String>,
        workspace_id: Option<String>,
    },
    Refuse(PlaceRefusal),
}

/// Pure decision core: prove the granted process generation is still the one
/// bound to the place before any release. Fixture-friendly — no provider
/// command runs here.
pub fn decide_place_release(
    demand: &PlaceReleaseDemand,
    pid_table: &[ObservedProcess],
    snapshot: &ProviderSnapshot,
) -> PlaceReleaseDecision {
    let (provider, components) = match parse_place_ref(&demand.place_ref) {
        Ok(parsed) => parsed,
        Err(error) => {
            return PlaceReleaseDecision::Refuse(PlaceRefusal {
                kind: REFUSAL_INVALID_PLACE_REF,
                provider: None,
                message: strip_error_prefix(&error.to_string()),
                evidence: json!({ "place_ref": demand.place_ref }),
            })
        }
    };

    // Process-generation law: the live pid must still be the granted
    // generation. Absent pid or a different start marker is a stale binding,
    // named with both markers — never killed through.
    let live = pid_table.iter().find(|process| process.pid == demand.pid);
    let live_marker = live
        .map(|process| process.start_marker.clone())
        .filter(|marker| !marker.is_empty());
    let Some(live_marker) = live_marker else {
        return PlaceReleaseDecision::Refuse(PlaceRefusal {
            kind: REFUSAL_STALE_BINDING,
            provider: Some(provider),
            message: format!(
                "pid {} is not in the live pid table; the granted process generation is gone, \
                 so the place cannot be proven to still host it",
                demand.pid
            ),
            evidence: json!({
                "place_ref": demand.place_ref,
                "granted_pid": demand.pid,
                "granted_start_marker": demand.start_marker,
                "live_start_marker": Value::Null,
            }),
        });
    };
    if live_marker != demand.start_marker {
        return PlaceReleaseDecision::Refuse(PlaceRefusal {
            kind: REFUSAL_STALE_BINDING,
            provider: Some(provider),
            message: format!(
                "pid {} was granted with start marker `{}` but now carries `{}` — the pid was \
                 recycled to a different process generation; refusing to release through a \
                 stale binding",
                demand.pid, demand.start_marker, live_marker
            ),
            evidence: json!({
                "place_ref": demand.place_ref,
                "granted_pid": demand.pid,
                "granted_start_marker": demand.start_marker,
                "live_start_marker": live_marker,
            }),
        });
    }

    match (provider, snapshot) {
        (
            TMUX_PROVIDER,
            ProviderSnapshot::Tmux {
                session_exists,
                pane_pids,
            },
        ) => {
            let Some(session_name) = components.get(1) else {
                return PlaceReleaseDecision::Refuse(PlaceRefusal {
                    kind: REFUSAL_INVALID_PLACE_REF,
                    provider: Some(TMUX_PROVIDER),
                    message: format!(
                        "place_ref `{}` must read `{}tmux:<socket>:<session_name>`",
                        demand.place_ref, PLACE_REF_PREFIX
                    ),
                    evidence: json!({ "place_ref": demand.place_ref }),
                });
            };
            if !session_exists {
                return PlaceReleaseDecision::Refuse(PlaceRefusal {
                    kind: REFUSAL_PLACE_GONE,
                    provider: Some(TMUX_PROVIDER),
                    message: format!(
                        "tmux session `{session_name}` no longer exists; nothing to release"
                    ),
                    evidence: json!({ "session_name": session_name }),
                });
            }
            if !pane_pids.contains(&demand.pid) {
                return PlaceReleaseDecision::Refuse(PlaceRefusal {
                    kind: REFUSAL_PLACE_MISMATCH,
                    provider: Some(TMUX_PROVIDER),
                    message: format!(
                        "pid {} is live with the granted start marker but is not a pane process \
                         of tmux session `{session_name}`; the place was rebound to a different \
                         process generation",
                        demand.pid
                    ),
                    evidence: json!({
                        "session_name": session_name,
                        "pane_pids": pane_pids,
                        "granted_pid": demand.pid,
                    }),
                });
            }
            PlaceReleaseDecision::Release {
                provider: TMUX_PROVIDER,
                session_name: Some(session_name.clone()),
                workspace_id: None,
            }
        }
        (HERDR_PROVIDER, ProviderSnapshot::Herdr { pane_pids }) => {
            let Some(workspace_id) = components.first() else {
                return PlaceReleaseDecision::Refuse(PlaceRefusal {
                    kind: REFUSAL_INVALID_PLACE_REF,
                    provider: Some(HERDR_PROVIDER),
                    message: format!(
                        "place_ref `{}` must read `{}herdr:<workspace_id>:<pane_id>`",
                        demand.place_ref, PLACE_REF_PREFIX
                    ),
                    evidence: json!({ "place_ref": demand.place_ref }),
                });
            };
            if !pane_pids.contains(&demand.pid) {
                return PlaceReleaseDecision::Refuse(PlaceRefusal {
                    kind: REFUSAL_PLACE_MISMATCH,
                    provider: Some(HERDR_PROVIDER),
                    message: format!(
                        "pid {} is live with the granted start marker but is not a pane process \
                         of herdr workspace `{workspace_id}`",
                        demand.pid
                    ),
                    evidence: json!({
                        "workspace_id": workspace_id,
                        "pane_pids": pane_pids,
                        "granted_pid": demand.pid,
                    }),
                });
            }
            PlaceReleaseDecision::Release {
                provider: HERDR_PROVIDER,
                session_name: None,
                workspace_id: Some(workspace_id.clone()),
            }
        }
        (provider, _) => PlaceReleaseDecision::Refuse(PlaceRefusal {
            kind: REFUSAL_INVALID_PLACE_REF,
            provider: Some(provider),
            message: format!(
                "place_ref `{}` names provider `{provider}` but the release demand supplied a \
                 different provider snapshot",
                demand.place_ref
            ),
            evidence: json!({ "place_ref": demand.place_ref }),
        }),
    }
}

/// Live release: gather the pid table and the provider snapshot, run the
/// decision core, and only then execute the single bounded effect (`tmux
/// kill-session -t <session>` / `herdr workspace close <workspace_id>`).
pub fn release_place_live(demand: &PlaceReleaseDemand) -> Result<Value, PlaceRefusal> {
    let (provider, _) = parse_place_ref(&demand.place_ref).map_err(|error| PlaceRefusal {
        kind: REFUSAL_INVALID_PLACE_REF,
        provider: None,
        message: strip_error_prefix(&error.to_string()),
        evidence: json!({ "place_ref": demand.place_ref }),
    })?;

    let pid_table = read_pid_table().unwrap_or_default();
    let snapshot = gather_provider_snapshot(provider, demand)?;
    match decide_place_release(demand, &pid_table, &snapshot) {
        PlaceReleaseDecision::Release {
            provider,
            session_name,
            workspace_id,
        } => execute_release(
            provider,
            session_name.as_deref(),
            workspace_id.as_deref(),
            demand,
        ),
        PlaceReleaseDecision::Refuse(refusal) => Err(refusal),
    }
}

fn gather_provider_snapshot(
    provider: &'static str,
    demand: &PlaceReleaseDemand,
) -> Result<ProviderSnapshot, PlaceRefusal> {
    match provider {
        TMUX_PROVIDER => {
            let (_, components) =
                parse_place_ref(&demand.place_ref).map_err(|error| PlaceRefusal {
                    kind: REFUSAL_INVALID_PLACE_REF,
                    provider: Some(TMUX_PROVIDER),
                    message: strip_error_prefix(&error.to_string()),
                    evidence: json!({ "place_ref": demand.place_ref }),
                })?;
            let session_name = components.get(1).cloned().unwrap_or_default();
            let has = Command::new("tmux")
                .args(["has-session", "-t", &session_name])
                .stdin(Stdio::null())
                .output()
                .map_err(|error| PlaceRefusal {
                    kind: REFUSAL_PROVIDER_ERROR,
                    provider: Some(TMUX_PROVIDER),
                    message: format!("run `tmux has-session -t {session_name}`: {error}"),
                    evidence: Value::Null,
                })?;
            if !has.status.success() {
                // Missing session or no server: the place is gone either way.
                return Ok(ProviderSnapshot::Tmux {
                    session_exists: false,
                    pane_pids: Vec::new(),
                });
            }
            let stdout = run_tmux_session_panes(&session_name).map_err(|error| {
                let stderr = match error {
                    TmuxListError::NoServer(stderr) | TmuxListError::Failed(stderr) => stderr,
                };
                PlaceRefusal {
                    kind: REFUSAL_PROVIDER_ERROR,
                    provider: Some(TMUX_PROVIDER),
                    message: format!(
                        "reading panes of tmux session `{session_name}` failed: {stderr}"
                    ),
                    evidence: json!({ "stderr": stderr }),
                }
            })?;
            let (rows, _) = parse_tmux_list_panes(&stdout);
            Ok(ProviderSnapshot::Tmux {
                session_exists: true,
                pane_pids: rows.iter().filter_map(|row| row.pane_pid).collect(),
            })
        }
        HERDR_PROVIDER => {
            let (_, components) =
                parse_place_ref(&demand.place_ref).map_err(|error| PlaceRefusal {
                    kind: REFUSAL_INVALID_PLACE_REF,
                    provider: Some(HERDR_PROVIDER),
                    message: strip_error_prefix(&error.to_string()),
                    evidence: json!({ "place_ref": demand.place_ref }),
                })?;
            let workspace_id = components.first().cloned().unwrap_or_default();
            let stdout = Command::new("herdr")
                .args(["pane", "list"])
                .stdin(Stdio::null())
                .output()
                .map_err(|error| PlaceRefusal {
                    kind: REFUSAL_PROVIDER_ERROR,
                    provider: Some(HERDR_PROVIDER),
                    message: format!("run `herdr pane list`: {error}"),
                    evidence: Value::Null,
                })
                .and_then(|output| {
                    if output.status.success() {
                        Ok(String::from_utf8_lossy(&output.stdout).into_owned())
                    } else {
                        Err(PlaceRefusal {
                            kind: REFUSAL_NO_PROVIDER,
                            provider: Some(HERDR_PROVIDER),
                            message: format!(
                                "the herdr server did not answer `pane list`: {}",
                                String::from_utf8_lossy(&output.stderr).trim()
                            ),
                            evidence: Value::Null,
                        })
                    }
                })?;
            let pane_pids = parse_herdr_pane_list(&stdout)
                .unwrap_or_default()
                .into_iter()
                .filter(|pane| pane.workspace_id.as_deref() == Some(workspace_id.as_str()))
                .filter_map(|pane| herdr_pane_process(&pane.pane_id))
                .filter_map(|info| info.shell_pid)
                .collect();
            Ok(ProviderSnapshot::Herdr { pane_pids })
        }
        other => Err(PlaceRefusal {
            kind: REFUSAL_INVALID_PLACE_REF,
            provider: None,
            message: format!("place_ref provider `{other}` is not releaseable"),
            evidence: json!({ "place_ref": demand.place_ref }),
        }),
    }
}

fn execute_release(
    provider: &'static str,
    session_name: Option<&str>,
    workspace_id: Option<&str>,
    demand: &PlaceReleaseDemand,
) -> Result<Value, PlaceRefusal> {
    match (provider, session_name, workspace_id) {
        (TMUX_PROVIDER, Some(session_name), _) => {
            let output = Command::new("tmux")
                .args(["kill-session", "-t", session_name])
                .stdin(Stdio::null())
                .output()
                .map_err(|error| PlaceRefusal {
                    kind: REFUSAL_PROVIDER_ERROR,
                    provider: Some(TMUX_PROVIDER),
                    message: format!("run `tmux kill-session -t {session_name}`: {error}"),
                    evidence: Value::Null,
                })?;
            if !output.status.success() {
                return Err(PlaceRefusal {
                    kind: REFUSAL_PROVIDER_ERROR,
                    provider: Some(TMUX_PROVIDER),
                    message: format!(
                        "`tmux kill-session -t {session_name}` failed: {}",
                        String::from_utf8_lossy(&output.stderr).trim()
                    ),
                    evidence: json!({ "session_name": session_name }),
                });
            }
            Ok(json!({
                "ok": true,
                "released": true,
                "place_ref": demand.place_ref,
                "provider": TMUX_PROVIDER,
                "action": "tmux kill-session",
                "proved_generation": {
                    "pid": demand.pid,
                    "process_start_marker": demand.start_marker,
                },
            }))
        }
        (HERDR_PROVIDER, _, Some(workspace_id)) => {
            let output = Command::new("herdr")
                .args(["workspace", "close", workspace_id])
                .stdin(Stdio::null())
                .output()
                .map_err(|error| PlaceRefusal {
                    kind: REFUSAL_PROVIDER_ERROR,
                    provider: Some(HERDR_PROVIDER),
                    message: format!("run `herdr workspace close {workspace_id}`: {error}"),
                    evidence: Value::Null,
                })?;
            if !output.status.success() {
                return Err(PlaceRefusal {
                    kind: REFUSAL_PROVIDER_ERROR,
                    provider: Some(HERDR_PROVIDER),
                    message: format!(
                        "`herdr workspace close {workspace_id}` failed: {}",
                        String::from_utf8_lossy(&output.stderr).trim()
                    ),
                    evidence: json!({ "workspace_id": workspace_id }),
                });
            }
            Ok(json!({
                "ok": true,
                "released": true,
                "place_ref": demand.place_ref,
                "provider": HERDR_PROVIDER,
                "action": "herdr workspace close",
                "proved_generation": {
                    "pid": demand.pid,
                    "process_start_marker": demand.start_marker,
                },
            }))
        }
        _ => Err(PlaceRefusal {
            kind: REFUSAL_INVALID_PLACE_REF,
            provider: Some(provider),
            message: format!(
                "place_ref `{}` lacks the address its provider needs to release",
                demand.place_ref
            ),
            evidence: json!({ "place_ref": demand.place_ref }),
        }),
    }
}

fn truncate(value: &str, max: usize) -> String {
    if value.len() <= max {
        value.to_owned()
    } else {
        let mut end = max;
        while !value.is_char_boundary(end) {
            end -= 1;
        }
        format!("{}…[truncated]", &value[..end])
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::instance_scan::ObservedProcess;

    const MARKER_A: &str = "Thu Sep 14 00:01:50 2026";
    const MARKER_B: &str = "Thu Sep 14 00:09:09 2026";

    fn pid_table() -> Vec<ObservedProcess> {
        vec![
            // 4242 was recycled: the grant recorded MARKER_A, the live table
            // now carries MARKER_B for the same pid number.
            ObservedProcess {
                pid: 4242,
                start_marker: MARKER_B.into(),
                comm: "zsh".into(),
            },
            ObservedProcess {
                pid: 5000,
                start_marker: MARKER_B.into(),
                comm: "zsh".into(),
            },
        ]
    }

    fn tmux_demand(pid: u32, marker: &str) -> PlaceReleaseDemand {
        PlaceReleaseDemand {
            place_ref: tmux_place_ref("agent-test"),
            pid,
            start_marker: marker.to_owned(),
        }
    }

    #[test]
    fn grant_version_is_the_published_contract() {
        assert_eq!(PLACE_GRANT_VERSION, "workcell.place-grant/v1");
    }

    #[test]
    fn place_names_are_tightly_bounded() {
        assert!(validate_place_name("agent-1").is_ok());
        assert!(validate_place_name("a").is_ok());
        assert!(validate_place_name(&"a".repeat(64)).is_ok());

        for bad in [
            "",              // empty
            "Agent-1",       // uppercase
            "agent_1",       // underscore
            "agent 1",       // space
            "agent;rm",      // shell-looking
            &"a".repeat(65), // too long
        ] {
            let error = validate_place_name(bad).unwrap_err().to_string();
            assert!(
                error.contains("[a-z0-9-]"),
                "`{bad}` refused by name: {error}"
            );
        }
    }

    #[test]
    fn place_refs_round_trip_through_the_parser() {
        let tmux_ref = tmux_place_ref("agent-test");
        assert_eq!(tmux_ref, "workcell:place:tmux:default:agent-test");
        let (provider, components) = parse_place_ref(&tmux_ref).unwrap();
        assert_eq!(provider, TMUX_PROVIDER);
        assert_eq!(
            components,
            vec!["default".to_owned(), "agent-test".to_owned()]
        );

        let herdr_ref = herdr_place_ref("w9", "w9:p1");
        assert_eq!(herdr_ref, "workcell:place:herdr:w9:w9:p1");
        let (provider, components) = parse_place_ref(&herdr_ref).unwrap();
        assert_eq!(provider, HERDR_PROVIDER);
        assert_eq!(components, vec!["w9".to_owned(), "w9:p1".to_owned()]);

        for bad in [
            "instance:hermes:abc",
            "workcell:place:tmux",
            "workcell:place:window:1:2",
        ] {
            assert!(parse_place_ref(bad).is_err(), "`{bad}` must not parse");
        }
    }

    fn grant() -> PlaceGrant {
        PlaceGrant {
            place_ref: tmux_place_ref("agent-test"),
            provider: TMUX_PROVIDER,
            session_name: "agent-test".into(),
            workspace_id: None,
            pane_id: "%7".into(),
            pane_pid: 4242,
            process_start_marker: MARKER_A.into(),
            created_utc: "2026-09-15T12:00:00Z".into(),
            grant_version: PLACE_GRANT_VERSION,
        }
    }

    #[test]
    fn place_grant_serialization_round_trip_keeps_every_field() {
        let value = grant().to_json();
        assert_eq!(value["grant_version"], PLACE_GRANT_VERSION);
        assert_eq!(value["place_ref"], "workcell:place:tmux:default:agent-test");
        assert_eq!(value["provider"], TMUX_PROVIDER);
        assert_eq!(value["session_name"], "agent-test");
        assert_eq!(value["workspace_id"], Value::Null);
        assert_eq!(value["pane_id"], "%7");
        assert_eq!(value["pane_pid"], 4242);
        assert_eq!(value["process_start_marker"], MARKER_A);
        assert_eq!(value["created_utc"], "2026-09-15T12:00:00Z");

        let round_tripped = PlaceGrant::from_json(&value).unwrap();
        assert_eq!(round_tripped, grant());
    }

    #[test]
    fn herdr_grant_round_trip_carries_the_workspace_id() {
        let value = PlaceGrant {
            place_ref: herdr_place_ref("w9", "w9:p1"),
            provider: HERDR_PROVIDER,
            session_name: "agent-test".into(),
            workspace_id: Some("w9".into()),
            pane_id: "w9:p1".into(),
            pane_pid: 77,
            process_start_marker: MARKER_B.into(),
            created_utc: "2026-09-15T12:00:00Z".into(),
            grant_version: PLACE_GRANT_VERSION,
        }
        .to_json();
        let round_tripped = PlaceGrant::from_json(&value).unwrap();
        assert_eq!(round_tripped.workspace_id.as_deref(), Some("w9"));
        assert_eq!(round_tripped.provider, HERDR_PROVIDER);
    }

    #[test]
    fn grant_round_trip_refuses_a_foreign_version() {
        let mut value = grant().to_json();
        value["grant_version"] = "workcell.place-grant/v0".into();
        let error = PlaceGrant::from_json(&value).unwrap_err().to_string();
        assert!(error.contains("workcell.place-grant/v1"), "{error}");
    }

    #[test]
    fn release_refuses_when_the_pid_is_gone() {
        // Process-generation law: no live pid, no proof, no kill.
        let empty = Vec::new();
        let decision = decide_place_release(
            &tmux_demand(4242, MARKER_A),
            &empty,
            &ProviderSnapshot::Tmux {
                session_exists: true,
                pane_pids: vec![4242],
            },
        );
        let PlaceReleaseDecision::Refuse(refusal) = decision else {
            panic!("expected a refusal, got {decision:?}");
        };
        assert_eq!(refusal.kind, REFUSAL_STALE_BINDING);
        assert!(refusal.message.contains("not in the live pid table"));
        assert_eq!(refusal.evidence["granted_start_marker"], MARKER_A);
        assert!(refusal.evidence["live_start_marker"].is_null());
    }

    #[test]
    fn release_refuses_a_recycled_pid_as_a_stale_binding() {
        let decision = decide_place_release(
            &tmux_demand(4242, MARKER_A),
            &pid_table(),
            &ProviderSnapshot::Tmux {
                session_exists: true,
                pane_pids: vec![4242],
            },
        );
        let PlaceReleaseDecision::Refuse(refusal) = decision else {
            panic!("expected a refusal, got {decision:?}");
        };
        assert_eq!(refusal.kind, REFUSAL_STALE_BINDING);
        assert!(refusal.message.contains("recycled"));
        assert_eq!(refusal.evidence["granted_start_marker"], MARKER_A);
        assert_eq!(refusal.evidence["live_start_marker"], MARKER_B);
    }

    #[test]
    fn release_refuses_when_the_session_is_gone() {
        // The generation proof passes (pid 4242 still carries the granted
        // marker), but the named session no longer exists.
        let decision = decide_place_release(
            &tmux_demand(4242, MARKER_B),
            &pid_table(),
            &ProviderSnapshot::Tmux {
                session_exists: false,
                pane_pids: vec![],
            },
        );
        let PlaceReleaseDecision::Refuse(refusal) = decision else {
            panic!("expected a refusal, got {decision:?}");
        };
        assert_eq!(refusal.kind, REFUSAL_PLACE_GONE);
    }

    #[test]
    fn release_refuses_when_the_place_was_rebound_to_another_process() {
        // Pid 5000 is live with its granted marker, but the session's pane
        // processes no longer include it: the place moved on. Not our kill.
        let decision = decide_place_release(
            &tmux_demand(5000, MARKER_B),
            &pid_table(),
            &ProviderSnapshot::Tmux {
                session_exists: true,
                pane_pids: vec![4242],
            },
        );
        let PlaceReleaseDecision::Refuse(refusal) = decision else {
            panic!("expected a refusal, got {decision:?}");
        };
        assert_eq!(refusal.kind, REFUSAL_PLACE_MISMATCH);
        assert_eq!(refusal.evidence["pane_pids"], json!([4242]));
    }

    #[test]
    fn release_decides_to_release_only_on_full_proof() {
        // Same pid, same start marker, same place: the one path to Release.
        let decision = decide_place_release(
            &tmux_demand(4242, MARKER_B),
            &pid_table(),
            &ProviderSnapshot::Tmux {
                session_exists: true,
                pane_pids: vec![4242],
            },
        );
        let PlaceReleaseDecision::Release {
            provider,
            session_name,
            workspace_id,
        } = decision
        else {
            panic!("expected a release decision, got {decision:?}");
        };
        assert_eq!(provider, TMUX_PROVIDER);
        assert_eq!(session_name.as_deref(), Some("agent-test"));
        assert_eq!(workspace_id, None);
    }

    #[test]
    fn herdr_release_follows_the_same_generation_proof() {
        let demand = PlaceReleaseDemand {
            place_ref: herdr_place_ref("w9", "w9:p1"),
            pid: 5000,
            start_marker: MARKER_B.into(),
        };
        let refused = decide_place_release(
            &demand,
            &pid_table(),
            &ProviderSnapshot::Herdr {
                pane_pids: vec![4242],
            },
        );
        assert!(matches!(
            refused,
            PlaceReleaseDecision::Refuse(PlaceRefusal {
                kind: REFUSAL_PLACE_MISMATCH,
                ..
            })
        ));

        let decided = decide_place_release(
            &demand,
            &pid_table(),
            &ProviderSnapshot::Herdr {
                pane_pids: vec![5000],
            },
        );
        let PlaceReleaseDecision::Release {
            provider,
            workspace_id,
            ..
        } = decided
        else {
            panic!("expected a release decision, got {decided:?}");
        };
        assert_eq!(provider, HERDR_PROVIDER);
        assert_eq!(workspace_id.as_deref(), Some("w9"));
    }

    #[test]
    fn refusal_kinds_map_to_distinct_error_kinds() {
        // Error paths must explain: every refusal renders a JSON document and
        // a WorkcellError whose kind follows the repo's exit-code contract.
        let refusal = PlaceRefusal {
            kind: REFUSAL_ALREADY_EXISTS,
            provider: Some(TMUX_PROVIDER),
            message: "exists".into(),
            evidence: json!({ "session_name": "agent-test" }),
        };
        let value = refusal.to_json();
        assert_eq!(value["ok"], false);
        assert_eq!(value["refusal"]["kind"], REFUSAL_ALREADY_EXISTS);
        assert_eq!(value["refusal"]["evidence"]["session_name"], "agent-test");
        assert!(matches!(
            refusal.to_error(),
            WorkcellError::UnsatisfiedDemand(_)
        ));

        let stale = PlaceRefusal {
            kind: REFUSAL_STALE_BINDING,
            provider: None,
            message: "stale".into(),
            evidence: Value::Null,
        };
        assert!(matches!(
            stale.to_error(),
            WorkcellError::OperationFailed(_)
        ));

        let invalid = PlaceRefusal {
            kind: REFUSAL_INVALID_NAME,
            provider: None,
            message: "bad name".into(),
            evidence: Value::Null,
        };
        assert!(matches!(
            invalid.to_error(),
            WorkcellError::InvalidDemand(_)
        ));

        let absent = PlaceRefusal {
            kind: REFUSAL_NO_PROVIDER,
            provider: None,
            message: "nothing answered".into(),
            evidence: Value::Null,
        };
        assert!(matches!(absent.to_error(), WorkcellError::Unavailable(_)));
    }

    /// Live tmux round trip, only when `TMUX_TEST_LIVE` is set: request a
    /// named place, prove the grant's generation against the live pid table,
    /// then release it. Skipped silently on machines without the guard (and
    /// therefore on any machine without a tmux server running tests).
    #[test]
    fn live_tmux_request_and_release_round_trip() {
        let Ok(_) = std::env::var("TMUX_TEST_LIVE") else {
            return;
        };
        let name = format!("workcell-live-test-{}", std::process::id());
        let Some(grant) = request_tmux_place(&name).ok() else {
            // tmux present but no server reachable / creation refused: the
            // refusal path is the honest outcome; assert it is typed.
            let refusal = request_tmux_place(&name).unwrap_err();
            assert!(
                matches!(
                    refusal.kind,
                    REFUSAL_PROVIDER_ERROR | REFUSAL_NO_PROVIDER | REFUSAL_ALREADY_EXISTS
                ),
                "unexpected refusal kind {}",
                refusal.kind
            );
            return;
        };
        assert_eq!(grant.provider, TMUX_PROVIDER);
        assert!(grant.place_ref.starts_with("workcell:place:tmux:default:"));
        assert!(validate_place_name(&name).is_ok());

        let demand = PlaceReleaseDemand {
            place_ref: grant.place_ref.clone(),
            pid: grant.pane_pid,
            start_marker: grant.process_start_marker.clone(),
        };
        let result = release_place_live(&demand);
        assert!(result.is_ok(), "live release failed: {result:?}");
        let value = result.unwrap();
        assert_eq!(value["released"], true);
        assert_eq!(value["action"], "tmux kill-session");
    }
}
