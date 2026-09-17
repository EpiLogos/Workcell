//! Origin-side secret projection ledger.
//!
//! One origin, projection not replication: when a credential stored in this
//! cell's secret source is authorised for use on another cell or a sandbox,
//! the origin holds the `SecretProjectionRequest` — source ref, target
//! relation, the one authorised materialisation class, purpose and scope.
//! The ledger is the origin's durable memory of those grants; it stores
//! refs and classes only, never material.
//!
//! The store is one JSON file under the workcell state root
//! (`<state-root>/secrets/projections.json`), following the same law as the
//! connection-grants registry: a missing file is an empty ledger, an
//! unreadable or invalid file is named unavailability, and the file is
//! bound to one `workcell_ref`. Revocation marks the record and keeps it;
//! every decision is computed fresh from the file so a revocation reaches
//! the projected target at its next use — including from another process.

use std::{
    collections::BTreeMap,
    fs,
    path::{Path, PathBuf},
};

use epilogos_workcell_core::{
    Result, SecretMaterialisationClass, SecretProjectionRequest, SecretProjectionTarget,
    WorkcellError, WorkcellRef,
};
use serde_json::{json, Value};

pub const SECRETS_DIRECTORY: &str = "secrets";
pub const PROJECTIONS_FILE: &str = "projections.json";
pub const PROJECTIONS_SCHEMA: &str = "workcell.secret-projections/v1";

pub const PROJECTION_STATE_ACTIVE: &str = "active";
pub const PROJECTION_STATE_REVOKED: &str = "revoked";

/// One durable projection relation held by the origin. Refs and classes
/// only — the credential value is not representable in this record.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SecretProjectionRecord {
    pub projection_ref: String,
    pub credential_ref: String,
    pub source_provider_ref: String,
    pub target_workcell_ref: Option<String>,
    pub target_connection_label: Option<String>,
    pub target_provider_ref: Option<String>,
    pub target_allocation_ref: Option<String>,
    pub class: String,
    pub purpose: String,
    pub scope: String,
    pub requested_by: String,
    pub state: String,
    pub created_at_unix_ms: u64,
    pub revoked_at_unix_ms: Option<u64>,
    pub provenance: BTreeMap<String, String>,
}

impl SecretProjectionRecord {
    /// Build the origin's record for one validated projection request.
    pub fn from_request(
        projection_ref: impl Into<String>,
        request: &SecretProjectionRequest,
        created_at_unix_ms: u64,
    ) -> Result<Self> {
        request.validate()?;
        let (
            target_workcell_ref,
            target_connection_label,
            target_provider_ref,
            target_allocation_ref,
        ) = match &request.target {
            SecretProjectionTarget::Workcell {
                workcell_ref,
                connection_label,
            } => (
                Some(workcell_ref.to_string()),
                Some(connection_label.clone()),
                None,
                None,
            ),
            SecretProjectionTarget::Sandbox {
                provider_ref,
                allocation_ref,
            } => (
                None,
                None,
                Some(provider_ref.to_string()),
                Some(allocation_ref.clone()),
            ),
        };
        Ok(Self {
            projection_ref: projection_ref.into(),
            credential_ref: request.credential_ref.as_str().to_owned(),
            source_provider_ref: request.source_provider_ref.as_str().to_owned(),
            target_workcell_ref,
            target_connection_label,
            target_provider_ref,
            target_allocation_ref,
            class: request.class.as_str().to_owned(),
            purpose: request.purpose.clone(),
            scope: request.scope.clone(),
            requested_by: request.requested_by.as_str().to_owned(),
            state: PROJECTION_STATE_ACTIVE.to_owned(),
            created_at_unix_ms,
            revoked_at_unix_ms: None,
            provenance: BTreeMap::from([(
                "created_by".to_owned(),
                "workcell secret project".to_owned(),
            )]),
        })
    }

