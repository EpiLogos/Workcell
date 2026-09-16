//! Serving-side connection grants.
//!
//! A grant is one client's permissioned relation to this cell: a client
//! label, the control operations it may invoke, what discovery advertises to
//! it, and the SHA-256 of its bearer credential. Grants are durable and
//! auditable: revocation marks the record and keeps it, and every authorise
//! check re-reads the registry so a revocation takes effect at the next use
//! — including from another process such as the `workcell revoke` command.
//! A grant may also carry an expiry (`--expires-in`): past that instant it
//! refuses at the next use with its own named decision, distinct from
//! revocation, and the record is kept. A grant created without an expiry
//! never expires.
//!
//! The store is one JSON file under the workcell state root
//! (`<state-root>/connections/grants.json`), following the same law as the
//! harness-instance registry: a missing file is an empty registry, an
//! unreadable or invalid file is named unavailability, and the file is bound
//! to one `workcell_ref`.

use std::{
    fs,
    path::{Path, PathBuf},
};

use epilogos_workcell_core::{Result, WorkcellError, WorkcellRef};
use epilogos_workcell_wire::{
    decode_grant_value, grant_value, ConnectionGrant, GRANT_STATE_ACTIVE, GRANT_STATE_REVOKED,
};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};

pub const CONNECTIONS_DIRECTORY: &str = "connections";
pub const GRANTS_FILE: &str = "grants.json";
pub const GRANTS_SCHEMA: &str = "workcell.connection-grants/v1";

/// Outcome of creating a grant.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CreateOutcome {
    Registered,
    /// An active grant already holds this exact credential. Named and
    /// returned; never auto-resolved — re-authorising over an existing
    /// credential is how silent permission inflation happens.
    Conflict {
        existing: Box<ConnectionGrant>,
    },
}

/// Result of one authorise check. Computed fresh from the registry file on
/// every call so revocation propagates at use time.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GrantDecision {
    Allowed {
        grant: Box<ConnectionGrant>,
    },
    /// No credential was presented at all.
    NoCredential,
    /// A credential was presented but no active grant holds its digest.
    UnknownCredential,
    /// The credential is valid but the operation is outside the grant.
    NotPermitted {
        grant: Box<ConnectionGrant>,
    },
    /// The credential matches a grant this cell issued and then revoked.
    /// Kept distinct from `UnknownCredential` so the client hears the truth:
    /// access existed and was withdrawn.
    Revoked {
        grant: Box<ConnectionGrant>,
    },
    /// The credential matches an active grant whose window has closed.
    /// Kept distinct from `Revoked` so the client hears the truth: access
    /// ended on its own terms, not by operator action.
    Expired {
        grant: Box<ConnectionGrant>,
    },
}

impl GrantDecision {
    /// The granted scope, when the decision allows the operation.
    pub fn grant(&self) -> Option<&ConnectionGrant> {
        match self {
            Self::Allowed { grant } | Self::NotPermitted { grant } => Some(grant),
            _ => None,
        }
    }
}

pub struct ConnectionGrants {
    path: PathBuf,
    workcell_ref: WorkcellRef,
}

