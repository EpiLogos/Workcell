//! Server-side sandbox reconciliation against a live OpenSandbox lifecycle
//! server: list material, classify orphans, and release them.
//!
//! Orphan rule, kept conservative on purpose: a sandbox whose lease expiry is
//! present, parseable, and in the past is an orphan and may be released. A
//! sandbox without lease evidence is `Unknown`, never an orphan — releasing
//! without evidence would be a guess, and guesses destroy live material.
//! Snapshots carry no lease at all; they are only touched when the caller
//! explicitly names the snapshot scope, and every deletion is reported.
//! Failures are named per target, never aggregated away.

use std::collections::BTreeMap;

use epilogos_workcell_core::{Result, WorkcellError};
use serde_json::Value;
use time::{format_description::well_known::Rfc3339, OffsetDateTime};

use super::client::{join_url, require_success_safe};
use super::protocol::{
    OpenSandboxHttpRequest, OpenSandboxHttpResponse, OpenSandboxTransport,
    OPENSANDBOX_API_KEY_HEADER,
};

const LIST_PAGE_SIZE: usize = 100;
const PAGE_SAFETY_VALVE: u32 = 100;

/// Lease classification for one server-side sandbox.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum SandboxLease {
    /// Lease expiry is in the future; the sandbox is spoken for.
    Live,
    /// Lease expiry is in the past; the sandbox is an orphan and releasable.
    Expired,
    /// No parseable lease evidence; never released by a reconcile pass.
    Unknown,
}

impl SandboxLease {
    pub fn as_str(&self) -> &'static str {
        match self {
            SandboxLease::Live => "live",
            SandboxLease::Expired => "expired-orphan",
            SandboxLease::Unknown => "unknown",
        }
    }
}

/// One sandbox as listed by the lifecycle server.
#[derive(Clone, Debug)]
pub struct SandboxSummary {
    pub id: String,
    pub state: String,
    pub expires_at: Option<String>,
    pub lease: SandboxLease,
}

/// One snapshot as listed by the lifecycle server.
#[derive(Clone, Debug)]
pub struct SnapshotSummary {
    pub id: String,
    pub state: String,
    pub created_at: Option<String>,
}

/// A named failure against one target; reconcile reports every one.
#[derive(Clone, Debug)]
pub struct ReconcileFailure {
    pub operation: String,
    pub target: String,
    pub reason: String,
}

/// Full result of one reconcile pass.
#[derive(Clone, Debug, Default)]
pub struct ReconcileReport {
    pub sandboxes: Vec<SandboxSummary>,
    pub snapshots: Vec<SnapshotSummary>,
    /// Entries the server returned that did not carry a recognisable identity.
    pub unrecognised_sandboxes: usize,
    pub unrecognised_snapshots: usize,
    /// Orphan sandboxes released (or already absent) this pass.
    pub released_sandboxes: Vec<String>,
    /// Snapshots deleted this pass (only when the snapshot scope was named).
    pub deleted_snapshots: Vec<String>,
    pub failures: Vec<ReconcileFailure>,
}

impl ReconcileReport {
    pub fn orphan_sandboxes(&self) -> Vec<&SandboxSummary> {
        self.sandboxes
            .iter()
            .filter(|sandbox| sandbox.lease == SandboxLease::Expired)
            .collect()
    }
}

/// Lightweight server-side reconciler. Unlike the execution provider it needs
/// no world, demand, or startup material — only the lifecycle base URL, an
/// optional API key, and a transport.
pub struct SandboxServerReconciler<T> {
    lifecycle_base_url: String,
    api_key: Option<String>,
    transport: T,
}