    /// Read the record back as the in-memory projection request, for
    /// executing the projection it grants.
    pub fn to_request(&self) -> Result<SecretProjectionRequest> {
        let target = match (
            self.target_workcell_ref.as_deref(),
            self.target_connection_label.as_deref(),
            self.target_provider_ref.as_deref(),
            self.target_allocation_ref.as_deref(),
        ) {
            (Some(workcell_ref), Some(connection_label), None, None) => {
                SecretProjectionTarget::Workcell {
                    workcell_ref: WorkcellRef::new(workcell_ref).map_err(|message| {
                        WorkcellError::InvalidDemand(format!(
                            "projection record holds an invalid workcell ref: {message}"
                        ))
                    })?,
                    connection_label: connection_label.to_owned(),
                }
            }
            (None, None, Some(provider_ref), Some(allocation_ref)) => {
                SecretProjectionTarget::Sandbox {
                    provider_ref: epilogos_workcell_core::ProviderRef::new(provider_ref).map_err(
                        |message| {
                            WorkcellError::InvalidDemand(format!(
                                "projection record holds an invalid provider ref: {message}"
                            ))
                        },
                    )?,
                    allocation_ref: allocation_ref.to_owned(),
                }
            }
            _ => {
                return Err(WorkcellError::InvalidDemand(
                    "projection record target is not a coherent workcell or sandbox relation"
                        .into(),
                ))
            }
        };
        let class = SecretMaterialisationClass::parse(&self.class).ok_or_else(|| {
            WorkcellError::InvalidDemand(format!(
                "projection record holds an unknown materialisation class `{}`",
                self.class
            ))
        })?;
        Ok(SecretProjectionRequest {
            credential_ref: epilogos_workcell_core::ExternalRef::new(&self.credential_ref)
                .map_err(|message| {
                    WorkcellError::InvalidDemand(format!(
                        "projection record holds an invalid credential ref: {message}"
                    ))
                })?,
            source_provider_ref: epilogos_workcell_core::ProviderRef::new(
                &self.source_provider_ref,
            )
            .map_err(|message| {
                WorkcellError::InvalidDemand(format!(
                    "projection record holds an invalid source provider ref: {message}"
                ))
            })?,
            target,
            class,
            purpose: self.purpose.clone(),
            scope: self.scope.clone(),
            requested_by: epilogos_workcell_core::ExternalRef::new(&self.requested_by).map_err(
                |message| {
                    WorkcellError::InvalidDemand(format!(
                        "projection record holds an invalid requester ref: {message}"
                    ))
                },
            )?,
        })
    }

    pub fn to_json(&self) -> Value {
        json!({
            "projection_ref": self.projection_ref,
            "credential_ref": self.credential_ref,
            "source_provider_ref": self.source_provider_ref,
            "target_workcell_ref": self.target_workcell_ref,
            "target_connection_label": self.target_connection_label,
            "target_provider_ref": self.target_provider_ref,
            "target_allocation_ref": self.target_allocation_ref,
            "class": self.class,
            "purpose": self.purpose,
            "scope": self.scope,
            "requested_by": self.requested_by,
            "state": self.state,
            "created_at_unix_ms": self.created_at_unix_ms,
            "revoked_at_unix_ms": self.revoked_at_unix_ms,
            "provenance": self.provenance,
        })
    }

