use epilogos_workcell_core::{Result, WorkcellError};

use crate::{
    ExternalManagedService, ExternalServiceAcquisition, ExternalServiceCommand, ManagedHostService,
    TcpEndpointProbe,
};

pub const HERMES_SOURCE_REVISION: &str = "036cbdfa0a3158454a0a2a7a7388cf70353326b4";
pub const HERMES_MANAGEMENT_SOURCE: &str =
    "hermes-agent-org/hermes:website/docs/reference/cli-commands.md";
pub const OPENCLAW_SOURCE_REVISION: &str = "9d4ba33c4a6e5e8386829e1c0010b280983599c5";
pub const OPENCLAW_MANAGEMENT_SOURCE: &str = "openclaw/openclaw:docs/cli/gateway.md";

/// Source pin for the accepted persistent `aikit-gateway serve` carrier implementation.
///
/// This remains a target source revision rather than a Workcell-owned protocol
/// version: AIKit owns the Gateway semantics and Workcell materialises that body.
pub const AIKIT_GATEWAY_SOURCE_REVISION: &str = "4b614a732090df3abda7940d2fede649fc218492";
pub const AIKIT_GATEWAY_MANAGEMENT_SOURCE: &str =
    "EpiLogos/ai-kit:crates/aikit-adapters/src/bin/aikit-gateway.rs";
pub const AIKIT_GATEWAY_APPLICATION_PROTOCOL: &str = "aikit.agency-gateway/v1";

/// Target-specific material description for the first-party AIKit Agency Gateway.
///
/// Workcell owns only the service body: process, endpoint, readiness, persistence
/// path and lifecycle. AIKit owns the gateway protocol, AgentSession, Agency,
/// ActuationStream, connector and messaging semantics.
///
/// The bearer secret is deliberately NOT accepted by this function. `token_env`
/// is only the name of the environment variable from which the already-materialised
/// gateway process reads its token. Workcell's existing secret layer remains the
/// place that may materialise the actual value into the child environment.
pub fn aikit_gateway_service(
    logical_ref: impl Into<String>,
    host: impl Into<String>,
    port: u16,
    state_file: impl Into<String>,
    token_env: impl Into<String>,
) -> Result<ManagedHostService> {
    let host = host.into();
    let state_file = state_file.into();
    let token_env = token_env.into();
    if state_file.trim().is_empty() {
        return Err(WorkcellError::InvalidDemand(
            "AIKit gateway state file must not be empty".into(),
        ));
    }
    if !valid_env_name(&token_env) {
        return Err(WorkcellError::InvalidDemand(format!(
            "AIKit gateway token environment name `{token_env}` is invalid"
        )));
    }
    let readiness = TcpEndpointProbe::new(host.clone(), port)?;
    let bind = format!("{host}:{port}");
    let endpoint = format!("ws://{bind}");

    Ok(
        ManagedHostService::new(logical_ref, endpoint, "aikit-gateway")?
            .with_arg("serve")
            .with_arg("--ws")
            .with_arg(bind)
            .with_arg("--token-env")
            .with_arg(token_env.clone())
            .with_arg("--state-file")
            .with_arg(state_file)
            .with_metadata("target", "aikit-gateway")
            .with_metadata("target_source_revision", AIKIT_GATEWAY_SOURCE_REVISION)
            .with_metadata("target_management_source", AIKIT_GATEWAY_MANAGEMENT_SOURCE)
            .with_metadata("configuration_owner", "aikit")
            .with_metadata("application_protocol", AIKIT_GATEWAY_APPLICATION_PROTOCOL)
            .with_metadata("credential_materialisation", "environment-name-only")
            .with_metadata("credential_env", token_env)
            .with_metadata("semantic_state_owner", "aikit")
            .with_tcp_readiness(readiness),
    )
}

/// Target-specific material management description for a Hermes gateway.
///
/// This describes only Hermes' published process/service lifecycle. Hermes
/// profile, channel, model, tool, session and messaging semantics stay owned by
/// Hermes/AIKit. `endpoint` is deployment-supplied because Hermes can expose
/// different target-native communication surfaces.
pub fn hermes_gateway_service(
    logical_ref: impl Into<String>,
    endpoint: impl Into<String>,
    acquisition: ExternalServiceAcquisition,
) -> Result<ExternalManagedService> {
    let start = command("hermes", &["gateway", "start"])?;
    let stop = command("hermes", &["gateway", "stop"])?;
    let restart = command("hermes", &["gateway", "restart"])?;

    Ok(ExternalManagedService::new(
        logical_ref,
        endpoint,
        command("hermes", &["gateway", "status"])?,
    )?
    .with_metadata("target", "hermes")?
    .with_metadata("target_source_revision", HERMES_SOURCE_REVISION)?
    .with_metadata("target_management_source", HERMES_MANAGEMENT_SOURCE)?
    .with_metadata("configuration_owner", "hermes")?
    .with_metadata("application_protocol", "opaque-to-workcell")?
    .with_start(start)
    .with_stop(stop)
    .with_restart(restart)
    .with_acquisition(acquisition))
}