impl<T> SandboxServerReconciler<T>
where
    T: OpenSandboxTransport,
{
    pub fn new(
        lifecycle_base_url: impl Into<String>,
        api_key: Option<String>,
        transport: T,
    ) -> Self {
        Self {
            lifecycle_base_url: lifecycle_base_url.into(),
            api_key,
            transport,
        }
    }

    fn request(&self, method: &str, path: &str) -> Result<OpenSandboxHttpResponse> {
        let mut headers = BTreeMap::new();
        if let Some(key) = &self.api_key {
            headers.insert(OPENSANDBOX_API_KEY_HEADER.into(), key.clone());
        }
        self.transport.request(OpenSandboxHttpRequest {
            method: method.into(),
            url: join_url(&self.lifecycle_base_url, path),
            headers,
            body: Vec::new(),
        })
    }

    fn json(&self, method: &str, path: &str, operation: &str) -> Result<Value> {
        let response = self.request(method, path)?;
        require_success_safe(&response, operation)?;
        if response.body.is_empty() {
            return Ok(Value::Null);
        }
        serde_json::from_slice(&response.body).map_err(|error| {
            WorkcellError::OperationFailed(format!(
                "decode OpenSandbox `{operation}` response: {error}"
            ))
        })
    }

    fn list_sandboxes(&self) -> Result<(Vec<SandboxSummary>, usize)> {
        let mut summaries = Vec::new();
        let mut unrecognised = 0usize;
        for page in 1..=PAGE_SAFETY_VALVE {
            let value = self.json(
                "GET",
                &format!("/sandboxes?page={page}&pageSize={LIST_PAGE_SIZE}"),
                "list sandboxes",
            )?;
            let items = collection_items(&value);
            let count = items.len();
            for item in items {
                match parse_sandbox(&item, OffsetDateTime::now_utc()) {
                    Some(summary) => summaries.push(summary),
                    None => unrecognised += 1,
                }
            }
            if count < LIST_PAGE_SIZE {
                break;
            }
        }
        Ok((summaries, unrecognised))
    }

    fn list_snapshots(&self) -> Result<(Vec<SnapshotSummary>, usize)> {
        let mut summaries = Vec::new();
        let mut unrecognised = 0usize;
        for page in 1..=PAGE_SAFETY_VALVE {
            let value = self.json(
                "GET",
                &format!("/snapshots?page={page}&pageSize={LIST_PAGE_SIZE}"),
                "list snapshots",
            )?;
            let items = collection_items(&value);
            let count = items.len();
            for item in items {
                match parse_snapshot(&item) {
                    Some(summary) => summaries.push(summary),
                    None => unrecognised += 1,
                }
            }
            if count < LIST_PAGE_SIZE {
                break;
            }
        }
        Ok((summaries, unrecognised))
    }

    /// One reconcile pass. `release_orphans` deletes expired-lease sandboxes;
    /// `include_snapshots` additionally lists — and, when releasing, deletes —
    /// server-side snapshots. With both flags false this is a pure report.
    pub fn reconcile(
        &self,
        release_orphans: bool,
        include_snapshots: bool,
    ) -> Result<ReconcileReport> {
        let mut report = ReconcileReport::default();
        let (sandboxes, unrecognised) = self.list_sandboxes()?;
        report.sandboxes = sandboxes;
        report.unrecognised_sandboxes = unrecognised;

        if release_orphans {
            let orphans: Vec<String> = report
                .orphan_sandboxes()
                .iter()
                .map(|sandbox| sandbox.id.clone())
                .collect();
            for id in orphans {
                match self.release_sandbox(&id) {
                    Ok(()) => report.released_sandboxes.push(id),
                    Err(error) => report.failures.push(ReconcileFailure {
                        operation: "release sandbox".into(),
                        target: id,
                        reason: error.to_string(),
                    }),
                }
            }
        }

        if include_snapshots {
            let (snapshots, unrecognised) = self.list_snapshots()?;
            report.snapshots = snapshots;
            report.unrecognised_snapshots = unrecognised;
            if release_orphans {
                let ids: Vec<String> = report
                    .snapshots
                    .iter()
                    .map(|snapshot| snapshot.id.clone())
                    .collect();
                for id in ids {
                    match self.delete_snapshot(&id) {
                        Ok(()) => report.deleted_snapshots.push(id),
                        Err(error) => report.failures.push(ReconcileFailure {
                            operation: "delete snapshot".into(),
                            target: id,
                            reason: error.to_string(),
                        }),
                    }
                }
            }
        }

        Ok(report)
    }

    /// Release exactly the named sandbox ids, regardless of lease evidence.
    /// This is the operator-asserted escape hatch for material that is known
    /// orphaned but whose lease still claims live (for example a panicked
    /// client that renewed before dying). Returns (released, failures); a 404
    /// counts as released because the material is already absent.
    pub fn release_asserted(&self, ids: &[String]) -> (Vec<String>, Vec<ReconcileFailure>) {
        let mut released = Vec::new();
        let mut failures = Vec::new();
        for id in ids {
            match self.release_sandbox(id) {
                Ok(()) => released.push(id.clone()),
                Err(error) => failures.push(ReconcileFailure {
                    operation: "release sandbox (operator-asserted)".into(),
                    target: id.clone(),
                    reason: error.to_string(),
                }),
            }
        }
        (released, failures)
    }

    /// DELETE one sandbox. HTTP 404 counts as success: the material is already
    /// absent, which is the desired end state.
    fn release_sandbox(&self, id: &str) -> Result<()> {
        let response = self.request("DELETE", &format!("/sandboxes/{id}"))?;
        if response.status == 404 {
            return Ok(());
        }
        require_success_safe(&response, "release sandbox")
    }

    /// DELETE one snapshot, with the same idempotent-404 rule as sandboxes.
    fn delete_snapshot(&self, id: &str) -> Result<()> {
        let response = self.request("DELETE", &format!("/snapshots/{id}"))?;
        if response.status == 404 {
            return Ok(());
        }
        require_success_safe(&response, "delete snapshot")
    }
}

