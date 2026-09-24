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

pub const REDIS_NOW_MANAGEMENT_SOURCE: &str =
    "redis/redis:redis.conf + redis.io/docs/latest/operate/oss_and_stack/management/persistence/";
pub const REDIS_NOW_MINIMUM_SERIES: &str = "8.10";

/// Target-owned Redis material for AIKit's hot NOW context.
///
/// Workcell owns only the daemon body and its target-native lifecycle. The
/// logical NOW, participant, source, Factory and disclosure identities remain
/// owned by Central/AIKit/Factory. This helper intentionally accepts only a
/// loopback endpoint: a remotely reachable Redis needs operator-owned ACL/TLS
/// and is declared through the generic external-service path instead of
/// silently widening this reference profile.
///
/// The referenced Redis config is required to carry persistence/resource
/// policy (\`appendonly yes\`, finite \`maxmemory\`, \`maxmemory-policy noeviction\`)
/// and a Workcell-owned data directory. Those are validated by the companion
/// \`redis_now_config_policy\` helper before the service is admitted.
pub fn redis_now_service(
    logical_ref: impl Into<String>,
    host: impl Into<String>,
    port: u16,
    config_path: impl Into<String>,
    acquisition: ExternalServiceAcquisition,
) -> Result<ExternalManagedService> {
    let host = host.into();
    let config_path = config_path.into();
    if !matches!(host.as_str(), "127.0.0.1" | "::1" | "localhost") {
        return Err(WorkcellError::InvalidDemand(
            "the built-in Redis NOW profile is loopback-only; remote Redis requires an explicit generic service declaration with operator-owned ACL/TLS".into(),
        ));
    }
    if port == 0 {
        return Err(WorkcellError::InvalidDemand(
            "Redis NOW port must not be zero".into(),
        ));
    }
    if config_path.trim().is_empty() || config_path.contains('\n') || config_path.contains('\r') {
        return Err(WorkcellError::InvalidDemand(
            "Redis NOW config path must be a non-empty single-line path".into(),
        ));
    }
    let port_text = port.to_string();
    let status = command_owned(
        "redis-cli",
        [
            "--raw",
            "-h",
            host.as_str(),
            "-p",
            port_text.as_str(),
            "PING",
        ],
    )?;
    let start = command_owned("redis-server", [config_path.as_str(), "--daemonize", "yes"])?;
    let stop = command_owned(
        "redis-cli",
        [
            "--raw",
            "-h",
            host.as_str(),
            "-p",
            port_text.as_str(),
            "SHUTDOWN",
        ],
    )?;

    Ok(ExternalManagedService::new(
        logical_ref,
        format!("redis://{host}:{port}"),
        status.clone(),
    )?
    .with_metadata("target", "redis")?
    .with_metadata("target_minimum_series", REDIS_NOW_MINIMUM_SERIES)?
    .with_metadata("target_management_source", REDIS_NOW_MANAGEMENT_SOURCE)?
    .with_metadata("configuration_owner", "workcell")?
    .with_metadata("semantic_state_owner", "central+aikit+factory")?
    .with_metadata("persistence_policy", "aof+operator-rdb")?
    .with_metadata("eviction_policy", "noeviction")?
    .with_metadata("binding", "loopback")?
    .with_readiness(status)
    .with_readiness_timing(5_000, 50)
    .with_start(start)
    .with_stop(stop)
    .with_acquisition(acquisition))
}

/// Render the bounded Redis configuration used by the built-in NOW profile.
/// The caller chooses the data directory and memory bound; Workcell never
/// flushes an existing database and does not own semantic payloads.
pub fn redis_now_config_policy(
    data_dir: impl Into<String>,
    bind: impl Into<String>,
    port: u16,
    maxmemory_bytes: u64,
) -> Result<String> {
    let data_dir = data_dir.into();
    let bind = bind.into();
    if data_dir.trim().is_empty() || data_dir.contains('\n') || data_dir.contains('\r') {
        return Err(WorkcellError::InvalidDemand(
            "Redis NOW data directory must be a non-empty single-line path".into(),
        ));
    }
    if !matches!(bind.as_str(), "127.0.0.1" | "::1") {
        return Err(WorkcellError::InvalidDemand(
            "built-in Redis NOW config must bind loopback only".into(),
        ));
    }
    if port == 0 || maxmemory_bytes < 64 * 1024 * 1024 {
        return Err(WorkcellError::InvalidDemand(
            "Redis NOW requires a non-zero port and at least 64 MiB maxmemory".into(),
        ));
    }
    Ok(format!(
        "bind {bind}\nprotected-mode yes\nport {port}\ndir {data_dir}\nappendonly yes\nappendfsync everysec\nsave 900 1\nsave 300 10\nmaxmemory {maxmemory_bytes}\nmaxmemory-policy noeviction\nstop-writes-on-bgsave-error yes\n"
    ))
}