    pub fn from_json(value: &Value) -> Result<Self> {
        let field = |name: &str| -> Result<String> {
            value
                .get(name)
                .and_then(Value::as_str)
                .map(str::to_owned)
                .ok_or_else(|| {
                    WorkcellError::InvalidDemand(format!("projection record is missing `{name}`"))
                })
        };
        let optional = |name: &str| value.get(name).and_then(Value::as_str).map(str::to_owned);
        let provenance = value
            .get("provenance")
            .and_then(Value::as_object)
            .map(|map| {
                map.iter()
                    .filter_map(|(key, value)| {
                        value.as_str().map(|value| (key.clone(), value.to_owned()))
                    })
                    .collect::<BTreeMap<String, String>>()
            })
            .unwrap_or_default();
        Ok(Self {
            projection_ref: field("projection_ref")?,
            credential_ref: field("credential_ref")?,
            source_provider_ref: field("source_provider_ref")?,
            target_workcell_ref: optional("target_workcell_ref"),
            target_connection_label: optional("target_connection_label"),
            target_provider_ref: optional("target_provider_ref"),
            target_allocation_ref: optional("target_allocation_ref"),
            class: field("class")?,
            purpose: field("purpose")?,
            scope: field("scope")?,
            requested_by: field("requested_by")?,
            state: field("state")?,
            created_at_unix_ms: value
                .get("created_at_unix_ms")
                .and_then(Value::as_u64)
                .ok_or_else(|| {
                    WorkcellError::InvalidDemand(
                        "projection record is missing `created_at_unix_ms`".into(),
                    )
                })?,
            revoked_at_unix_ms: value.get("revoked_at_unix_ms").and_then(Value::as_u64),
            provenance,
        })
    }
}

/// The origin's projection decision for one ref, computed fresh from the
/// file on every read so revocation takes effect at the target's next use.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProjectionDecision {
    Active(Box<SecretProjectionRecord>),
    /// Kept distinct from `Unknown` so the target hears the truth: a
    /// relation existed and was withdrawn.
    Revoked(Box<SecretProjectionRecord>),
    Unknown,
}

pub struct SecretProjectionLedger {
    path: PathBuf,
    workcell_ref: WorkcellRef,
}