/// Accept the response shapes upstream paginators use: a bare array, or an
/// object carrying the collection under any of the common keys.
fn collection_items(value: &Value) -> Vec<Value> {
    let array = match value {
        Value::Array(items) => items.clone(),
        Value::Object(object) => object
            .get("items")
            .or_else(|| object.get("sandboxes"))
            .or_else(|| object.get("snapshots"))
            .or_else(|| object.get("data"))
            .or_else(|| object.get("results"))
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default(),
        _ => Vec::new(),
    };
    array
}

fn first_string(object: &serde_json::Map<String, Value>, keys: &[&str]) -> Option<String> {
    keys.iter()
        .find_map(|key| object.get(*key).and_then(Value::as_str))
        .map(str::to_owned)
}

fn parse_sandbox(value: &Value, now: OffsetDateTime) -> Option<SandboxSummary> {
    let object = value.as_object()?;
    let id = first_string(object, &["id", "sandbox_id"])?;
    if id.trim().is_empty() {
        return None;
    }
    let state = first_string(object, &["state", "status"]).unwrap_or_else(|| "unknown".into());
    let expires_at = first_string(object, &["expiresAt", "expires_at", "lease_expires_at"]);
    let lease = match expires_at.as_deref() {
        Some(raw) => match OffsetDateTime::parse(raw, &Rfc3339) {
            Ok(expiry) if expiry <= now => SandboxLease::Expired,
            Ok(_) => SandboxLease::Live,
            Err(_) => SandboxLease::Unknown,
        },
        None => SandboxLease::Unknown,
    };
    Some(SandboxSummary {
        id,
        state,
        expires_at,
        lease,
    })
}

