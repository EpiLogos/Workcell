//! Workcell Run records (`workcell.run/v1`) and their durable ledger.
//!
//! One notion of an agent run across every rung: a durable record binding one
//! agent execution to the material world it ran in (demand → place → observe →
//! collect → release). Workcell owns the *material* record; Factory owns run
//! semantics and is carried as an optional `canonical_run_ref` — a run whose
//! canonical ref is null is a first-class Factory-less run, not a gap. The
//! `execution_status` vocabulary is Factory's closed `ExecutionStatus`
//! (`factory/src/build.rs`), reused verbatim: it is explicitly "shared with
//! every host that renders Factory executions", so Workcell admits exactly
//! those eight strings and invents none.
//!
//! The ledger is plain JSON under the state root (`runs/<slug>.json` plus a
//! `runs/runs.json` index), written through a staged file and renamed into
//! place, so records survive process restarts like every other durable
//! surface in this crate.

use std::{
    fs,
    path::{Path, PathBuf},
};

use epilogos_workcell_core::{Result, WorkcellError};
use serde_json::{json, Value};

pub const RUN_SCHEMA: &str = "workcell.run/v1";
pub const RUN_INDEX_SCHEMA: &str = "workcell.run-index/v1";
pub const RUNS_DIRECTORY: &str = "runs";
pub const RUNS_INDEX_FILE: &str = "runs.json";

/// Factory's closed execution-status vocabulary, reused verbatim
/// (`ExecutionStatus::ALL`, factory/src/build.rs). Workcell admits exactly
/// these eight strings.
pub const EXECUTION_STATUSES: [&str; 8] = [
    "queued",
    "running",
    "blocked",
    "returned",
    "success",
    "fail",
    "cancelled",
    "contract-fixture",
];

/// The rungs a run may execute on. Same record shape, statuses and deliverable
/// contract on every one.
pub const RUN_RUNGS: [&str; 3] = ["local", "remote", "sandbox"];

/// The prepared-run-scope contract (`workcell.prepared-run-scope/v1`): a
/// composition of receipts that already exist — the prepared write boundary,
/// the workspace allocation, the optional place grant — plus the run slug and
/// demand digest, so an encounter resident can be born *inside* the run.
pub const PREPARED_RUN_SCOPE_SCHEMA: &str = "workcell.prepared-run-scope/v1";

/// The legal `execution_status` transitions. `queued → running → returned →
/// success|fail|cancelled` is the spine; `blocked` is entered from anywhere
/// live and always names its reason; `returned` means "collect ran,
/// recognition pending". Terminal statuses stay put.
fn transition_allowed(from: &str, to: &str) -> bool {
    if from == to {
        return true;
    }
    match from {
        "queued" => matches!(to, "running" | "blocked" | "cancelled" | "fail"),
        "running" => matches!(
            to,
            "blocked" | "returned" | "success" | "fail" | "cancelled"
        ),
        "blocked" => matches!(
            to,
            "running" | "returned" | "success" | "fail" | "cancelled"
        ),
        "returned" => matches!(to, "blocked" | "success" | "fail" | "cancelled"),
        _ => false,
    }
}

pub fn now_unix_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_millis() as u64)
        .unwrap_or(0)
}

fn is_safe_slug(slug: &str) -> bool {
    !slug.is_empty()
        && slug.len() <= 128
        && slug
            .chars()
            .all(|character| character.is_ascii_alphanumeric() || matches!(character, '-' | '_'))
}