impl SecretProjectionLedger {
    pub fn new(state_root: impl AsRef<Path>, workcell_ref: WorkcellRef) -> Self {
        Self {
            path: state_root
                .as_ref()
                .join(SECRETS_DIRECTORY)
                .join(PROJECTIONS_FILE),
            workcell_ref,
        }
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    fn empty_file(&self) -> Value {
        json!({
            "schema": PROJECTIONS_SCHEMA,
            "workcell_ref": self.workcell_ref.to_string(),
            "projections": {},
        })
    }

    /// Load the ledger. A missing file is an empty ledger; an unreadable or
    /// invalid file is named unavailability, never an empty result.
    pub fn load(&self) -> Result<Value> {
        if !self.path.exists() {
            return Ok(self.empty_file());
        }
        let text = fs::read_to_string(&self.path).map_err(|error| {
            WorkcellError::Unavailable(format!(
                "secret projection ledger at {} could not be read: {error}",
                self.path.display()
            ))
        })?;
        let value: Value = serde_json::from_str(&text).map_err(|error| {
            WorkcellError::Unavailable(format!(
                "secret projection ledger at {} is not valid JSON: {error}",
                self.path.display()
            ))
        })?;
        if value.get("schema").and_then(Value::as_str) != Some(PROJECTIONS_SCHEMA) {
            return Err(WorkcellError::Unavailable(format!(
                "secret projection ledger at {} does not carry schema {PROJECTIONS_SCHEMA}",
                self.path.display()
            )));
        }
        Ok(value)
    }

    fn save(&self, value: &Value) -> Result<()> {
        if let Some(parent) = self.path.parent() {
            fs::create_dir_all(parent).map_err(|error| {
                WorkcellError::OperationFailed(format!(
                    "could not create {} for the secret projection ledger: {error}",
                    parent.display()
                ))
            })?;
        }
        let text = serde_json::to_string_pretty(value).map_err(|error| {
            WorkcellError::OperationFailed(format!(
                "could not encode the secret projection ledger: {error}"
            ))
        })?;
        fs::write(&self.path, text).map_err(|error| {
            WorkcellError::OperationFailed(format!(
                "could not write the secret projection ledger at {}: {error}",
                self.path.display()
            ))
        })
    }

    /// Record a projection. An existing record with the same ref is a named
    /// conflict, never a silent overwrite — re-granting over an existing
    /// projection is how silent scope inflation happens.
    pub fn record(&self, record: SecretProjectionRecord) -> Result<()> {
        if record.projection_ref.trim().is_empty() {
            return Err(WorkcellError::InvalidDemand(
                "projection ref must not be empty".into(),
            ));
        }
        let mut file = self.load()?;
        let projections = file
            .as_object_mut()
            .and_then(|file| file.get_mut("projections"))
            .and_then(Value::as_object_mut)
            .ok_or_else(|| {
                WorkcellError::Unavailable(
                    "secret projection ledger has no `projections` object".into(),
                )
            })?;
        if projections.contains_key(&record.projection_ref) {
            return Err(WorkcellError::InvalidDemand(format!(
                "projection ref `{}` already exists in this ledger; revoke it first or choose another name",
                record.projection_ref
            )));
        }
        projections.insert(record.projection_ref.clone(), record.to_json());
        self.save(&file)
    }

    pub fn list(&self) -> Result<Vec<SecretProjectionRecord>> {
        let file = self.load()?;
        let mut records = Vec::new();
        if let Some(projections) = file.get("projections").and_then(Value::as_object) {
            for (projection_ref, value) in projections {
                let mut record = SecretProjectionRecord::from_json(value)?;
                if record.projection_ref != *projection_ref {
                    return Err(WorkcellError::Unavailable(format!(
                        "secret projection ledger key `{projection_ref}` does not match its record ref `{}`",
                        record.projection_ref
                    )));
                }
                records.push(record);
            }
        }
        records.sort_by(|a, b| a.projection_ref.cmp(&b.projection_ref));
        Ok(records)
    }

    pub fn decision_for(&self, projection_ref: &str) -> Result<ProjectionDecision> {
        let file = self.load()?;
        let Some(value) = file.get("projections").and_then(|p| p.get(projection_ref)) else {
            return Ok(ProjectionDecision::Unknown);
        };
        let record = SecretProjectionRecord::from_json(value)?;
        if record.state == PROJECTION_STATE_REVOKED {
            Ok(ProjectionDecision::Revoked(Box::new(record)))
        } else {
            Ok(ProjectionDecision::Active(Box::new(record)))
        }
    }

    /// Revoke a projection. The record is kept as audit evidence; the
    /// revocation reaches the projected target at its next use.
    pub fn revoke(
        &self,
        projection_ref: &str,
        revoked_at_unix_ms: u64,
    ) -> Result<SecretProjectionRecord> {
        let decision = self.decision_for(projection_ref)?;
        let mut record = match decision {
            ProjectionDecision::Active(record) | ProjectionDecision::Revoked(record) => *record,
            ProjectionDecision::Unknown => {
                return Err(WorkcellError::InvalidDemand(format!(
                    "no secret projection `{projection_ref}` exists in this ledger"
                )))
            }
        };
        if record.state == PROJECTION_STATE_REVOKED {
            return Ok(record);
        }
        record.state = PROJECTION_STATE_REVOKED.to_owned();
        record.revoked_at_unix_ms = Some(revoked_at_unix_ms);
        let mut file = self.load()?;
        if let Some(projections) = file.get_mut("projections").and_then(Value::as_object_mut) {
            projections.insert(record.projection_ref.clone(), record.to_json());
        }
        self.save(&file)?;
        Ok(record)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use epilogos_workcell_core::{ExternalRef, ProviderRef, SecretMaterialisationClass};

    fn sandbox_request() -> SecretProjectionRequest {
        SecretProjectionRequest {
            credential_ref: ExternalRef::new("credential:github/operator").unwrap(),
            source_provider_ref: ProviderRef::new("secret-provider:keychain/macos").unwrap(),
            target: SecretProjectionTarget::Sandbox {
                provider_ref: ProviderRef::new("provider:opensandbox").unwrap(),
                allocation_ref: "sbx_fixture".into(),
            },
            class: SecretMaterialisationClass::CredentialBroker,
            purpose: "github-api".into(),
            scope: "repo:read".into(),
            requested_by: ExternalRef::new("agent-session:fixture").unwrap(),
        }
    }

    fn ledger() -> (SecretProjectionLedger, std::path::PathBuf) {
        let root = std::env::temp_dir().join(format!(
            "wck-projections-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let ledger =
            SecretProjectionLedger::new(&root, WorkcellRef::new("workcell:origin").unwrap());
        (ledger, root)
    }

    #[test]
    fn record_round_trips_through_the_ledger() {
        let (ledger, root) = ledger();
        let record = SecretProjectionRecord::from_request(
            "secret-projection:github-operator",
            &sandbox_request(),
            1_000,
        )
        .unwrap();
        ledger.record(record.clone()).unwrap();

        let listed = ledger.list().unwrap();
        assert_eq!(listed, vec![record.clone()]);

        match ledger
            .decision_for("secret-projection:github-operator")
            .unwrap()
        {
            ProjectionDecision::Active(record) => {
                let request = record.to_request().unwrap();
                assert_eq!(request, sandbox_request());
            }
            other => panic!("expected an active decision, got {other:?}"),
        }
        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn duplicate_projection_ref_is_a_named_conflict() {
        let (ledger, root) = ledger();
        let record = SecretProjectionRecord::from_request(
            "secret-projection:dupe",
            &sandbox_request(),
            1_000,
        )
        .unwrap();
        ledger.record(record).unwrap();
        let second = SecretProjectionRecord::from_request(
            "secret-projection:dupe",
            &sandbox_request(),
            2_000,
        )
        .unwrap();
        let err = ledger.record(second).unwrap_err();
        assert!(format!("{err:?}").contains("already exists"));
        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn revocation_marks_keeps_and_reaches_the_next_decision() {
        let (ledger, root) = ledger();
        let record = SecretProjectionRecord::from_request(
            "secret-projection:revme",
            &sandbox_request(),
            1_000,
        )
        .unwrap();
        ledger.record(record).unwrap();

        let revoked = ledger.revoke("secret-projection:revme", 2_000).unwrap();
        assert_eq!(revoked.state, PROJECTION_STATE_REVOKED);
        assert_eq!(revoked.revoked_at_unix_ms, Some(2_000));

        // The record is kept as audit evidence.
        assert_eq!(ledger.list().unwrap().len(), 1);
        match ledger.decision_for("secret-projection:revme").unwrap() {
            ProjectionDecision::Revoked(_) => {}
            other => panic!("expected a revoked decision, got {other:?}"),
        }
        // Revoking again is idempotent and honest about the state.
        let again = ledger.revoke("secret-projection:revme", 3_000).unwrap();
        assert_eq!(again.revoked_at_unix_ms, Some(2_000));
        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn unknown_projection_is_unknown_not_refused() {
        let (ledger, root) = ledger();
        assert_eq!(
            ledger.decision_for("secret-projection:nonesuch").unwrap(),
            ProjectionDecision::Unknown
        );
        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn workcell_target_record_round_trips() {
        let (ledger, root) = ledger();
        let mut request = sandbox_request();
        request.target = SecretProjectionTarget::Workcell {
            workcell_ref: WorkcellRef::new("workcell:remote").unwrap(),
            connection_label: "laptop".into(),
        };
        request.class = SecretMaterialisationClass::File;
        let record =
            SecretProjectionRecord::from_request("secret-projection:remote", &request, 1_000)
                .unwrap();
        ledger.record(record).unwrap();
        match ledger.decision_for("secret-projection:remote").unwrap() {
            ProjectionDecision::Active(record) => {
                assert_eq!(record.to_request().unwrap(), request);
            }
            other => panic!("expected active, got {other:?}"),
        }
        std::fs::remove_dir_all(&root).ok();
    }
}