fn parse_snapshot(value: &Value) -> Option<SnapshotSummary> {
    let object = value.as_object()?;
    let id = first_string(object, &["id", "snapshot_id"])?;
    if id.trim().is_empty() {
        return None;
    }
    let state = first_string(object, &["state", "status"]).unwrap_or_else(|| "unknown".into());
    let created_at = first_string(object, &["createdAt", "created_at"]);
    Some(SnapshotSummary {
        id,
        state,
        created_at,
    })
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Mutex};

    use super::*;
    use epilogos_workcell_core::WorkcellError;

    #[derive(Clone, Default)]
    struct FixtureTransport {
        requests: Arc<Mutex<Vec<OpenSandboxHttpRequest>>>,
        responses: Arc<Mutex<Vec<OpenSandboxHttpResponse>>>,
    }

    impl FixtureTransport {
        fn with_responses(responses: Vec<OpenSandboxHttpResponse>) -> Self {
            Self {
                requests: Arc::new(Mutex::new(Vec::new())),
                responses: Arc::new(Mutex::new(responses.into_iter().rev().collect())),
            }
        }

        fn requests(&self) -> Vec<OpenSandboxHttpRequest> {
            self.requests.lock().unwrap().clone()
        }
    }

    impl OpenSandboxTransport for FixtureTransport {
        fn request(&self, request: OpenSandboxHttpRequest) -> Result<OpenSandboxHttpResponse> {
            self.requests.lock().unwrap().push(request);
            self.responses
                .lock()
                .unwrap()
                .pop()
                .ok_or_else(|| WorkcellError::OperationFailed("fixture response exhausted".into()))
        }
    }

    fn response(status: u16, value: Value) -> OpenSandboxHttpResponse {
        OpenSandboxHttpResponse {
            status,
            headers: BTreeMap::new(),
            body: if value.is_null() {
                Vec::new()
            } else {
                serde_json::to_vec(&value).unwrap()
            },
        }
    }

    fn reconciler(transport: FixtureTransport) -> SandboxServerReconciler<FixtureTransport> {
        SandboxServerReconciler::new("http://127.0.0.1:8080", Some("test-key".into()), transport)
    }

    fn paged_sandboxes() -> Value {
        serde_json::json!({
            "items": [
                {"id": "expired-one", "state": "Running",
                 "expiresAt": "2026-09-07T20:58:25.816410Z"},
                {"id": "live-one", "state": "Running",
                 "expiresAt": "2999-01-01T00:00:00Z"},
                {"id": "no-lease", "state": "Running"},
                {"id": "bad-lease", "state": "Running", "expiresAt": "not-a-date"},
            ]
        })
    }

    #[test]
    fn report_classifies_orphans_without_releasing_by_default() {
        let transport = FixtureTransport::with_responses(vec![response(200, paged_sandboxes())]);
        let reconciler = reconciler(transport.clone());

        let report = reconciler.reconcile(false, false).unwrap();

        let leases: BTreeMap<String, SandboxLease> = report
            .sandboxes
            .iter()
            .map(|sandbox| (sandbox.id.clone(), sandbox.lease.clone()))
            .collect();
        assert_eq!(leases["expired-one"], SandboxLease::Expired);
        assert_eq!(leases["live-one"], SandboxLease::Live);
        assert_eq!(leases["no-lease"], SandboxLease::Unknown);
        assert_eq!(leases["bad-lease"], SandboxLease::Unknown);
        assert_eq!(report.orphan_sandboxes().len(), 1);
        assert!(report.released_sandboxes.is_empty());
        assert!(report.failures.is_empty());
        // Only the list call was made.
        assert_eq!(transport.requests().len(), 1);
    }

    #[test]
    fn release_orphans_deletes_only_expired() {
        let transport = FixtureTransport::with_responses(vec![
            response(200, paged_sandboxes()),
            response(204, Value::Null),
        ]);
        let reconciler = reconciler(transport.clone());

        let report = reconciler.reconcile(true, false).unwrap();

        assert_eq!(report.released_sandboxes, vec!["expired-one"]);
        assert!(report.failures.is_empty());
        let requests = transport.requests();
        assert_eq!(requests.len(), 2);
        assert_eq!(requests[1].method, "DELETE");
        assert!(requests[1].url.ends_with("/sandboxes/expired-one"));
    }

    #[test]
    fn release_treats_absent_sandbox_as_released() {
        let transport = FixtureTransport::with_responses(vec![
            response(200, paged_sandboxes()),
            response(404, Value::Null),
        ]);
        let reconciler = reconciler(transport);

        let report = reconciler.reconcile(true, false).unwrap();

        assert_eq!(report.released_sandboxes, vec!["expired-one"]);
        assert!(report.failures.is_empty());
    }

    #[test]
    fn release_names_failures_without_stopping() {
        let transport = FixtureTransport::with_responses(vec![
            response(200, paged_sandboxes()),
            response(500, Value::Null),
        ]);
        let reconciler = reconciler(transport);

        let report = reconciler.reconcile(true, false).unwrap();

        assert!(report.released_sandboxes.is_empty());
        assert_eq!(report.failures.len(), 1);
        assert_eq!(report.failures[0].target, "expired-one");
        assert_eq!(report.failures[0].operation, "release sandbox");
    }

    #[test]
    fn snapshots_only_touched_when_scope_named() {
        let transport = FixtureTransport::with_responses(vec![response(200, paged_sandboxes())]);
        let reconciler = reconciler(transport);

        let report = reconciler.reconcile(false, false).unwrap();
        assert!(report.snapshots.is_empty());
    }

    #[test]
    fn include_snapshots_lists_and_with_release_deletes() {
        let transport = FixtureTransport::with_responses(vec![
            response(200, paged_sandboxes()),
            response(204, Value::Null), // orphan release
            response(
                200,
                serde_json::json!({"items": [
                    {"id": "snap-one", "state": "Ready",
                     "createdAt": "2026-09-07T21:00:00Z"},
                ]}),
            ),
            response(204, Value::Null), // snapshot delete
        ]);
        let reconciler = reconciler(transport.clone());

        let report = reconciler.reconcile(true, true).unwrap();

        assert_eq!(report.snapshots.len(), 1);
        assert_eq!(report.deleted_snapshots, vec!["snap-one"]);
        let requests = transport.requests();
        assert_eq!(requests.len(), 4);
        assert_eq!(requests[1].method, "DELETE");
        assert!(requests[1].url.ends_with("/sandboxes/expired-one"));
        assert_eq!(requests[3].method, "DELETE");
        assert!(requests[3].url.ends_with("/snapshots/snap-one"));
    }

    #[test]
    fn release_asserted_releases_named_ids_regardless_of_lease() {
        let transport = FixtureTransport::with_responses(vec![
            response(200, paged_sandboxes()),
            response(204, Value::Null), // live-one: lease says live, operator asserts orphan
            response(404, Value::Null), // missing: already absent
        ]);
        let reconciler = reconciler(transport.clone());

        let report = reconciler.reconcile(false, false).unwrap();
        let (released, failures) =
            reconciler.release_asserted(&["live-one".into(), "missing".into()]);

        assert_eq!(released, vec!["live-one", "missing"]);
        assert!(failures.is_empty());
        // The lease rule is untouched: expired-one is still a lease-orphan,
        // and the operator-asserted path did not act on it.
        assert_eq!(
            report
                .orphan_sandboxes()
                .iter()
                .map(|s| s.id.as_str())
                .collect::<Vec<_>>(),
            vec!["expired-one"]
        );
        let requests = transport.requests();
        assert_eq!(requests.len(), 3);
        assert!(requests[1].url.ends_with("/sandboxes/live-one"));
        assert!(requests[2].url.ends_with("/sandboxes/missing"));
    }

    #[test]
    fn release_asserted_names_failures() {
        let transport = FixtureTransport::with_responses(vec![response(500, Value::Null)]);
        let reconciler = reconciler(transport);

        let (released, failures) = reconciler.release_asserted(&["stuck".into()]);

        assert!(released.is_empty());
        assert_eq!(failures.len(), 1);
        assert_eq!(failures[0].operation, "release sandbox (operator-asserted)");
        assert_eq!(failures[0].target, "stuck");
    }

    #[test]
    fn bare_array_collection_shape_is_accepted() {
        let transport = FixtureTransport::with_responses(vec![response(
            200,
            serde_json::json!([
                {"id": "array-one", "state": "Running",
                 "expiresAt": "2026-09-07T20:58:25Z"},
            ]),
        )]);
        let reconciler = reconciler(transport);

        let report = reconciler.reconcile(false, false).unwrap();

        assert_eq!(report.sandboxes.len(), 1);
        assert_eq!(report.sandboxes[0].id, "array-one");
        assert_eq!(report.sandboxes[0].lease, SandboxLease::Expired);
    }

    #[test]
    fn unrecognised_entries_are_counted_not_fatal() {
        let transport = FixtureTransport::with_responses(vec![response(
            200,
            serde_json::json!({"items": [
                {"id": "ok", "state": "Running", "expiresAt": "2999-01-01T00:00:00Z"},
                {"state": "Running"},
                {"id": ""},
            ]}),
        )]);
        let reconciler = reconciler(transport);

        let report = reconciler.reconcile(false, false).unwrap();

        assert_eq!(report.sandboxes.len(), 1);
        assert_eq!(report.unrecognised_sandboxes, 2);
    }

    #[test]
    fn api_key_travels_in_the_wire_header() {
        let transport = FixtureTransport::with_responses(vec![response(200, paged_sandboxes())]);
        let reconciler = reconciler(transport.clone());

        reconciler.reconcile(false, false).unwrap();

        let requests = transport.requests();
        assert_eq!(
            requests[0]
                .headers
                .get(OPENSANDBOX_API_KEY_HEADER)
                .map(String::as_str),
            Some("test-key")
        );
    }

    #[test]
    fn empty_page_stops_pagination() {
        let transport =
            FixtureTransport::with_responses(vec![response(200, serde_json::json!({"items": []}))]);
        let reconciler = reconciler(transport.clone());

        let report = reconciler.reconcile(false, false).unwrap();

        assert!(report.sandboxes.is_empty());
        assert_eq!(transport.requests().len(), 1);
    }

    #[test]
    fn full_page_fetches_the_next_page() {
        let many: Value = serde_json::json!({
            "items": (0..LIST_PAGE_SIZE)
                .map(|index| serde_json::json!({
                    "id": format!("sandbox-{index}"),
                    "state": "Running",
                    "expiresAt": "2999-01-01T00:00:00Z",
                }))
                .collect::<Vec<_>>()
        });
        let transport = FixtureTransport::with_responses(vec![
            response(200, many),
            response(200, serde_json::json!({"items": []})),
        ]);
        let reconciler = reconciler(transport.clone());

        let report = reconciler.reconcile(false, false).unwrap();

        assert_eq!(report.sandboxes.len(), LIST_PAGE_SIZE);
        let requests = transport.requests();
        assert_eq!(requests.len(), 2);
        assert!(requests[0].url.contains("page=1"));
        assert!(requests[1].url.contains("page=2"));
    }
}