/// Validate the carrier shape of a run record. Vocabulary membership is
/// checked; the *truth* of a status is the caller's responsibility — the
/// ledger never invents or rewrites a reading.
pub fn validate_run_record(record: &Value) -> Result<()> {
    let object = record
        .as_object()
        .ok_or_else(|| WorkcellError::InvalidDemand("run record must be a JSON object".into()))?;
    if record["schema"] != RUN_SCHEMA {
        return Err(WorkcellError::InvalidDemand(format!(
            "run record schema must be {RUN_SCHEMA}"
        )));
    }
    if record["run_slug"]
        .as_str()
        .is_none_or(|slug| !is_safe_slug(slug))
    {
        return Err(WorkcellError::InvalidDemand(
            "run_slug must be 1-128 characters of [a-zA-Z0-9_-]".into(),
        ));
    }
    if record["demand_ref"]
        .as_str()
        .is_none_or(|value| value.trim().is_empty())
    {
        return Err(WorkcellError::InvalidDemand(
            "run record demand_ref must be a non-empty string".into(),
        ));
    }
    match &record["canonical_run_ref"] {
        Value::Null => {}
        Value::String(value) if !value.trim().is_empty() => {}
        _ => {
            return Err(WorkcellError::InvalidDemand(
                "canonical_run_ref must be a non-empty string or null; a null canonical ref is a first-class Factory-less run".into(),
            ))
        }
    }
    let status = record["execution_status"].as_str().ok_or_else(|| {
        WorkcellError::InvalidDemand("run record execution_status must be a string".into())
    })?;
    if !EXECUTION_STATUSES.contains(&status) {
        return Err(WorkcellError::InvalidDemand(format!(
            "execution_status `{status}` is outside the closed execution vocabulary ({})",
            EXECUTION_STATUSES.join("|")
        )));
    }
    let rung = record["rung"]
        .as_str()
        .ok_or_else(|| WorkcellError::InvalidDemand("run record rung must be a string".into()))?;
    if !RUN_RUNGS.contains(&rung) {
        return Err(WorkcellError::InvalidDemand(format!(
            "rung `{rung}` is outside the run vocabulary ({})",
            RUN_RUNGS.join("|")
        )));
    }
    if record["provider_ref"]
        .as_str()
        .is_none_or(|value| value.trim().is_empty())
    {
        return Err(WorkcellError::InvalidDemand(
            "run record provider_ref must be a non-empty string".into(),
        ));
    }
    for field in ["material_refs", "correlations"] {
        if !record[field].is_array() {
            return Err(WorkcellError::InvalidDemand(format!(
                "run record field `{field}` must be an array"
            )));
        }
    }
    for field in [
        "created_at_unix_ms",
        "opened_at_unix_ms",
        "collected_at_unix_ms",
        "closed_at_unix_ms",
    ] {
        match &record[field] {
            Value::Null => {}
            Value::Number(number) if number.is_u64() => {}
            _ => {
                return Err(WorkcellError::InvalidDemand(format!(
                    "run record field `{field}` must be a unix-ms number or null"
                )))
            }
        }
    }
    if !record["deliverable"].is_object() {
        return Err(WorkcellError::InvalidDemand(
            "run record deliverable must be an object".into(),
        ));
    }
    validate_agency_block(&record["agency"])?;
    validate_operative_block(&record["operative"])?;
    let _ = object;
    Ok(())
}

/// A-1: the optional `agency` block. Both `agency` and `operative` absent is a
/// first-class agency-less run, the same way a null `canonical_run_ref` is a
/// first-class Factory-less run.
pub fn validate_agency_block(agency: &Value) -> Result<()> {
    match agency {
        Value::Null => Ok(()),
        Value::Object(_) => {
            for field in [
                "agency_ref",
                "agency_rev",
                "source_ref",
                "source_digest",
                "binding_revision",
            ] {
                if agency[field]
                    .as_str()
                    .is_none_or(|value| value.trim().is_empty())
                {
                    return Err(WorkcellError::InvalidDemand(format!(
                        "agency block field `{field}` must be a non-empty string"
                    )));
                }
            }
            match &agency["minted_by"] {
                Value::Null => {}
                Value::String(value) if !value.trim().is_empty() => {}
                _ => {
                    return Err(WorkcellError::InvalidDemand(
                        "agency block field `minted_by` must be a non-empty string or null".into(),
                    ))
                }
            }
            Ok(())
        }
        _ => Err(WorkcellError::InvalidDemand(
            "agency block must be an object or null".into(),
        )),
    }
}