/// Target-specific material management description for an OpenClaw Gateway.
///
/// `openclaw gateway health` is kept separate from service-manager `status`:
/// status answers installed/running service state while health exercises the
/// Gateway's own liveness path. Workcell records both as material observation;
/// it does not consume OpenClaw WebSocket/session semantics.
pub fn openclaw_gateway_service(
    logical_ref: impl Into<String>,
    endpoint: impl Into<String>,
    acquisition: ExternalServiceAcquisition,
) -> Result<ExternalManagedService> {
    let readiness = command("openclaw", &["gateway", "health"])?;
    let start = command("openclaw", &["gateway", "start"])?;
    let stop = command("openclaw", &["gateway", "stop"])?;
    let restart = command("openclaw", &["gateway", "restart"])?;

    Ok(ExternalManagedService::new(
        logical_ref,
        endpoint,
        command("openclaw", &["gateway", "status"])?,
    )?
    .with_metadata("target", "openclaw")?
    .with_metadata("target_source_revision", OPENCLAW_SOURCE_REVISION)?
    .with_metadata("target_management_source", OPENCLAW_MANAGEMENT_SOURCE)?
    .with_metadata("configuration_owner", "openclaw")?
    .with_metadata("application_protocol", "websocket-owned-by-openclaw")?
    .with_readiness(readiness)
    .with_start(start)
    .with_stop(stop)
    .with_restart(restart)
    .with_acquisition(acquisition))
}

fn command(program: &str, args: &[&str]) -> Result<ExternalServiceCommand> {
    let mut command = ExternalServiceCommand::new(program)?;
    for arg in args {
        command = command.with_arg(*arg);
    }
    Ok(command)
}

fn valid_env_name(value: &str) -> bool {
    let mut chars = value.chars();
    let Some(first) = chars.next() else {
        return false;
    };
    (first == '_' || first.is_ascii_alphabetic())
        && chars.all(|character| character == '_' || character.is_ascii_alphanumeric())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn aikit_gateway_is_an_ordinary_managed_service_with_no_secret_value_in_descriptor() {
        let service = aikit_gateway_service(
            "service:agency-gateway/personal-world",
            "127.0.0.1",
            7778,
            "/var/lib/oi/gateway/state.json",
            "AIKIT_GATEWAY_TOKEN",
        )
        .unwrap();

        assert_eq!(service.logical_ref, "service:agency-gateway/personal-world");
        assert_eq!(service.endpoint, "ws://127.0.0.1:7778");
        assert_eq!(service.program, "aikit-gateway");
        assert_eq!(
            service.args,
            [
                "serve",
                "--ws",
                "127.0.0.1:7778",
                "--token-env",
                "AIKIT_GATEWAY_TOKEN",
                "--state-file",
                "/var/lib/oi/gateway/state.json",
            ]
        );
        let readiness = service.readiness.as_ref().unwrap();
        assert_eq!(readiness.host, "127.0.0.1");
        assert_eq!(readiness.port, 7778);
        assert_eq!(
            service.metadata.get("target").map(String::as_str),
            Some("aikit-gateway")
        );
        assert_eq!(
            service
                .metadata
                .get("application_protocol")
                .map(String::as_str),
            Some(AIKIT_GATEWAY_APPLICATION_PROTOCOL)
        );
        assert_eq!(
            service.metadata.get("credential_env").map(String::as_str),
            Some("AIKIT_GATEWAY_TOKEN")
        );
        let encoded = format!("{service:?}");
        assert!(!encoded.contains("bot-token"));
        assert!(!encoded.contains("Bearer "));
        assert!(!encoded.contains("secret-value"));
    }

    #[test]
    fn aikit_gateway_rejects_invalid_material_configuration_without_importing_gateway_semantics() {
        assert!(aikit_gateway_service(
            "service:agency-gateway/personal-world",
            "127.0.0.1",
            0,
            "/var/lib/oi/gateway/state.json",
            "AIKIT_GATEWAY_TOKEN",
        )
        .is_err());
        assert!(aikit_gateway_service(
            "service:agency-gateway/personal-world",
            "127.0.0.1",
            7778,
            "",
            "AIKIT_GATEWAY_TOKEN",
        )
        .is_err());
        assert!(aikit_gateway_service(
            "service:agency-gateway/personal-world",
            "127.0.0.1",
            7778,
            "/var/lib/oi/gateway/state.json",
            "AIKIT-GATEWAY-TOKEN",
        )
        .is_err());
    }

    #[test]
    fn reference_targets_remain_distinct_target_native_management_surfaces() {
        let hermes = hermes_gateway_service(
            "service:assistant-gateway",
            "target-native://hermes/default",
            ExternalServiceAcquisition::ObserveExisting,
        )
        .unwrap();
        let openclaw = openclaw_gateway_service(
            "service:assistant-gateway",
            "ws://127.0.0.1:18789",
            ExternalServiceAcquisition::ObserveExisting,
        )
        .unwrap();

        assert_eq!(hermes.logical_ref, openclaw.logical_ref);
        assert_eq!(hermes.status.program, "hermes");
        assert_eq!(hermes.status.args, ["gateway", "status"]);
        assert!(hermes.readiness.is_none());
        assert_eq!(
            hermes
                .metadata
                .get("target_source_revision")
                .map(String::as_str),
            Some(HERMES_SOURCE_REVISION)
        );

        assert_eq!(openclaw.status.program, "openclaw");
        assert_eq!(openclaw.status.args, ["gateway", "status"]);
        assert_eq!(
            openclaw.readiness.as_ref().unwrap().args,
            ["gateway", "health"]
        );
        assert_eq!(
            openclaw
                .metadata
                .get("target_source_revision")
                .map(String::as_str),
            Some(OPENCLAW_SOURCE_REVISION)
        );

        assert_ne!(hermes.endpoint, openclaw.endpoint);
        assert_ne!(
            hermes.metadata.get("application_protocol"),
            openclaw.metadata.get("application_protocol")
        );
    }
}