/// Source pin for the accepted persistent `aikit gateway serve` carrier implementation.
///
/// This remains a target source revision rather than a Workcell-owned protocol
/// version: AIKit owns the Gateway semantics and Workcell materialises that body.
/// Pinned to the revision that resolves occupancy across declared Workcells
/// (EpiLogos/ai-kit#425) and serves with a token location (#433).
pub const AIKIT_GATEWAY_SOURCE_REVISION: &str = "363def309b2ca24a3fdfac2e08a8cfe999d8d631";
pub const AIKIT_GATEWAY_MANAGEMENT_SOURCE: &str =
    "EpiLogos/ai-kit:crates/aikit-cli/src/gateway_ops.rs";
pub const AIKIT_GATEWAY_APPLICATION_PROTOCOL: &str = "aikit.agency-gateway/v1";

/// Target-specific material description for the first-party AIKit Agency Gateway.
///
/// Workcell owns only the service body: process, endpoint, readiness, persistence
/// path and lifecycle. AIKit owns the gateway protocol, AgentSession, Agency,
/// ActuationStream, connector and messaging semantics.
///
/// The body is the `aikit` CLI's `gateway serve`, serving the home's Unix socket
/// (local inbox and turn-boundary delivery) beside the authenticated WebSocket
/// that other Workcells relay through.
///
/// The bearer secret is deliberately NOT accepted by this function.
/// `token_location` names where the token lives — `file:/abs/path` (owner-only)
/// or a `keychain://`, `pass://`, `op://` or `varlock://` reference — and the
/// gateway reads it from there itself. A value that is not a location is refused,
/// so a token pasted in place of its location never reaches a descriptor.
pub fn aikit_gateway_service(
    logical_ref: impl Into<String>,
    host: impl Into<String>,
    port: u16,
    state_file: impl Into<String>,
    token_location: impl Into<String>,
) -> Result<ManagedHostService> {
    let host = host.into();
    let state_file = state_file.into();
    let token_location = token_location.into();
    if state_file.trim().is_empty() {
        return Err(WorkcellError::InvalidDemand(
            "AIKit gateway state file must not be empty".into(),
        ));
    }
    if !valid_token_location(&token_location) {
        return Err(WorkcellError::InvalidDemand(format!(
            "AIKit gateway token location `{}` is not a location: expected `file:/abs/path` or a keychain://, pass://, op:// or varlock:// reference, never the token itself",
            redact_location(&token_location)
        )));
    }
    let readiness = TcpEndpointProbe::new(host.clone(), port)?;
    let bind = format!("{host}:{port}");
    let endpoint = format!("ws://{bind}");

    Ok(ManagedHostService::new(logical_ref, endpoint, "aikit")?
        .with_arg("gateway")
        .with_arg("serve")
        .with_arg("--unix")
        .with_arg("--ws")
        .with_arg(bind)
        .with_arg("--ws-token-location")
        .with_arg(token_location.clone())
        .with_arg("--state-file")
        .with_arg(state_file)
        .with_metadata("target", "aikit-gateway")
        .with_metadata("target_source_revision", AIKIT_GATEWAY_SOURCE_REVISION)
        .with_metadata("target_management_source", AIKIT_GATEWAY_MANAGEMENT_SOURCE)
        .with_metadata("configuration_owner", "aikit")
        .with_metadata("application_protocol", AIKIT_GATEWAY_APPLICATION_PROTOCOL)
        .with_metadata("credential_materialisation", "location-only")
        .with_metadata("credential_location", token_location)
        .with_metadata("semantic_state_owner", "aikit")
        .with_tcp_readiness(readiness))
}

/// A token location, never a token: an absolute `file:` path or a reference in
/// one of the secret stores AIKit resolves, on one line, without whitespace.
fn valid_token_location(location: &str) -> bool {
    if location.is_empty()
        || location
            .chars()
            .any(|c| c.is_whitespace() || c.is_control())
    {
        return false;
    }
    if let Some(path) = location.strip_prefix("file:") {
        return path.starts_with('/') && path.len() > 1;
    }
    ["keychain://", "pass://", "op://", "varlock://"]
        .iter()
        .any(|scheme| location.len() > scheme.len() && location.starts_with(scheme))
}

