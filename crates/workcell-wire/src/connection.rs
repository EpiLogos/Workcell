//! Cross-cell connection records on the wire.
//!
//! A connection is a durable, revocable, expirable, permissioned relation
//! between two cells, not a socket. Two record types carry it:
//!
//! * `workcell.connection/v1` — the client-side receipt of one relation:
//!   endpoint identity, agreed protocol, granted operations, credential
//!   *reference* (never material) and the reconciled state of the last
//!   connect/reconnect attempt.
//! * `workcell.connection-grant/v1` — the serving-side grant: which client
//!   label, which operations, which capability advertisement, and the
//!   SHA-256 of the credential. Grant material is never recorded anywhere.
//!
//! Both follow the material-world wire law: explicit version, explicit
//! fields, unsupported versions and unknown values refused loudly — never
//! silently reinterpreted.

use std::collections::BTreeMap;

use epilogos_workcell_core::{Result, WorkcellError};
use serde_json::{json, Map, Value};

pub const CONNECTION_WIRE_VERSION: &str = "workcell.connection/v1";
pub const CONNECTION_GRANT_WIRE_VERSION: &str = "workcell.connection-grant/v1";

/// Connection record states. `connected` and `disconnected` describe the
/// client-side relation; `refused` and `incompatible` are the two loud
/// refusal states (authorisation refused; unsupported protocol combination).
pub const CONNECTION_STATES: [&str; 4] = ["connected", "disconnected", "refused", "incompatible"];

pub const GRANT_STATE_ACTIVE: &str = "active";
pub const GRANT_STATE_REVOKED: &str = "revoked";
pub const GRANT_STATES: [&str; 2] = [GRANT_STATE_ACTIVE, GRANT_STATE_REVOKED];

/// Client-side receipt of one cross-cell connection.
#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub struct ConnectionRecord {
    pub connection_ref: String,
    pub label: String,
    pub endpoint: String,
    /// The control protocol the two cells agreed on (or attempted, when the
    /// connection was refused as incompatible).
    pub protocol: String,
    pub remote_workcell_ref: Option<String>,
    /// Remote software disclosure; absent when the remote never authorised
    /// this client far enough to disclose it.
    pub remote_software: Option<String>,
    pub local_software: String,
    /// The operations the serving cell granted — exactly what was granted,
    /// which is not the same as what the remote advertises.
    pub granted_operations: Vec<String>,
    /// Where the credential lives (for example `keychain://…`). A location,
    /// never the credential material.
    pub credential_ref: Option<String>,
    pub state: String,
    /// Last reconciliation note: refusal reason, operator disconnect, or the
    /// compatibility observations from the last successful handshake.
    pub detail: Option<String>,
    pub connected_at_unix_ms: Option<u64>,
    /// When the serving cell's grant stops authorising, as disclosed by the
    /// last handshake. `None` when the grant has no expiry or the cell never
    /// authorised this client.
    pub expires_at_unix_ms: Option<u64>,
    pub last_reconciled_at_unix_ms: u64,
    pub provenance: BTreeMap<String, String>,
}

impl ConnectionRecord {
    pub fn state(&self) -> Result<&str> {
        state_name(&self.state)
    }
}

/// Serving-side grant for one connecting client.
#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub struct ConnectionGrant {
    pub grant_ref: String,
    pub client_label: String,
    pub protocol: String,
    /// Control operations the grant permits. Capability advertisement is
    /// separate: advertising is not authorisation.
    pub operations: Vec<String>,
    /// Provider ports advertised to this client in discovery; empty means
    /// the cell's whole advertisement is visible to the grant.
    pub advertise: Vec<String>,
    /// SHA-256 of the bearer credential. The material itself is never
    /// stored by any Workcell surface.
    pub credential_sha256: String,
    /// Optional keychain location where the client asked the granting cell
    /// to keep the material.
    pub credential_ref: Option<String>,
    pub created_at_unix_ms: u64,
    /// When the grant stops authorising, in Unix milliseconds. `None` is a
    /// grant created without `--expires-in`: it never expires. An expired
    /// grant is refused at the next use — named and distinct from
    /// revocation; the record itself is kept as audit evidence.
    pub expires_at_unix_ms: Option<u64>,
    pub state: String,
    pub revoked_at_unix_ms: Option<u64>,
    pub provenance: BTreeMap<String, String>,
}

