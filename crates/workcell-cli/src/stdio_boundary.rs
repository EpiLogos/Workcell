//! Protocol-preserving entry into the existing material write boundary.
//! This replaces this launcher, not the protocol host or its session identity.
use epilogos_workcell_runtime::{PreparedWriteBoundary, WriteBoundaryRequirements};
use std::{fs, process::Command};

pub const USAGE: &str =
    "exec REQUIREMENTS.json CURRENT_POLICY_REVISION EXPECTED_DIGEST -- PROGRAM [ARG...]";

pub fn execute(args: &[String]) -> Result<(), Box<dyn std::error::Error>> {
    if args.len() < 6 || args[4] != "--" {
        return Err(USAGE.into());
    }
    if fs::metadata(&args[1])?.len() > 1_048_576 {
        return Err("requirements exceed 1 MiB".into());
    }
    let requirements = WriteBoundaryRequirements::from_json(&fs::read_to_string(&args[1])?)?;
    if requirements.digest() != args[3] {
        return Err(
            "write boundary changed since native preparation; re-resolve before launch".into(),
        );
    }
    let boundary = PreparedWriteBoundary::prepare(requirements, &args[2])?;
    let mut command = Command::new(&args[5]);
    command.args(&args[6..]);
    boundary.configure_command(&mut command, &args[2])?;
    protocol_exec(command)
}

#[cfg(target_os = "linux")]
fn protocol_exec(mut command: Command) -> Result<(), Box<dyn std::error::Error>> {
    use std::os::unix::{fs::FileTypeExt, process::CommandExt};
    use std::process::Stdio;
    // A pre-opened regular stdout could otherwise bypass pathname protection.
    // The protocol host supplies pipes/sockets; files and terminals are refused.
    for fd in [0, 1] {
        let kind = fs::metadata(format!("/proc/self/fd/{fd}"))?.file_type();
        if !kind.is_fifo() && !kind.is_socket() {
            return Err(
                "protocol exec requires pipe/socket stdin and stdout, not inherited files".into(),
            );
        }
    }
    // The existing ruleset's pre-exec hook closes all other inherited handles
    // on exec. Provider diagnostics are deliberately not an unrestricted fd 2.
    // Launcher refusals go to stderr; successful stdout is ONLY provider bytes.
    command
        .stdin(Stdio::inherit())
        .stdout(Stdio::inherit())
        .stderr(Stdio::null());
    Err(command.exec().into())
}

#[cfg(not(target_os = "linux"))]
fn protocol_exec(_command: Command) -> Result<(), Box<dyn std::error::Error>> {
    Err("protocol write-boundary exec is unavailable on this platform".into())
}
