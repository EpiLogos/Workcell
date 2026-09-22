//! Protocol-preserving entry into the existing material write boundary.
//! This replaces this launcher, not the protocol host or its session identity.
use epilogos_workcell_runtime::{PreparedWriteBoundary, WriteBoundaryRequirements};
use serde_json::Value;
use std::{fs, process::Command};

pub const USAGE: &str =
    "exec REQUIREMENTS_OR_PREPARATION.json CURRENT_POLICY_REVISION EXPECTED_DIGEST -- PROGRAM [ARG...]";

pub fn execute(args: &[String]) -> Result<(), Box<dyn std::error::Error>> {
    if args.len() < 6 || args[4] != "--" {
        return Err(USAGE.into());
    }
    if fs::metadata(&args[1])?.len() > 1_048_576 {
        return Err("requirements exceed 1 MiB".into());
    }
    let input: Value = serde_json::from_str(&fs::read_to_string(&args[1])?)?;
    let prepared = input["schema"] == "workcell.prepared-write-boundary/v1";
    let requirements = if prepared {
        if input["state"] != "prepared-not-executed" {
            return Err("expected the exact native prepared boundary reading".into());
        }
        WriteBoundaryRequirements::from_json(&input["requirements"].to_string())?
    } else {
        WriteBoundaryRequirements::from_json(&input.to_string())?
    };
    if requirements.digest() != args[3] {
        return Err(
            "write boundary changed since native preparation; re-resolve before launch".into(),
        );
    }
    let boundary = PreparedWriteBoundary::prepare(requirements, &args[2])?;
    if prepared {
        let current = boundary.inspect(&args[2])?;
        for field in ["requirements_digest", "objects", "protected_objects"] {
            if input[field] != current[field] {
                return Err(format!(
                    "prepared protocol {field} changed; re-resolve before execution"
                )
                .into());
            }
        }
    }
    boundary.validate_protocol_stdio()?;
    let mut command = boundary.command(&args[5], &args[2])?;
    command.args(&args[6..]);
    protocol_exec(command)
}

#[cfg(any(target_os = "linux", target_os = "macos"))]
fn protocol_exec(mut command: Command) -> Result<(), Box<dyn std::error::Error>> {
    use std::os::unix::process::CommandExt;
    use std::process::Stdio;
    // The existing ruleset's pre-exec hook closes all other inherited handles
    // on exec. Provider diagnostics are deliberately not an unrestricted fd 2.
    // Launcher refusals go to stderr; successful stdout is ONLY provider bytes.
    command
        .stdin(Stdio::inherit())
        .stdout(Stdio::inherit())
        .stderr(Stdio::null());
    Err(command.exec().into())
}

#[cfg(not(any(target_os = "linux", target_os = "macos")))]
fn protocol_exec(_command: Command) -> Result<(), Box<dyn std::error::Error>> {
    Err("protocol write-boundary exec is unavailable on this platform".into())
}