impl ConnectionGrant {
    pub fn is_active(&self) -> bool {
        self.state == GRANT_STATE_ACTIVE
    }

    /// Whether the grant's window has closed at `now_unix_ms`. A grant
    /// without an expiry never expires.
    pub fn is_expired_at(&self, now_unix_ms: u64) -> bool {
        self.expires_at_unix_ms
            .is_some_and(|expiry| expiry <= now_unix_ms)
    }
}

pub fn encode_connection(record: &ConnectionRecord) -> Result<String> {
    serde_json::to_string_pretty(&connection_value(record)?).map_err(|error| {
        WorkcellError::OperationFailed(format!("encode connection record: {error}"))
    })
}

pub fn decode_connection(input: &str) -> Result<ConnectionRecord> {
    let value: Value = serde_json::from_str(input).map_err(|error| {
        WorkcellError::InvalidDemand(format!("decode connection record: {error}"))
    })?;
    decode_connection_value(&value)
}

pub fn connection_value(record: &ConnectionRecord) -> Result<Value> {
    state_name(&record.state)?;
    Ok(json!({
        "version": CONNECTION_WIRE_VERSION,
        "connection_ref": record.connection_ref,
        "label": record.label,
        "endpoint": record.endpoint,
        "protocol": record.protocol,
        "remote_workcell_ref": record.remote_workcell_ref,
        "remote_software": record.remote_software,
        "local_software": record.local_software,
        "granted_operations": record.granted_operations,
        "credential_ref": record.credential_ref,
        "state": record.state,
        "detail": record.detail,
        "connected_at_unix_ms": record.connected_at_unix_ms,
        "expires_at_unix_ms": record.expires_at_unix_ms,
        "last_reconciled_at_unix_ms": record.last_reconciled_at_unix_ms,
        "provenance": record.provenance,
    }))
}

pub fn decode_connection_value(value: &Value) -> Result<ConnectionRecord> {
    let record = object(value, "connection record")?;
    let version = string_field(record, "version")?;
    if version != CONNECTION_WIRE_VERSION {
        return Err(WorkcellError::Unsupported(format!(
            "connection record version `{version}` is not supported"
        )));
    }
    Ok(ConnectionRecord {
        connection_ref: string_field(record, "connection_ref")?.to_owned(),
        label: string_field(record, "label")?.to_owned(),
        endpoint: string_field(record, "endpoint")?.to_owned(),
        protocol: string_field(record, "protocol")?.to_owned(),
        remote_workcell_ref: optional_string_field(record, "remote_workcell_ref")?
            .map(str::to_owned),
        remote_software: optional_string_field(record, "remote_software")?.map(str::to_owned),
        local_software: string_field(record, "local_software")?.to_owned(),
        granted_operations: string_array(record, "granted_operations")?,
        credential_ref: optional_string_field(record, "credential_ref")?.map(str::to_owned),
        state: state_name(string_field(record, "state")?)?.to_owned(),
        detail: optional_string_field(record, "detail")?.map(str::to_owned),
        connected_at_unix_ms: optional_u64_field(record, "connected_at_unix_ms")?,
        expires_at_unix_ms: optional_u64_field_or_absent(record, "expires_at_unix_ms")?,
        last_reconciled_at_unix_ms: u64_field(record, "last_reconciled_at_unix_ms")?,
        provenance: string_map_field(record, "provenance")?,
    })
}

