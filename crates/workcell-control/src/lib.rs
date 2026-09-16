mod client;
pub mod codec;
pub mod grants;
pub mod machines;
mod network;
pub mod projections;
mod service;

pub use client::{
    ControlClient, ControlClientError, ControlTransport, DirectTransport, LengthPrefixedTransport,
    TransportFailure, UnavailableTransport,
};
use epilogos_workcell_core::WorkcellError;
pub use grants::{
    credential_sha256, generate_credential, grant_ref_for, parse_duration_millis, validate_label,
    ConnectionGrants, CreateOutcome, GrantDecision, CONNECTIONS_DIRECTORY, GRANTS_FILE,
};
pub use machines::{
    RemoteMachineDeclaration, RemoteMachineRegistry, MACHINE_CREDENTIAL_SCHEMES, MACHINES_FILE,
    MACHINES_SCHEMA,
};
pub use network::{TcpControlServer, TcpControlTransport};
pub use projections::{
    ProjectionDecision, SecretProjectionLedger, SecretProjectionRecord, PROJECTIONS_FILE,
    PROJECTIONS_SCHEMA, PROJECTION_STATE_ACTIVE, PROJECTION_STATE_REVOKED, SECRETS_DIRECTORY,
};
pub use service::ControlService;

pub const CONTROL_PROTOCOL_VERSION: &str = "workcell.control/v1";

/// The control operations a grant can permit. `connection.handshake` is not
/// in this list: it is the compatibility disclosure every client may attempt,
/// and it discloses no capability and no workcell identity.
pub const CONTROL_OPERATIONS: [&str; 11] = [
    "status",
    "discover",
    "plan",
    "prepare",
    "inspect",
    "recover",
    "observe",
    "expose",
    "collect",
    "release",
    "reconcile",
];

pub const CONNECTION_HANDSHAKE_OPERATION: &str = "connection.handshake";

/// The software disclosure carried in every connection handshake. Protocol
/// compatibility is decided by [`CONTROL_PROTOCOL_VERSION`]; the software
/// revision is provenance that is reported, never refused on and never a
/// trigger to update the other cell.
pub fn software_version() -> String {
    format!(
        "workcell {} ({})",
        env!("CARGO_PKG_VERSION"),
        option_env!("SUITE_BUILD_REVISION").unwrap_or("unknown")
    )
}

/// What two cells disclosed about each other during a handshake.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CompatibilityReport {
    pub client_protocol: String,
    pub server_protocol: String,
    pub client_software: String,
    pub server_software: String,
    pub compatible: bool,
    pub notes: Vec<String>,
}

/// Decide whether this client can speak to what the remote disclosed.
///
/// The protocol version is the compatibility contract: a different protocol
/// refuses loudly. A different software revision inside the same protocol is
/// a reported note, never a refusal and never permission to update the
/// remote (or this client) silently.
pub fn check_compatibility(
    server_protocol: &str,
    server_software: &str,
) -> Result<CompatibilityReport, WorkcellError> {
    let client_software = software_version();
    let compatible = server_protocol == CONTROL_PROTOCOL_VERSION;
    let mut notes = Vec::new();
    if !compatible {
        return Err(WorkcellError::Unsupported(format!(
            "connection refused: remote control protocol `{server_protocol}` is not supported by this cell (`{CONTROL_PROTOCOL_VERSION}`); the two cells are incompatible"
        )));
    }
    if server_software != client_software {
        notes.push(format!(
            "software revisions differ (local `{client_software}`, remote `{server_software}`); protocol {CONTROL_PROTOCOL_VERSION} is the compatibility contract and nothing was updated"
        ));
    }
    Ok(CompatibilityReport {
        client_protocol: CONTROL_PROTOCOL_VERSION.to_owned(),
        server_protocol: server_protocol.to_owned(),
        client_software,
        server_software: server_software.to_owned(),
        compatible: true,
        notes,
    })
}

#[cfg(test)]
mod compatibility_tests {
    use super::*;

    #[test]
    fn same_protocol_is_compatible_and_reports_software_differences() {
        let report =
            check_compatibility(CONTROL_PROTOCOL_VERSION, "workcell 0.1.0 (older)").unwrap();
        assert!(report.compatible);
        assert_eq!(report.server_protocol, CONTROL_PROTOCOL_VERSION);
        assert_eq!(report.notes.len(), 1);
        assert!(report.notes[0].contains("nothing was updated"));
    }

    #[test]
    fn identical_disclosure_has_no_notes() {
        let report = check_compatibility(CONTROL_PROTOCOL_VERSION, &software_version()).unwrap();
        assert!(report.compatible);
        assert!(report.notes.is_empty());
    }

    #[test]
    fn different_protocol_refuses_loudly() {
        let error = check_compatibility("workcell.control/v2", "workcell 9.9").unwrap_err();
        assert!(matches!(error, WorkcellError::Unsupported(_)));
        let message = error.to_string();
        assert!(message.contains("workcell.control/v2"));
        assert!(message.contains("refused"));
    }
}
