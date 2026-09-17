//! Live-state probe for a staged share, run over an existing SSH alias.
//!
//! Decision (recorded where it governs): this is evidence gathering for a
//! human surface — the same class of probe as `workcell-tailscale` shelling
//! out to `tailscale status --json` — never a second control API. The control
//! plane remains `workcell.control/v1` (`docs/CONNECTIVITY-FABRIC.md`: SSH
//! must not become an implicit replacement API). It also stays separate from
//! [`crate::ServiceProvider::observe_service`], which reports disk truth;
//! this module reports the machine's live listening state.
//!
//! The state names carry the runbook's law: `LoopbackOnly` is the silent
//! failure mode of interface binding on a /32 Tailscale peer — smb active,
//! 445 bound to loopback alone, every remote connection refused with no error
//! at service start.

use std::io;
use std::process::Command;

use epilogos_workcell_core::HealthState;

/// What the target machine is actually doing with port 445.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum LiveShareState {
    /// Bound on a wildcard address: reachable from the tailnet.
    Served,
    /// smb active but bound to loopback alone — the interface-binding trap.
    LoopbackOnly,
    /// Nothing listening on 445: service down or never installed.
    NotListening,
    /// The probe could not reach the host at all; the reason travels with it.
    Unreachable(String),
}

impl LiveShareState {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Served => "served",
            Self::LoopbackOnly => "loopback-only",
            Self::NotListening => "not-listening",
            Self::Unreachable(_) => "unreachable",
        }
    }

    pub fn health(&self) -> HealthState {
        match self {
            Self::Served => HealthState::Healthy,
            Self::LoopbackOnly | Self::NotListening => HealthState::Degraded,
            Self::Unreachable(_) => HealthState::Unavailable,
        }
    }
}

/// How the probe reaches the target. A trait so tests can stand in for SSH.
pub trait ProbeTransport {
    /// Run `command` on `alias` and return stdout; an error means the host
    /// was not reachable through the transport.
    fn run(&self, alias: &str, command: &str) -> io::Result<String>;
}

/// The real transport: key-authenticated SSH, non-interactive, short timeout.
pub struct SshProbeTransport;

impl ProbeTransport for SshProbeTransport {
    fn run(&self, alias: &str, command: &str) -> io::Result<String> {
        let output = Command::new("ssh")
            .args([
                "-o",
                "BatchMode=yes",
                "-o",
                "ConnectTimeout=8",
                alias,
                "--",
                command,
            ])
            .output()?;
        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(io::Error::other(format!(
                "ssh {alias} failed: {}",
                stderr.trim()
            )));
        }
        Ok(String::from_utf8_lossy(&output.stdout).into_owned())
    }
}

/// Probe a target machine's live SMB listening state over `alias`.
pub fn probe_live_share(transport: &impl ProbeTransport, alias: &str) -> LiveShareState {
    match transport.run(alias, "ss -tln") {
        Ok(output) => classify_listen_output(&output),
        Err(error) => LiveShareState::Unreachable(error.to_string()),
    }
}

/// Classify `ss -tln` output. Any LISTEN line whose local address ends `:445`
/// decides the state: loopback addresses mean the trap, anything else means
/// the share is being served. No 445 listener means not listening.
pub fn classify_listen_output(output: &str) -> LiveShareState {
    for line in output.lines() {
        if !line.contains("LISTEN") {
            continue;
        }
        for token in line.split_whitespace() {
            if let Some(host) = token.strip_suffix(":445") {
                return if host == "127.0.0.1" || host == "[::1]" {
                    LiveShareState::LoopbackOnly
                } else {
                    LiveShareState::Served
                };
            }
        }
    }
    LiveShareState::NotListening
}

#[cfg(test)]
mod tests {
    use super::*;

    struct FixedTransport(&'static str);

    impl ProbeTransport for FixedTransport {
        fn run(&self, _alias: &str, _command: &str) -> io::Result<String> {
            Ok(self.0.to_string())
        }
    }

    struct BrokenTransport;

    impl ProbeTransport for BrokenTransport {
        fn run(&self, alias: &str, _command: &str) -> io::Result<String> {
            Err(io::Error::other(format!(
                "ssh {alias} failed: connection refused"
            )))
        }
    }

    #[test]
    fn the_proven_loopback_trap_is_named() {
        // The exact LISTEN line from the 2026-09-16 session: smb active,
        // every remote connection refused, no error at start-up.
        let output = "State  Recv-Q Send-Q  Local Address:Port  Peer Address:Port\n\
                      LISTEN 0      50          127.0.0.1:445        0.0.0.0:*\n";
        assert_eq!(classify_listen_output(output), LiveShareState::LoopbackOnly);
    }

    #[test]
    fn wildcard_and_ipv6_listens_mean_served() {
        assert_eq!(
            classify_listen_output("LISTEN 0 50 0.0.0.0:445 0.0.0.0:*"),
            LiveShareState::Served
        );
        assert_eq!(
            classify_listen_output("LISTEN 0 50 *:445 *:*"),
            LiveShareState::Served
        );
        assert_eq!(
            classify_listen_output("LISTEN 0 50 [::]:445 [::]:*"),
            LiveShareState::Served
        );
        // Non-listening rows never decide: a connected peer row is not a serve.
        let output = "ESTAB 0 0 100.92.62.101:445 100.109.102.82:52810\n";
        assert_eq!(classify_listen_output(output), LiveShareState::NotListening);
    }

    #[test]
    fn ipv6_loopback_is_the_same_trap_and_absence_is_named() {
        assert_eq!(
            classify_listen_output("LISTEN 0 50 [::1]:445 [::]:*"),
            LiveShareState::LoopbackOnly
        );
        assert_eq!(
            classify_listen_output("LISTEN 0 50 0.0.0.0:22 0.0.0.0:*"),
            LiveShareState::NotListening
        );
        assert_eq!(classify_listen_output(""), LiveShareState::NotListening);
    }

    #[test]
    fn probe_reports_states_and_carries_unreach_reason() {
        let served = probe_live_share(
            &FixedTransport("LISTEN 0 50 0.0.0.0:445 0.0.0.0:*"),
            "frank",
        );
        assert_eq!(served, LiveShareState::Served);
        assert_eq!(served.health(), HealthState::Healthy);

        let trapped = probe_live_share(
            &FixedTransport("LISTEN 0 50 127.0.0.1:445 0.0.0.0:*"),
            "frank",
        );
        assert_eq!(trapped.health(), HealthState::Degraded);

        let unreachable = probe_live_share(&BrokenTransport, "ghost");
        match &unreachable {
            LiveShareState::Unreachable(reason) => {
                assert!(reason.contains("ghost"), "reason names the host: {reason}");
            }
            other => panic!("expected unreachable, got {other:?}"),
        }
        assert_eq!(unreachable.health(), HealthState::Unavailable);
    }
}