/// What a refusal may echo of a rejected location: its scheme at most, so a
/// token supplied by mistake is not printed back.
fn redact_location(location: &str) -> String {
    match location.split_once(':') {
        Some((scheme, _)) if !scheme.is_empty() && scheme.len() <= 16 => format!("{scheme}:…"),
        _ => "…".into(),
    }
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

fn command_owned<I, S>(program: &str, args: I) -> Result<ExternalServiceCommand>
where
    I: IntoIterator<Item = S>,
    S: Into<String>,
{
    let mut command = ExternalServiceCommand::new(program)?;
    for arg in args {
        command = command.with_arg(arg.into());
    }
    Ok(command)
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
    fn aikit_gateway_is_an_ordinary_managed_service_with_no_secret_value_in_descriptor() {
        let service = aikit_gateway_service(
            "service:agency-gateway/personal-world",
            "127.0.0.1",
            7778,
            "/var/lib/oi/gateway/state.json",
            "file:/var/lib/oi/gateway/ws.token",
        )
        .unwrap();

        assert_eq!(service.logical_ref, "service:agency-gateway/personal-world");
        assert_eq!(service.endpoint, "ws://127.0.0.1:7778");
        assert_eq!(service.program, "aikit");
        assert_eq!(
            service.args,
            [
                "gateway",
                "serve",
                "--unix",
                "--ws",
                "127.0.0.1:7778",
                "--ws-token-location",
                "file:/var/lib/oi/gateway/ws.token",
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
            service
                .metadata
                .get("credential_materialisation")
                .map(String::as_str),
            Some("location-only")
        );
        assert_eq!(
            service
                .metadata
                .get("credential_location")
                .map(String::as_str),
            Some("file:/var/lib/oi/gateway/ws.token")
        );
        let encoded = format!("{service:?}");
        assert!(!encoded.contains("bot-token"));
        assert!(!encoded.contains("Bearer "));
        assert!(!encoded.contains("secret-value"));
    }

    #[test]
    fn aikit_gateway_rejects_invalid_material_configuration_without_importing_gateway_semantics() {
        let build = |port, state: &str, location: &str| {
            aikit_gateway_service(
                "service:agency-gateway/personal-world",
                "127.0.0.1",
                port,
                state,
                location,
            )
        };
        assert!(build(0, "/var/lib/oi/gateway/state.json", "file:/t").is_err());
        assert!(build(7778, "", "file:/t").is_err());
        // Locations are accepted in every store AIKit resolves.
        for location in [
            "file:/home/me/.aikit/credentials/gateway-ws.token",
            "keychain://aikit/gateway-ws",
            "pass://aikit/gateway-ws",
            "op://vault/aikit/gateway-ws",
            "varlock://aikit/GATEWAY_WS",
        ] {
            assert!(build(7778, "/s.json", location).is_ok(), "{location}");
        }
        // A relative file, an empty ref, a bare environment name or a pasted
        // token is refused, and the refusal never echoes the value.
        for rejected in [
            "file:relative/ws.token",
            "keychain://",
            "AIKIT_GATEWAY_TOKEN",
            "4f2c9a0e1b7d3c5a8e6f0b2d4c6a8e0f",
            "file:/a b",
        ] {
            let error = build(7778, "/s.json", rejected).unwrap_err();
            assert!(!format!("{error:?}").contains("4f2c9a0e"), "{rejected}");
        }
    }

    #[test]
    fn redis_now_is_target_owned_loopback_material_with_explicit_persistence_policy() {
        let service = redis_now_service(
            "service:redis-now/personal-workcell",
            "127.0.0.1",
            6381,
            "/var/lib/oi/redis-now/redis.conf",
            ExternalServiceAcquisition::EnsureRunning,
        )
        .unwrap();
        assert_eq!(service.endpoint, "redis://127.0.0.1:6381");
        assert_eq!(service.status.program, "redis-cli");
        assert_eq!(service.start.as_ref().unwrap().program, "redis-server");
        assert_eq!(
            service.acquisition,
            ExternalServiceAcquisition::EnsureRunning
        );
        assert_eq!(
            service.metadata.get("eviction_policy").map(String::as_str),
            Some("noeviction")
        );
        assert_eq!(
            service
                .metadata
                .get("semantic_state_owner")
                .map(String::as_str),
            Some("central+aikit+factory")
        );

        let rendered =
            redis_now_config_policy("/var/lib/oi/redis-now/data", "127.0.0.1", 6381, 268_435_456)
                .unwrap();
        for required in [
            "appendonly yes",
            "appendfsync everysec",
            "maxmemory 268435456",
            "maxmemory-policy noeviction",
            "protected-mode yes",
        ] {
            assert!(rendered.contains(required), "missing {required}");
        }
        assert!(!rendered.contains("FLUSH"));
    }

    #[test]
    fn built_in_redis_now_profile_refuses_remote_or_unbounded_material() {
        assert!(redis_now_service(
            "service:redis-now/x",
            "10.0.0.4",
            6379,
            "/tmp/redis.conf",
            ExternalServiceAcquisition::ObserveExisting
        )
        .is_err());
        assert!(redis_now_config_policy("/tmp/redis", "0.0.0.0", 6379, 268_435_456).is_err());
        assert!(redis_now_config_policy("/tmp/redis", "127.0.0.1", 6379, 1).is_err());
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