/// A-6: the optional `operative` block — thin declarations only. Every field
/// optional: an absent field means the suite default (harness = suite default
/// profile, model = roster winner), never a fabricated ref.
pub fn validate_operative_block(operative: &Value) -> Result<()> {
    match operative {
        Value::Null => Ok(()),
        Value::Object(object) => {
            for (field, value) in object {
                if !matches!(
                    field.as_str(),
                    "harness_ref" | "connection_ref" | "model_ref" | "agent_profile_ref"
                ) {
                    return Err(WorkcellError::InvalidDemand(format!(
                        "operative block field `{field}` is unknown; the block carries harness_ref, connection_ref, model_ref, agent_profile_ref"
                    )));
                }
                match value {
                    Value::Null => {}
                    Value::String(text) if !text.trim().is_empty() => {}
                    _ => {
                        return Err(WorkcellError::InvalidDemand(format!(
                            "operative block field `{field}` must be a non-empty string or null"
                        )))
                    }
                }
            }
            Ok(())
        }
        _ => Err(WorkcellError::InvalidDemand(
            "operative block must be an object or null".into(),
        )),
    }
}

/// Move a run's `execution_status` through the legal transition graph,
/// stamping the matching timestamp and an optional reason. A refusal names the
/// transition it refused; the record is never touched on refusal.
pub fn set_run_status(record: &mut Value, to: &str, reason: Option<&str>) -> Result<()> {
    if !EXECUTION_STATUSES.contains(&to) {
        return Err(WorkcellError::InvalidDemand(format!(
            "execution_status `{to}` is outside the closed execution vocabulary ({})",
            EXECUTION_STATUSES.join("|")
        )));
    }
    let from = record["execution_status"].as_str().unwrap_or("").to_owned();
    if !transition_allowed(&from, to) {
        return Err(WorkcellError::OperationFailed(format!(
            "run `{}` cannot transition execution_status from `{from}` to `{to}`",
            record["run_slug"].as_str().unwrap_or("?")
        )));
    }
    record["execution_status"] = json!(to);
    let now = now_unix_ms();
    match to {
        "running" if record["opened_at_unix_ms"].is_null() => {
            record["opened_at_unix_ms"] = json!(now);
        }
        "returned" => record["collected_at_unix_ms"] = json!(now),
        "success" | "fail" | "cancelled" if record["closed_at_unix_ms"].is_null() => {
            record["closed_at_unix_ms"] = json!(now);
        }
        _ => {}
    }
    record["status_reason"] = match reason {
        Some(reason) => json!(reason),
        None => Value::Null,
    };
    Ok(())
}

/// The durable run ledger: `<state-root>/runs/<slug>.json` records plus a
/// `runs/runs.json` index. Individual records are the source of truth; the
/// index is derived and rewritten on every write.
pub struct RunLedger {
    state_root: PathBuf,
}

impl RunLedger {
    pub fn new(state_root: impl Into<PathBuf>) -> Self {
        Self {
            state_root: state_root.into(),
        }
    }

    fn runs_dir(&self) -> PathBuf {
        self.state_root.join(RUNS_DIRECTORY)
    }

    fn record_path(&self, slug: &str) -> PathBuf {
        self.runs_dir().join(format!("{slug}.json"))
    }

    fn index_path(&self) -> PathBuf {
        self.runs_dir().join(RUNS_INDEX_FILE)
    }

    // All writers share the ledger lock, including the index projection.
    // A scope can compare its exact inspected record without racing a release.
    fn lock(&self) -> Result<fs::File> {
        fs::create_dir_all(self.runs_dir())
            .map_err(|e| WorkcellError::OperationFailed(e.to_string()))?;
        let file = fs::OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(self.runs_dir().join(".lock"))
            .map_err(|e| WorkcellError::OperationFailed(e.to_string()))?;
        file.lock()
            .map_err(|e| WorkcellError::OperationFailed(e.to_string()))?;
        Ok(file)
    }

