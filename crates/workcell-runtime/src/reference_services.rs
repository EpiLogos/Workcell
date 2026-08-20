use epilogos_workcell_core::Result;

use crate::{ExternalManagedService, ExternalServiceAcquisition, ExternalServiceCommand};

pub const HERMES_SOURCE_REVISION: &str = "036cbdfa0a3158454a0a2a7a7388cf70353326b4";
pub const HERMES_MANAGEMENT_SOURCE: &str =
    "hermes-agent-org/hermes:website/docs/reference/cli-commands.md";
pub const OPENCLAW_SOURCE_REVISION: &str = "9d4ba33c4a6e5e8386829e1c0010b280983599c5";
pub const OPENCLAW_MANAGEMENT_SOURCE: &str = "openclaw/openclaw:docs/cli/gateway.md";

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

#[cfg(test)]
mod tests {
    use super::*;

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
