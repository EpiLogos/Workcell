//! Protocol-preserving entry into the existing material write boundary.
//! This replaces this launcher, not the protocol host or its session identity.
use epilogos_workcell_runtime::{PreparedWriteBoundary, RuntimeProjection, WriteBoundaryRequirements};
use serde_json::Value;
use std::{fs, process::Command};

pub const USAGE: &str =
    "exec REQUIREMENTS_OR_PREPARATION.json CURRENT_POLICY_REVISION EXPECTED_DIGEST -- PROGRAM [ARG...]";

pub const RUNTIME_USAGE: &str = "exec-runtime REQUIREMENTS_OR_PREPARATION.json CURRENT_POLICY_REVISION EXPECTED_DIGEST PROJECTION.json EXPECTED_PROJECTION_DIGEST -- PROGRAM [ARG...]";

pub fn execute(args: &[String]) -> Result<(), Box<dyn std::error::Error>> {
    let runtime = args.first().map(String::as_str) == Some("exec-runtime");
    let delimiter = if runtime { 6 } else { 4 };
    if args.len() < delimiter + 2 || args[delimiter] != "--" {
        return Err(if runtime { RUNTIME_USAGE } else { USAGE }.into());
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
    if runtime { boundary.validate_protocol_stdio()?; }
    let mut command = if runtime {
        let projection = RuntimeProjection::read(std::path::Path::new(&args[4]), &args[5])?;
        boundary.command_with_runtime_projection(&args[delimiter + 1], &args[2], &projection)?
    } else { boundary.command(&args[delimiter + 1], &args[2])? };
    command.args(&args[delimiter + 2..]);
    if runtime {
        boundary.configure_protocol_stdio(&mut command)
            .map_err(epilogos_workcell_runtime::RuntimeProjectionFailure::after_boundary_error)?;
    } else { boundary.configure_protocol_stdio(&mut command)?; }
    if runtime {
        protocol_exec(command).map_err(|error| match error.downcast::<std::io::Error>() {
            Ok(cause) => Box::new(epilogos_workcell_runtime::RuntimeProjectionFailure::after_exec_error(*cause)) as Box<dyn std::error::Error>,
            Err(error) => error,
        })
    } else { protocol_exec(command) }
}

#[cfg(any(target_os = "linux", target_os = "macos"))]
fn protocol_exec(mut command: Command) -> Result<(), Box<dyn std::error::Error>> {
    use std::os::unix::process::CommandExt;
    // The boundary selects only an inspected pipe/socket diagnostic channel,
    // otherwise the legacy null sink. Its hook checks all stdio after remapping
    // and closes other inherited handles. Stdout stays ONLY provider bytes.
    Err(command.exec().into())
}

#[cfg(not(any(target_os = "linux", target_os = "macos")))]
fn protocol_exec(_command: Command) -> Result<(), Box<dyn std::error::Error>> {
    Err("protocol write-boundary exec is unavailable on this platform".into())
}