pub fn grant_value(grant: &ConnectionGrant) -> Result<Value> {
    if grant.state != GRANT_STATE_ACTIVE && grant.state != GRANT_STATE_REVOKED {
        return Err(WorkcellError::InvalidDemand(format!(
            "unknown connection grant state `{}`",
            grant.state
        )));
    }
    Ok(json!({
        "version": CONNECTION_GRANT_WIRE_VERSION,
        "grant_ref": grant.grant_ref,
        "client_label": grant.client_label,
        "protocol": grant.protocol,
        "operations": grant.operations,
        "advertise": grant.advertise,
        "credential_sha256": grant.credential_sha256,
        "credential_ref": grant.credential_ref,
        "created_at_unix_ms": grant.created_at_unix_ms,
        "expires_at_unix_ms": grant.expires_at_unix_ms,
        "state": grant.state,
        "revoked_at_unix_ms": grant.revoked_at_unix_ms,
        "provenance": grant.provenance,
    }))
}

pub fn decode_grant_value(value: &Value) -> Result<ConnectionGrant> {
    let grant = object(value, "connection grant")?;
    let version = string_field(grant, "version")?;
    if version != CONNECTION_GRANT_WIRE_VERSION {
        return Err(WorkcellError::Unsupported(format!(
            "connection grant version `{version}` is not supported"
        )));
    }
    let state = string_field(grant, "state")?;
    if state != GRANT_STATE_ACTIVE && state != GRANT_STATE_REVOKED {
        return Err(WorkcellError::InvalidDemand(format!(
            "unknown connection grant state `{state}`"
        )));
    }
    Ok(ConnectionGrant {
        grant_ref: string_field(grant, "grant_ref")?.to_owned(),
        client_label: string_field(grant, "client_label")?.to_owned(),
        protocol: string_field(grant, "protocol")?.to_owned(),
        operations: string_array(grant, "operations")?,
        advertise: string_array(grant, "advertise")?,
        credential_sha256: string_field(grant, "credential_sha256")?.to_owned(),
        credential_ref: optional_string_field(grant, "credential_ref")?.map(str::to_owned),
        created_at_unix_ms: u64_field(grant, "created_at_unix_ms")?,
        // Absent decodes as `None`: registries written before grant expiry
        // existed hold grants that never expire.
        expires_at_unix_ms: optional_u64_field_or_absent(grant, "expires_at_unix_ms")?,
        state: state.to_owned(),
        revoked_at_unix_ms: optional_u64_field(grant, "revoked_at_unix_ms")?,
        provenance: string_map_field(grant, "provenance")?,
    })
}

fn state_name(state: &str) -> Result<&'static str> {
    if CONNECTION_STATES.contains(&state) {
        // The slice holds exactly the canonical spellings.
        Ok(CONNECTION_STATES
            .iter()
            .find(|candidate| **candidate == state)
            .expect("checked above"))
    } else {
        Err(WorkcellError::InvalidDemand(format!(
            "unknown connection state `{state}`"
        )))
    }
}

fn object<'a>(value: &'a Value, label: &str) -> Result<&'a Map<String, Value>> {
    value
        .as_object()
        .ok_or_else(|| invalid(format!("{label} must be a JSON object")))
}

fn string_array(map: &Map<String, Value>, key: &str) -> Result<Vec<String>> {
    let values = map
        .get(key)
        .ok_or_else(|| missing(key))?
        .as_array()
        .ok_or_else(|| invalid(format!("field `{key}` must be an array")))?;
    values
        .iter()
        .map(|value| string(value, key).map(str::to_owned))
        .collect()
}

fn string_field<'a>(map: &'a Map<String, Value>, key: &str) -> Result<&'a str> {
    string(map.get(key).ok_or_else(|| missing(key))?, key)
}

fn optional_string_field<'a>(map: &'a Map<String, Value>, key: &str) -> Result<Option<&'a str>> {
    match map.get(key).ok_or_else(|| missing(key))? {
        Value::Null => Ok(None),
        value => string(value, key).map(Some),
    }
}