    /// Write a new run record. A duplicate slug is refused — a run slug names
    /// one material execution, and history is never silently replaced.
    pub fn create(&self, mut record: Value) -> Result<Value> {
        let _lock = self.lock()?;
        if record["schema"] != RUN_SCHEMA {
            record["schema"] = json!(RUN_SCHEMA);
        }
        if record["created_at_unix_ms"].is_null() {
            record["created_at_unix_ms"] = json!(now_unix_ms());
        }
        for (field, value) in [
            ("opened_at_unix_ms", Value::Null),
            ("collected_at_unix_ms", Value::Null),
            ("closed_at_unix_ms", Value::Null),
            ("status_reason", Value::Null),
            ("canonical_run_ref", Value::Null),
            ("world_ref", Value::Null),
            ("world_receipt", Value::Null),
            ("demand_digest", Value::Null),
            ("place", Value::Null),
            ("boundary_digest", Value::Null),
            ("agency", Value::Null),
            ("operative", Value::Null),
        ] {
            if record.get(field).is_none() {
                record[field] = value;
            }
        }
        if record.get("deliverable").is_none() {
            record["deliverable"] = json!({"outputs": [], "branch": null});
        }
        for (field, value) in [("material_refs", json!([])), ("correlations", json!([]))] {
            if record.get(field).is_none() {
                record[field] = value;
            }
        }
        validate_run_record(&record)?;
        let slug = record["run_slug"]
            .as_str()
            .expect("validated slug")
            .to_owned();
        let path = self.record_path(&slug);
        if path.exists() {
            return Err(WorkcellError::OperationFailed(format!(
                "run `{slug}` already exists; a run slug names one material execution"
            )));
        }
        self.write_record(&path, &record)?;
        self.write_index()?;
        Ok(record)
    }

    /// Commit only if the owner record still equals the one inspected.
    pub fn update_if_unchanged(&self, previous: &Value, record: &Value) -> Result<()> {
        let _lock = self.lock()?;
        let slug = previous["run_slug"]
            .as_str()
            .ok_or_else(|| WorkcellError::InvalidDemand("Missing run slug".into()))?;
        if record["run_slug"] != previous["run_slug"] || self.get(slug)?.as_ref() != Some(previous)
        {
            return Err(WorkcellError::OperationFailed(
                "Run changed while the operation was returning; read the current run and retry"
                    .into(),
            ));
        }
        validate_run_record(record)?;
        let from = previous["execution_status"].as_str().unwrap_or("");
        let to = record["execution_status"].as_str().unwrap_or("");
        if !transition_allowed(from, to) {
            return Err(WorkcellError::InvalidDemand(format!(
                "run cannot transition from `{from}` to `{to}`"
            )));
        }
        self.update_locked(record)
    }

    fn update_locked(&self, record: &Value) -> Result<()> {
        validate_run_record(record)?;
        let slug = record["run_slug"].as_str().expect("validated slug");
        let path = self.record_path(slug);
        if !path.exists() {
            return Err(WorkcellError::NotFound(format!(
                "run `{slug}` is not in this ledger"
            )));
        }
        self.write_record(&path, record)?;
        self.write_index()?;
        Ok(())
    }