impl ConnectionGrants {
    pub fn new(state_root: impl AsRef<Path>, workcell_ref: WorkcellRef) -> Self {
        Self {
            path: state_root
                .as_ref()
                .join(CONNECTIONS_DIRECTORY)
                .join(GRANTS_FILE),
            workcell_ref,
        }
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    fn empty_file(&self) -> Value {
        json!({
            "schema": GRANTS_SCHEMA,
            "workcell_ref": self.workcell_ref.to_string(),
            "grants": {},
        })
    }

    /// Load the registry. A missing file is an empty registry; an unreadable
    /// or invalid file is named unavailability, never an empty result.
    pub fn load(&self) -> Result<Value> {
        if !self.path.exists() {
            return Ok(self.empty_file());
        }
        let raw = fs::read_to_string(&self.path).map_err(|error| {
            WorkcellError::OperationFailed(format!(
                "read connection grants {}: {error}",
                self.path.display()
            ))
        })?;
        let parsed: Value = serde_json::from_str(&raw).map_err(|error| {
            WorkcellError::OperationFailed(format!(
                "parse connection grants {}: {error}",
                self.path.display()
            ))
        })?;
        let object = parsed.as_object().ok_or_else(|| {
            WorkcellError::OperationFailed(format!(
                "connection grants {} must be a JSON object",
                self.path.display()
            ))
        })?;
        if object.get("schema").and_then(Value::as_str) != Some(GRANTS_SCHEMA) {
            return Err(WorkcellError::OperationFailed(format!(
                "connection grants {} does not carry schema {GRANTS_SCHEMA}",
                self.path.display()
            )));
        }
        if object.get("workcell_ref").and_then(Value::as_str) != Some(self.workcell_ref.as_str()) {
            return Err(WorkcellError::OperationFailed(format!(
                "connection grants {} belong to {}, not {}",
                self.path.display(),
                object
                    .get("workcell_ref")
                    .and_then(Value::as_str)
                    .unwrap_or("unknown"),
                self.workcell_ref
            )));
        }
        for (reference, value) in
            object
                .get("grants")
                .and_then(Value::as_object)
                .ok_or_else(|| {
                    WorkcellError::OperationFailed(format!(
                        "connection grants {} has no grants map",
                        self.path.display()
                    ))
                })?
        {
            let grant = decode_grant_value(value).map_err(|error| {
                WorkcellError::OperationFailed(format!(
                    "connection grant `{reference}` in {} is invalid: {error}",
                    self.path.display()
                ))
            })?;
            if grant.grant_ref != reference.as_str() {
                return Err(WorkcellError::OperationFailed(format!(
                    "connection grant keyed `{reference}` carries ref `{}`",
                    grant.grant_ref
                )));
            }
        }
        Ok(parsed)
    }

    fn store(&self, file: &Value) -> Result<()> {
        if let Some(parent) = self.path.parent() {
            fs::create_dir_all(parent).map_err(|error| {
                WorkcellError::OperationFailed(format!(
                    "create connection registry directory {}: {error}",
                    parent.display()
                ))
            })?;
        }
        let temp = self
            .path
            .with_extension(format!("json.pending-{}", std::process::id()));
        fs::write(
            &temp,
            serde_json::to_vec_pretty(file).map_err(|error| {
                WorkcellError::OperationFailed(format!("encode connection grants: {error}"))
            })?,
        )
        .map_err(|error| {
            WorkcellError::OperationFailed(format!(
                "write connection grants {}: {error}",
                temp.display()
            ))
        })?;
        fs::rename(&temp, &self.path).map_err(|error| {
            WorkcellError::OperationFailed(format!(
                "publish connection grants {}: {error}",
                self.path.display()
            ))
        })
    }

    pub fn list(&self) -> Result<Vec<ConnectionGrant>> {
        let file = self.load()?;
        Ok(file["grants"]
            .as_object()
            .expect("validated registry has a grants map")
            .values()
            .map(|value| decode_grant_value(value).expect("validated registry grants decode"))
            .collect())
    }

    pub fn active_count(&self) -> Result<usize> {
        let now = unix_millis();
        Ok(self
            .list()?
            .into_iter()
            .filter(|grant| grant.is_active() && !grant.is_expired_at(now))
            .count())
    }

    /// Register one validated grant. The caller supplies the credential
    /// digest — this layer never sees credential material.
    pub fn create(&self, grant: ConnectionGrant) -> Result<CreateOutcome> {
        validate_grant(&grant)?;
        let mut file = self.load()?;
        let grants = file
            .get_mut("grants")
            .and_then(Value::as_object_mut)
            .expect("validated registry has a grants map");
        if let Some(existing_value) = grants.get(&grant.grant_ref) {
            let existing =
                decode_grant_value(existing_value).expect("validated registry grants decode");
            return Ok(CreateOutcome::Conflict {
                existing: Box::new(existing),
            });
        }
        for existing_value in grants.values() {
            let existing =
                decode_grant_value(existing_value).expect("validated registry grants decode");
            if existing.is_active() && existing.credential_sha256 == grant.credential_sha256 {
                return Ok(CreateOutcome::Conflict {
                    existing: Box::new(existing),
                });
            }
        }
        grants.insert(
            grant.grant_ref.clone(),
            grant_value(&grant).expect("validated grant encodes"),
        );
        self.store(&file)?;
        Ok(CreateOutcome::Registered)
    }

    /// Check a presented credential against this operation, reading the
    /// registry file fresh so a concurrent `workcell revoke` takes effect
    /// at the next use. An expired grant refuses here for as long as it
    /// sits in the registry — evaluation is at use time, never at write
    /// time.
    pub fn authorise(&self, credential: Option<&str>, operation: &str) -> Result<GrantDecision> {
        let Some(credential) = credential.filter(|value| !value.is_empty()) else {
            return Ok(GrantDecision::NoCredential);
        };
        let digest = credential_sha256(credential);
        let now = unix_millis();
        let file = self.load()?;
        for value in file["grants"]
            .as_object()
            .expect("validated registry has a grants map")
            .values()
        {
            let grant = decode_grant_value(value).expect("validated registry grants decode");
            if grant.credential_sha256 != digest {
                continue;
            }
            if !grant.is_active() {
                // A matching but revoked grant is a named refusal, not
                // anonymity: the client's credential was known and was
                // withdrawn here.
                return Ok(GrantDecision::Revoked {
                    grant: Box::new(grant),
                });
            }
            if grant.is_expired_at(now) {
                // Same law for expiry: the credential was known and its
                // window closed. Named, and distinct from revocation.
                return Ok(GrantDecision::Expired {
                    grant: Box::new(grant),
                });
            }
            if grant.operations.iter().any(|allowed| allowed == operation) {
                return Ok(GrantDecision::Allowed {
                    grant: Box::new(grant),
                });
            }
            return Ok(GrantDecision::NotPermitted {
                grant: Box::new(grant),
            });
        }
        Ok(GrantDecision::UnknownCredential)
    }

    /// Revoke by `grant_ref` or by client label. Revoked grants stay in the
    /// registry with their revocation timestamp; nothing is deleted.
    pub fn revoke(&self, target: &str) -> Result<Vec<ConnectionGrant>> {
        let mut file = self.load()?;
        let grants = file
            .get_mut("grants")
            .and_then(Value::as_object_mut)
            .expect("validated registry has a grants map");
        let now = unix_millis();
        let mut revoked = Vec::new();
        for value in grants.values_mut() {
            let mut grant = decode_grant_value(value).expect("validated registry grants decode");
            let matches = grant.grant_ref == target || grant.client_label == target;
            if !matches || !grant.is_active() {
                continue;
            }
            grant.state = GRANT_STATE_REVOKED.to_owned();
            grant.revoked_at_unix_ms = Some(now);
            *value = grant_value(&grant).expect("revoked grant encodes");
            revoked.push(grant);
        }
        if revoked.is_empty() {
            return Err(WorkcellError::NotFound(format!(
                "no active connection grant matches `{target}`"
            )));
        }
        self.store(&file)?;
        Ok(revoked)
    }
}

/// Validate a grant before it enters the registry.
pub fn validate_grant(grant: &ConnectionGrant) -> Result<()> {
    if grant.state != GRANT_STATE_ACTIVE && grant.state != GRANT_STATE_REVOKED {
        return Err(WorkcellError::InvalidDemand(format!(
            "unknown connection grant state `{}`",
            grant.state
        )));
    }
    validate_label(&grant.client_label)?;
    if !grant.grant_ref.starts_with("grant:") || grant.grant_ref.trim() != grant.grant_ref {
        return Err(WorkcellError::InvalidDemand(format!(
            "connection grant ref must start with `grant:` and have no surrounding whitespace, got `{}`",
            grant.grant_ref
        )));
    }
    if grant.credential_sha256.is_empty() {
        return Err(WorkcellError::InvalidDemand(
            "connection grant requires the credential SHA-256 digest; grants never carry credential material".into(),
        ));
    }
    if grant.operations.is_empty() {
        return Err(WorkcellError::InvalidDemand(format!(
            "connection grant `{}` permits no operations; name at least one --allow operation so the grant says exactly what it grants",
            grant.grant_ref
        )));
    }
    for operation in &grant.operations {
        if !crate::CONTROL_OPERATIONS.contains(&operation.as_str()) {
            return Err(WorkcellError::InvalidDemand(format!(
                "connection grant `{}` permits unknown operation `{operation}`; known operations are {}",
                grant.grant_ref,
                crate::CONTROL_OPERATIONS.join(", ")
            )));
        }
    }
    Ok(())
}

/// Client labels become grant identity and, on the client side, file names.
/// Restrict them to safe characters up front.
pub fn validate_label(label: &str) -> Result<()> {
    if label.trim().is_empty() || label.trim() != label {
        return Err(WorkcellError::InvalidDemand(
            "connection label must be non-empty without surrounding whitespace".into(),
        ));
    }
    if label == GRANTS_FILE.trim_end_matches(".json") {
        return Err(WorkcellError::InvalidDemand(
            "`grants` is reserved for the serving cell's grant registry; choose another connection label".into(),
        ));
    }
    if !label
        .chars()
        .all(|character| character.is_ascii_alphanumeric() || matches!(character, '-' | '_' | '.'))
    {
        return Err(WorkcellError::InvalidDemand(format!(
            "connection label `{label}` may only contain letters, digits, `-`, `_` and `.`"
        )));
    }
    Ok(())
}

/// Derive a stable grant ref from the client label and the credential
/// digest: re-authorising the same label with a new credential produces a
/// new, separately revocable grant instead of overwriting history.
pub fn grant_ref_for(label: &str, credential: &str) -> String {
    let digest = credential_sha256(credential);
    format!("grant:{label}-{}", &digest[..12])
}

pub fn credential_sha256(credential: &str) -> String {
    hex(&Sha256::digest(credential.as_bytes()))
}

/// Generate one bearer credential. The material is shown once at grant
/// creation and stored nowhere; only its digest enters the registry.
#[cfg(unix)]
pub fn generate_credential() -> Result<String> {
    use std::io::Read;
    let mut bytes = [0u8; 32];
    fs::File::open("/dev/urandom")
        .and_then(|mut file| file.read_exact(&mut bytes))
        .map_err(|error| {
            WorkcellError::Unavailable(format!(
                "generate connection credential: cannot read system entropy (/dev/urandom): {error}"
            ))
        })?;
    Ok(format!("wck_{}", hex(&bytes)))
}

#[cfg(not(unix))]
pub fn generate_credential() -> Result<String> {
    Err(WorkcellError::Unavailable(
        "connection credential generation requires a system entropy source; this platform is not supported by this Workcell increment".into(),
    ))
}

/// Parse a grant lifetime (`30m`, `12h`, `7d`, `45s`) into milliseconds.
/// Zero, negative and malformed values are refused with the usage line, so
/// a typo never creates a grant that is born expired.
pub fn parse_duration_millis(raw: &str) -> Result<u64> {
    let invalid = || {
        WorkcellError::InvalidDemand(format!(
            "invalid `--expires-in` duration `{raw}`; use `<number><s|m|h|d>` (for example `30m`, `12h`, `7d`) with a value above zero"
        ))
    };
    let mut digits = raw.chars();
    let unit_millis = match digits.next_back() {
        Some('s') => 1_000_u64,
        Some('m') => 60 * 1_000,
        Some('h') => 60 * 60 * 1_000,
        Some('d') => 24 * 60 * 60 * 1_000,
        _ => return Err(invalid()),
    };
    let amount: u64 = digits.as_str().parse().map_err(|_| invalid())?;
    if amount == 0 {
        return Err(invalid());
    }
    amount.checked_mul(unit_millis).ok_or_else(invalid)
}

pub fn unix_millis() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_millis() as u64)
        .unwrap_or(0)
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_root(label: &str) -> std::path::PathBuf {
        let nonce = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        std::env::temp_dir().join(format!(
            "workcell-grants-{label}-{}-{nonce}",
            std::process::id()
        ))
    }

    fn registry(label: &str) -> ConnectionGrants {
        ConnectionGrants::new(
            temp_root(label),
            WorkcellRef::new("workcell:grants-test").unwrap(),
        )
    }

    fn grant_for(label: &str, credential: &str, operations: &[&str]) -> ConnectionGrant {
        ConnectionGrant {
            grant_ref: grant_ref_for(label, credential),
            client_label: label.to_owned(),
            protocol: crate::CONTROL_PROTOCOL_VERSION.to_owned(),
            operations: operations.iter().map(|value| value.to_string()).collect(),
            advertise: Vec::new(),
            credential_sha256: credential_sha256(credential),
            credential_ref: None,
            created_at_unix_ms: unix_millis(),
            expires_at_unix_ms: None,
            state: GRANT_STATE_ACTIVE.to_owned(),
            revoked_at_unix_ms: None,
            provenance: BTreeMap::new(),
        }
    }

    use std::collections::BTreeMap;

    #[test]
    fn missing_registry_is_empty_and_loads_cleanly() {
        let grants = registry("empty");
        assert!(grants.list().unwrap().is_empty());
        assert_eq!(grants.active_count().unwrap(), 0);
    }

    #[test]
    fn authorise_follows_the_whole_lifecycle_at_use_time() {
        let grants = registry("lifecycle");
        let credential = generate_credential().unwrap();

        // No grant yet: unknown credential.
        assert_eq!(
            grants.authorise(Some(&credential), "discover").unwrap(),
            GrantDecision::UnknownCredential
        );

        grants
            .create(grant_for("laptop", &credential, &["status", "discover"]))
            .unwrap();

        let allowed = grants.authorise(Some(&credential), "discover").unwrap();
        assert!(matches!(allowed, GrantDecision::Allowed { .. }));

        // Valid credential, operation outside the grant.
        assert!(matches!(
            grants.authorise(Some(&credential), "release").unwrap(),
            GrantDecision::NotPermitted { .. }
        ));

        // Revoke from "another process" — a fresh handle on the same file.
        let second_handle = ConnectionGrants::new(
            grants.path().parent().unwrap().parent().unwrap(),
            WorkcellRef::new("workcell:grants-test").unwrap(),
        );
        let revoked = second_handle.revoke("laptop").unwrap();
        assert_eq!(revoked.len(), 1);
        assert_eq!(revoked[0].state, GRANT_STATE_REVOKED);
        assert!(revoked[0].revoked_at_unix_ms.is_some());

        // Revocation takes effect at the next use, with the grant retained
        // in the file as audit evidence.
        let revoked_decision = grants.authorise(Some(&credential), "discover").unwrap();
        match revoked_decision {
            GrantDecision::Revoked { grant } => {
                assert_eq!(grant.client_label, "laptop");
            }
            other => panic!("expected revoked decision, got {other:?}"),
        }
        assert_eq!(grants.list().unwrap().len(), 1);
        assert_eq!(grants.active_count().unwrap(), 0);
    }

    #[test]
    fn expired_grants_refuse_as_expired_distinct_from_revoked() {
        let grants = registry("expiry");
        let expired_credential = generate_credential().unwrap();
        let live_credential = generate_credential().unwrap();
        let now = unix_millis();

        // One grant whose window already closed, one with a future expiry,
        // one created without an expiry.
        let mut expired = grant_for("laptop", &expired_credential, &["status"]);
        expired.expires_at_unix_ms = Some(now.saturating_sub(1));
        grants.create(expired).unwrap();
        let mut live = grant_for("tablet", &live_credential, &["status"]);
        live.expires_at_unix_ms = Some(now + 60_000);
        grants.create(live).unwrap();

        // The expired grant names itself and says `expired`, and never
        // masquerades as unknown or revoked.
        match grants
            .authorise(Some(&expired_credential), "status")
            .unwrap()
        {
            GrantDecision::Expired { grant } => {
                assert_eq!(grant.client_label, "laptop");
                assert!(grant.is_expired_at(now));
            }
            other => panic!("expected expired decision, got {other:?}"),
        }

        // A future expiry is still inside the window: allowed like any
        // active grant.
        assert!(matches!(
            grants.authorise(Some(&live_credential), "status").unwrap(),
            GrantDecision::Allowed { .. }
        ));

        // An expired grant cannot authorise, so it is not an active grant:
        // the serve-side gate must not lean on it.
        assert_eq!(grants.active_count().unwrap(), 1);
        assert_eq!(grants.list().unwrap().len(), 2);
    }

    #[test]
    fn durations_parse_and_non_positive_or_malformed_values_are_refused() {
        assert_eq!(parse_duration_millis("45s").unwrap(), 45_000);
        assert_eq!(parse_duration_millis("30m").unwrap(), 30 * 60_000);
        assert_eq!(parse_duration_millis("12h").unwrap(), 12 * 60 * 60_000);
        assert_eq!(parse_duration_millis("7d").unwrap(), 7 * 24 * 60 * 60_000);
        for bad in [
            "0m",
            "0",
            "-5m",
            "banana",
            "30",
            "30x",
            "",
            "m",
            "30分",
            "99999999999999999999d",
        ] {
            let error = parse_duration_millis(bad).unwrap_err();
            assert!(
                matches!(error, WorkcellError::InvalidDemand(_)),
                "`{bad}` should be a usage error, got {error:?}"
            );
        }
    }

    #[test]
    fn same_credential_cannot_be_regranted_while_active() {
        let grants = registry("conflict");
        let credential = generate_credential().unwrap();
        grants
            .create(grant_for("tablet", &credential, &["status"]))
            .unwrap();
        let conflict = grants.create(grant_for("other-label", &credential, &["status"]));
        match conflict {
            Ok(CreateOutcome::Conflict { existing }) => {
                assert_eq!(existing.client_label, "tablet");
            }
            other => panic!("expected conflict, got {other:?}"),
        }
    }

    #[test]
    fn revoke_by_unknown_target_is_not_found() {
        let grants = registry("missing");
        let error = grants.revoke("nobody").unwrap_err();
        assert!(matches!(error, WorkcellError::NotFound(_)));
    }

    #[test]
    fn grants_without_operations_or_with_unknown_operations_are_refused() {
        assert!(validate_grant(&grant_for("x", "cred", &[])).is_err());
        assert!(validate_grant(&grant_for("x", "cred", &["launch-missiles"])).is_err());
        assert!(validate_grant(&grant_for("bad label", "cred", &["status"])).is_err());
        assert!(validate_grant(&grant_for("grants", "cred", &["status"])).is_err());
    }

    #[test]
    fn foreign_or_invalid_registry_files_are_named_unavailability() {
        let root = temp_root("foreign");
        let grants =
            ConnectionGrants::new(&root, WorkcellRef::new("workcell:grants-test").unwrap());
        std::fs::create_dir_all(grants.path().parent().unwrap()).unwrap();
        std::fs::write(
            grants.path(),
            json!({"schema": GRANTS_SCHEMA, "workcell_ref": "workcell:elsewhere", "grants": {}})
                .to_string(),
        )
        .unwrap();
        let error = grants.list().unwrap_err();
        assert!(matches!(error, WorkcellError::OperationFailed(_)));
        assert!(error.to_string().contains("workcell:elsewhere"));
    }
}