fn u64_field(map: &Map<String, Value>, key: &str) -> Result<u64> {
    map.get(key)
        .ok_or_else(|| missing(key))?
        .as_u64()
        .ok_or_else(|| invalid(format!("field `{key}` must be an unsigned integer")))
}

fn optional_u64_field(map: &Map<String, Value>, key: &str) -> Result<Option<u64>> {
    match map.get(key).ok_or_else(|| missing(key))? {
        Value::Null => Ok(None),
        value => value
            .as_u64()
            .map(Some)
            .ok_or_else(|| invalid(format!("field `{key}` must be an unsigned integer or null"))),
    }
}

/// Like [`optional_u64_field`], but a field that is absent entirely also
/// decodes as `None`: records and registries written before the field
/// existed must keep decoding, and absence is no expiry.
fn optional_u64_field_or_absent(map: &Map<String, Value>, key: &str) -> Result<Option<u64>> {
    match map.get(key) {
        None => Ok(None),
        Some(Value::Null) => Ok(None),
        Some(value) => value
            .as_u64()
            .map(Some)
            .ok_or_else(|| invalid(format!("field `{key}` must be an unsigned integer or null"))),
    }
}

fn string<'a>(value: &'a Value, label: &str) -> Result<&'a str> {
    value
        .as_str()
        .ok_or_else(|| invalid(format!("{label} must be a JSON string")))
}

fn string_map_field(map: &Map<String, Value>, key: &str) -> Result<BTreeMap<String, String>> {
    let entries = map
        .get(key)
        .ok_or_else(|| missing(key))?
        .as_object()
        .ok_or_else(|| invalid(format!("field `{key}` must be an object")))?;
    entries
        .iter()
        .map(|(name, value)| Ok((name.clone(), string(value, key)?.to_owned())))
        .collect()
}

fn missing(key: &str) -> WorkcellError {
    invalid(format!("connection field `{key}` is missing"))
}