    pub fn get(&self, slug: &str) -> Result<Option<Value>> {
        if !is_safe_slug(slug) {
            return Err(WorkcellError::InvalidDemand(
                "run slug must be 1-128 characters of [a-zA-Z0-9_-]".into(),
            ));
        }
        let path = self.record_path(slug);
        match fs::read(&path) {
            Ok(bytes) => {
                let record: Value = serde_json::from_slice(&bytes).map_err(|error| {
                    WorkcellError::OperationFailed(format!(
                        "parse run record `{}`: {error}",
                        path.display()
                    ))
                })?;
                Ok(Some(record))
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(error) => Err(WorkcellError::OperationFailed(format!(
                "read run record `{}`: {error}",
                path.display()
            ))),
        }
    }

    /// Every record in the ledger, ordered by slug. The index is never read
    /// for listing — the records are the truth; the index only summarises.
    pub fn list(&self) -> Result<Vec<Value>> {
        let mut runs = Vec::new();
        let entries = match fs::read_dir(self.runs_dir()) {
            Ok(entries) => entries,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(runs),
            Err(error) => {
                return Err(WorkcellError::OperationFailed(format!(
                    "read runs directory `{}`: {error}",
                    self.runs_dir().display()
                )))
            }
        };
        for entry in entries {
            let entry = entry.map_err(|error| {
                WorkcellError::OperationFailed(format!("read runs directory entry: {error}"))
            })?;
            let path = entry.path();
            if path.extension().and_then(|value| value.to_str()) != Some("json")
                || path
                    .file_name()
                    .and_then(|value| value.to_str())
                    .is_some_and(|name| name == RUNS_INDEX_FILE || name.ends_with(".scope.json"))
            {
                continue;
            }
            let bytes = fs::read(&path).map_err(|error| {
                WorkcellError::OperationFailed(format!(
                    "read run record `{}`: {error}",
                    path.display()
                ))
            })?;
            let record: Value = serde_json::from_slice(&bytes).map_err(|error| {
                WorkcellError::OperationFailed(format!(
                    "parse run record `{}`: {error}",
                    path.display()
                ))
            })?;
            runs.push(record);
        }
        runs.sort_by_key(|record| record["run_slug"].as_str().unwrap_or("").to_owned());
        Ok(runs)
    }

    fn write_record(&self, path: &Path, record: &Value) -> Result<()> {
        fs::create_dir_all(self.runs_dir()).map_err(|error| {
            WorkcellError::OperationFailed(format!(
                "create runs directory `{}`: {error}",
                self.runs_dir().display()
            ))
        })?;
        let staged = path.with_extension("json.tmp");
        fs::write(
            &staged,
            serde_json::to_vec_pretty(record).map_err(|error| {
                WorkcellError::OperationFailed(format!("encode run record: {error}"))
            })?,
        )
        .map_err(|error| {
            WorkcellError::OperationFailed(format!(
                "write staged run record `{}`: {error}",
                staged.display()
            ))
        })?;
        fs::rename(&staged, path).map_err(|error| {
            WorkcellError::OperationFailed(format!(
                "commit run record `{}`: {error}",
                path.display()
            ))
        })
    }

    fn write_index(&self) -> Result<()> {
        let runs = self.list()?;
        let index = json!({
            "schema": RUN_INDEX_SCHEMA,
            "generated_at_unix_ms": now_unix_ms(),
            "runs": runs
                .iter()
                .map(|record| {
                    json!({
                        "run_slug": record["run_slug"],
                        "demand_ref": record["demand_ref"],
                        "canonical_run_ref": record["canonical_run_ref"],
                        "execution_status": record["execution_status"],
                        "rung": record["rung"],
                        "world_ref": record["world_ref"],
                        "created_at_unix_ms": record["created_at_unix_ms"],
                        "closed_at_unix_ms": record["closed_at_unix_ms"],
                    })
                })
                .collect::<Vec<_>>(),
        });
        let staged = self.index_path().with_extension("json.tmp");
        fs::create_dir_all(self.runs_dir()).map_err(|error| {
            WorkcellError::OperationFailed(format!("create runs directory: {error}"))
        })?;
        fs::write(
            &staged,
            serde_json::to_vec_pretty(&index).map_err(|error| {
                WorkcellError::OperationFailed(format!("encode run index: {error}"))
            })?,
        )
        .map_err(|error| {
            WorkcellError::OperationFailed(format!(
                "write staged run index `{}`: {error}",
                staged.display()
            ))
        })?;
        fs::rename(&staged, self.index_path()).map_err(|error| {
            WorkcellError::OperationFailed(format!(
                "commit run index `{}`: {error}",
                self.index_path().display()
            ))
        })
    }
}

/// Digest of the exact native run reading used to compose a scope.
pub fn run_record_revision(run: &Value) -> String {
    use sha2::{Digest, Sha256};
    format!(
        "sha256:{:x}",
        Sha256::digest(serde_json::to_vec(run).expect("JSON value"))
    )
}

/// Compose a `workcell.prepared-run-scope/v1` document from receipts that
/// already exist. `prepared_write_boundary` is the exact
/// `workcell.prepared-write-boundary/v1` object (digest included) when the
/// platform could prepare one, `null` with a named degradation when it could
/// not — never a weaker stand-in.
pub fn compose_prepared_run_scope(
    run: &Value,
    worktree_path: &str,
    workspace_material_ref: &str,
    prepared_write_boundary: Option<&Value>,
    place_grant: Option<&Value>,
) -> Value {
    json!({
        "schema": PREPARED_RUN_SCOPE_SCHEMA,
        "run_revision": run_record_revision(run),
        "run_slug": run["run_slug"],
        "worktree_path": worktree_path,
        "workspace_material_ref": workspace_material_ref,
        "prepared_write_boundary": prepared_write_boundary,
        "place_grant": place_grant,
        "demand_digest": run["demand_digest"],
        "agency": run["agency"],
        "operative": run["operative"],
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_root(label: &str) -> PathBuf {
        let nonce = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        std::env::temp_dir().join(format!(
            "epilogos-workcell-run-ledger-{label}-{}-{nonce}",
            std::process::id()
        ))
    }

    fn sample_record(slug: &str) -> Value {
        let mut record = json!({
            "schema": RUN_SCHEMA,
            "run_slug": slug,
            "demand_ref": "demand:run-my-run",
            "canonical_run_ref": null,
            "execution_status": "queued",
            "rung": "local",
            "provider_ref": "provider:collapsed-local-host-process",
        });
        // The defaults `create()` stamps for a minimal record.
        record["material_refs"] = json!([]);
        record["correlations"] = json!([]);
        record["deliverable"] = json!({"outputs": [], "branch": null});
        record
    }

    #[test]
    fn stale_scope_cannot_overwrite_a_later_run_transition() {
        let root = temp_root("scope-cas");
        let ledger = RunLedger::new(&root);
        let original = ledger.create(sample_record("scope-cas")).unwrap();
        let mut later = original.clone();
        set_run_status(&mut later, "blocked", Some("native boundary refused")).unwrap();
        ledger.update_if_unchanged(&original, &later).unwrap();
        let mut stale = original.clone();
        stale["boundary_digest"] = json!("sha256:scope");
        assert!(ledger.update_if_unchanged(&original, &stale).is_err());
        assert_eq!(ledger.get("scope-cas").unwrap().unwrap(), later);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn stale_observer_cannot_resurrect_a_concurrently_released_run() {
        let root = temp_root("observe-release-cas");
        let ledger = RunLedger::new(&root);
        let queued = ledger.create(sample_record("observe-release")).unwrap();
        let mut running = queued.clone();
        set_run_status(&mut running, "running", None).unwrap();
        ledger.update_if_unchanged(&queued, &running).unwrap();

        // The observer reads the real ledger before the release completes.
        // Its delayed return must never replace the newer terminal record.
        let (read_tx, read_rx) = std::sync::mpsc::channel();
        let (release_tx, release_rx) = std::sync::mpsc::channel();
        let observer_root = root.clone();
        let observer = std::thread::spawn(move || {
            let ledger = RunLedger::new(observer_root);
            let original = ledger.get("observe-release").unwrap().unwrap();
            read_tx.send(()).unwrap();
            release_rx.recv().unwrap();
            let mut observed = original.clone();
            set_run_status(&mut observed, "blocked", Some("late observation")).unwrap();
            ledger.update_if_unchanged(&original, &observed)
        });
        read_rx.recv().unwrap();
        let mut released = running.clone();
        set_run_status(&mut released, "success", None).unwrap();
        released["release_disposition"] = json!("released");
        ledger.update_if_unchanged(&running, &released).unwrap();
        release_tx.send(()).unwrap();
        let error = observer.join().unwrap().unwrap_err();
        assert!(error.to_string().contains("Run changed"));
        assert_eq!(ledger.get("observe-release").unwrap().unwrap(), released);
        assert_eq!(ledger.list().unwrap(), vec![released.clone()]);

        // Even a caller bypassing set_run_status cannot reverse a terminal
        // transition through the sole durable update method.
        let mut invalid = released.clone();
        invalid["execution_status"] = json!("running");
        assert!(ledger.update_if_unchanged(&released, &invalid).is_err());
        assert_eq!(ledger.get("observe-release").unwrap().unwrap(), released);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn concurrent_run_writers_commit_only_one_exact_basis() {
        let root = temp_root("simultaneous-cas");
        let ledger = RunLedger::new(&root);
        let queued = ledger.create(sample_record("simultaneous-cas")).unwrap();
        let mut running = queued.clone();
        set_run_status(&mut running, "running", None).unwrap();
        ledger.update_if_unchanged(&queued, &running).unwrap();
        let barrier = std::sync::Arc::new(std::sync::Barrier::new(3));
        let writers: Vec<_> = ["first-return", "second-return"]
            .into_iter()
            .map(|label| {
                let root = root.clone();
                let barrier = barrier.clone();
                std::thread::spawn(move || {
                    let ledger = RunLedger::new(root);
                    let original = ledger.get("simultaneous-cas").unwrap().unwrap();
                    let mut changed = original.clone();
                    set_run_status(&mut changed, "blocked", Some(label)).unwrap();
                    barrier.wait();
                    (ledger.update_if_unchanged(&original, &changed), changed)
                })
            })
            .collect();
        barrier.wait();
        let results: Vec<_> = writers
            .into_iter()
            .map(|writer| writer.join().unwrap())
            .collect();
        assert_eq!(
            results.iter().filter(|(result, _)| result.is_ok()).count(),
            1
        );
        assert_eq!(
            results.iter().filter(|(result, _)| result.is_err()).count(),
            1
        );
        let winner = &results.iter().find(|(result, _)| result.is_ok()).unwrap().1;
        assert_eq!(
            ledger.get("simultaneous-cas").unwrap().as_ref(),
            Some(winner)
        );
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn the_execution_vocabulary_is_factorys_closed_set_reused_verbatim() {
        assert_eq!(
            EXECUTION_STATUSES,
            [
                "queued",
                "running",
                "blocked",
                "returned",
                "success",
                "fail",
                "cancelled",
                "contract-fixture"
            ]
        );
    }

    #[test]
    fn a_record_outside_the_closed_vocabulary_is_refused() {
        let mut record = sample_record("outside-vocab");
        record["execution_status"] = json!("almost-done");
        let error = validate_run_record(&record).unwrap_err();
        assert!(error.to_string().contains("closed execution vocabulary"));
    }

    #[test]
    fn a_factory_less_run_is_first_class_and_a_named_canonical_ref_is_carried() {
        let ledger = RunLedger::new(temp_root("factory-less"));
        let record = ledger.create(sample_record("factoryless-1")).unwrap();
        assert!(record["canonical_run_ref"].is_null());

        let mut carried = sample_record("factory-1");
        carried["canonical_run_ref"] = json!("run:my-project/42");
        let carried = ledger.create(carried).unwrap();
        assert_eq!(carried["canonical_run_ref"], json!("run:my-project/42"));

        let mut empty = sample_record("factory-2");
        empty["canonical_run_ref"] = json!("");
        assert!(validate_run_record(&empty).is_err());
    }

    #[test]
    fn status_transitions_follow_the_phase_mapping_and_stamp_timestamps() {
        let mut record = sample_record("transitions");
        set_run_status(&mut record, "running", None).unwrap();
        assert!(!record["opened_at_unix_ms"].is_null());
        set_run_status(&mut record, "returned", None).unwrap();
        assert!(!record["collected_at_unix_ms"].is_null());
        set_run_status(&mut record, "success", None).unwrap();
        assert!(!record["closed_at_unix_ms"].is_null());

        // Terminal statuses are terminal.
        let error = set_run_status(&mut record, "running", None).unwrap_err();
        assert!(error.to_string().contains("cannot transition"));

        // Blocked always names its reason and can be entered while live.
        let mut blocked = sample_record("blocked");
        set_run_status(&mut blocked, "running", None).unwrap();
        set_run_status(
            &mut blocked,
            "blocked",
            Some("release refused: worktree dirty"),
        )
        .unwrap();
        assert_eq!(
            blocked["status_reason"],
            json!("release refused: worktree dirty")
        );
        set_run_status(&mut blocked, "success", None).unwrap();
    }

    #[test]
    fn records_survive_a_reconstructed_ledger_and_the_index_is_derived() {
        let root = temp_root("restart");
        let ledger = RunLedger::new(&root);
        ledger.create(sample_record("durable-1")).unwrap();
        let mut second = sample_record("durable-2");
        second["rung"] = json!("remote");
        ledger.create(second).unwrap();

        // A new process constructs a new ledger over the same state root.
        let restarted = RunLedger::new(&root);
        let listed = restarted.list().unwrap();
        assert_eq!(listed.len(), 2);
        assert_eq!(listed[0]["run_slug"], json!("durable-1"));
        let fetched = restarted.get("durable-2").unwrap().unwrap();
        assert_eq!(fetched["rung"], json!("remote"));

        let index: Value = serde_json::from_str(
            &fs::read_to_string(root.join(RUNS_DIRECTORY).join(RUNS_INDEX_FILE)).unwrap(),
        )
        .unwrap();
        assert_eq!(index["schema"], json!(RUN_INDEX_SCHEMA));
        assert_eq!(index["runs"].as_array().unwrap().len(), 2);

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn a_duplicate_slug_is_refused_and_history_is_never_replaced() {
        let root = temp_root("duplicate");
        let ledger = RunLedger::new(&root);
        ledger.create(sample_record("twice")).unwrap();
        let error = ledger.create(sample_record("twice")).unwrap_err();
        assert!(error.to_string().contains("already exists"));
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn agency_and_operative_blocks_validate_as_optional_shaped_objects() {
        let mut record = sample_record("a1-a6");
        record["agency"] = json!({
            "agency_ref": "agency:aikit-mint-x",
            "agency_rev": "rev/agency-1",
            "source_ref": "agency-mint:abc123",
            "source_digest": "blake3:deadbeef",
            "binding_revision": "binding/01",
            "minted_by": "owner:aikit session-space agency-mint"
        });
        record["operative"] = json!({
            "harness_ref": "harness-profile/pi",
            "model_ref": null,
        });
        validate_run_record(&record).unwrap();

        record["operative"]["model_ref"] = json!("");
        assert!(validate_run_record(&record).is_err());

        let mut bad_agency = sample_record("a1-bad");
        bad_agency["agency"] = json!({"agency_ref": "agency:x"});
        assert!(validate_run_record(&bad_agency).is_err());

        let mut unknown_operative = sample_record("a6-bad");
        unknown_operative["operative"] = json!({"favourite_colour": "blue"});
        assert!(validate_run_record(&unknown_operative).is_err());

        // Absent blocks are the suite-default agency-less run.
        validate_run_record(&sample_record("a1-absent")).unwrap();
    }

    #[test]
    fn the_scope_composition_carries_the_run_identity_and_receipts() {
        let mut run = sample_record("scoped");
        run["demand_digest"] = json!("sha256:abc");
        run["agency"] = json!({
            "agency_ref": "agency:aikit-mint-x",
            "agency_rev": "rev/agency-1",
            "source_ref": "agency-mint:abc123",
            "source_digest": "blake3:deadbeef",
            "binding_revision": "binding/01",
            "minted_by": Value::Null,
        });
        let boundary = json!({"schema": "workcell.prepared-write-boundary/v1", "requirements_digest": "sha256:f00d"});
        let scope = compose_prepared_run_scope(
            &run,
            "/tmp/worktree-path",
            "workspace:git-worktree:abc",
            Some(&boundary),
            None,
        );
        assert_eq!(scope["schema"], json!(PREPARED_RUN_SCOPE_SCHEMA));
        assert_eq!(scope["run_slug"], json!("scoped"));
        assert_eq!(scope["worktree_path"], json!("/tmp/worktree-path"));
        assert_eq!(
            scope["workspace_material_ref"],
            json!("workspace:git-worktree:abc")
        );
        assert_eq!(scope["prepared_write_boundary"], boundary);
        assert!(scope["place_grant"].is_null());
        assert_eq!(scope["demand_digest"], json!("sha256:abc"));
        assert_eq!(scope["agency"]["agency_ref"], json!("agency:aikit-mint-x"));
        assert_eq!(scope["operative"], Value::Null);
    }
}