fn invalid(message: String) -> WorkcellError {
    WorkcellError::InvalidDemand(message)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_record() -> ConnectionRecord {
        ConnectionRecord {
            connection_ref: "connection:home-server".into(),
            label: "home-server".into(),
            endpoint: "127.0.0.1:7777".into(),
            protocol: "workcell.control/v1".into(),
            remote_workcell_ref: Some("workcell:home-server".into()),
            remote_software: Some("workcell 0.1.0 (abc123)".into()),
            local_software: "workcell 0.1.0 (def456)".into(),
            granted_operations: vec!["discover".into(), "observe".into()],
            credential_ref: Some("keychain://workcell-connection/home-server".into()),
            state: "connected".into(),
            detail: Some("reconnected; remote software unchanged".into()),
            connected_at_unix_ms: Some(1000),
            expires_at_unix_ms: None,
            last_reconciled_at_unix_ms: 2000,
            provenance: BTreeMap::from([("created_by".into(), "workcell connect".into())]),
        }
    }

    #[test]
    fn connection_record_round_trips_without_translation() {
        let record = sample_record();
        let encoded = encode_connection(&record).unwrap();
        assert!(encoded.contains(CONNECTION_WIRE_VERSION));
        assert_eq!(decode_connection(&encoded).unwrap(), record);
    }

    #[test]
    fn connection_record_never_carries_credential_material() {
        // The schema has no field a credential could hide in: the encode
        // output contains only the credential *reference*.
        let encoded = encode_connection(&sample_record()).unwrap();
        assert!(encoded.contains("keychain://workcell-connection/home-server"));
        assert!(!encoded.contains("wck_"));
    }

    #[test]
    fn unknown_connection_record_version_fails_explicitly() {
        let mut value = connection_value(&sample_record()).unwrap();
        value["version"] = json!("workcell.connection/v99");
        let encoded = serde_json::to_string(&value).unwrap();
        assert!(matches!(
            decode_connection(&encoded),
            Err(WorkcellError::Unsupported(_))
        ));
    }

    #[test]
    fn unknown_connection_state_fails_explicitly() {
        let mut record = sample_record();
        record.state = "kind-of-connected".into();
        assert!(encode_connection(&record).is_err());
        let mut value = connection_value(&sample_record()).unwrap();
        value["state"] = json!("half-open");
        assert!(decode_connection_value(&value).is_err());
    }

    #[test]
    fn grant_round_trips_and_keeps_only_the_credential_digest() {
        let grant = ConnectionGrant {
            grant_ref: "grant:home-client-9a11".into(),
            client_label: "home-client".into(),
            protocol: "workcell.control/v1".into(),
            operations: vec!["status".into(), "discover".into(), "prepare".into()],
            advertise: vec!["service".into()],
            credential_sha256: "64-hex-digest".into(),
            credential_ref: None,
            created_at_unix_ms: 5,
            expires_at_unix_ms: Some(9000),
            state: GRANT_STATE_ACTIVE.into(),
            revoked_at_unix_ms: None,
            provenance: BTreeMap::new(),
        };
        let value = grant_value(&grant).unwrap();
        assert_eq!(decode_grant_value(&value).unwrap(), grant);
        assert!(value.to_string().contains("64-hex-digest"));

        let mut skewed = value.clone();
        skewed["version"] = json!("workcell.connection-grant/v99");
        assert!(matches!(
            decode_grant_value(&skewed),
            Err(WorkcellError::Unsupported(_))
        ));

        let mut revoked_state = value;
        revoked_state["state"] = json!("forgotten");
        assert!(decode_grant_value(&revoked_state).is_err());
    }

    #[test]
    fn expiry_is_carved_into_the_grant_and_never_implied() {
        let mut grant = sample_grant();
        assert!(!grant.is_expired_at(8999));
        assert!(grant.is_expired_at(9000));

        // A grant created without an expiry never expires, at any time.
        grant.expires_at_unix_ms = None;
        assert!(!grant.is_expired_at(0));
        assert!(!grant.is_expired_at(u64::MAX));
        assert_eq!(
            decode_grant_value(&grant_value(&grant).unwrap()).unwrap(),
            grant
        );
    }

    #[test]
    fn records_written_before_expiry_existed_still_decode() {
        // Older registries and receipts carry no `expires_at_unix_ms` field
        // at all; absence decodes as no expiry, never as an error.
        let mut grant = grant_value(&sample_grant()).unwrap();
        grant
            .as_object_mut()
            .unwrap()
            .remove("expires_at_unix_ms")
            .expect("sample grant carries the field");
        assert_eq!(decode_grant_value(&grant).unwrap().expires_at_unix_ms, None);

        let mut record = connection_value(&sample_record()).unwrap();
        record
            .as_object_mut()
            .unwrap()
            .remove("expires_at_unix_ms")
            .expect("sample record carries the field");
        assert_eq!(
            decode_connection_value(&record).unwrap().expires_at_unix_ms,
            None
        );

        // A present-but-non-numeric expiry is still refused loudly.
        grant["expires_at_unix_ms"] = json!("soon");
        assert!(decode_grant_value(&grant).is_err());
    }

    fn sample_grant() -> ConnectionGrant {
        ConnectionGrant {
            grant_ref: "grant:home-client-9a11".into(),
            client_label: "home-client".into(),
            protocol: "workcell.control/v1".into(),
            operations: vec!["status".into()],
            advertise: Vec::new(),
            credential_sha256: "64-hex-digest".into(),
            credential_ref: None,
            created_at_unix_ms: 5,
            expires_at_unix_ms: Some(9000),
            state: GRANT_STATE_ACTIVE.into(),
            revoked_at_unix_ms: None,
            provenance: BTreeMap::new(),
        }
    }
}
