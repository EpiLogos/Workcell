use std::{io, path::PathBuf, process::Command};

use epilogos_workcell_core::{Result, WorkcellError};

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ArrakisCommand {
    pub program: PathBuf,
    pub args: Vec<String>,
}

impl ArrakisCommand {
    pub fn new(
        program: impl Into<PathBuf>,
        args: impl IntoIterator<Item = impl Into<String>>,
    ) -> Self {
        Self {
            program: program.into(),
            args: args.into_iter().map(Into::into).collect(),
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ArrakisCommandOutput {
    pub stdout: String,
    pub stderr: String,
}

pub trait ArrakisCommandRunner: Send + Sync {
    fn run(&self, command: &ArrakisCommand) -> Result<ArrakisCommandOutput>;
}

#[derive(Clone, Copy, Debug, Default)]
pub struct SystemArrakisCommandRunner;

impl ArrakisCommandRunner for SystemArrakisCommandRunner {
    fn run(&self, command: &ArrakisCommand) -> Result<ArrakisCommandOutput> {
        let output = Command::new(&command.program)
            .args(&command.args)
            .output()
            .map_err(|error| match error.kind() {
                io::ErrorKind::NotFound => WorkcellError::Unavailable(format!(
                    "Arrakis client `{}` is not available",
                    command.program.display()
                )),
                _ => WorkcellError::OperationFailed(format!(
                    "failed to launch Arrakis client `{}`: {error}",
                    command.program.display()
                )),
            })?;

        let stdout = String::from_utf8_lossy(&output.stdout).into_owned();
        let stderr = String::from_utf8_lossy(&output.stderr).into_owned();
        if !output.status.success() {
            let detail = if stderr.trim().is_empty() {
                stdout.trim()
            } else {
                stderr.trim()
            };
            if detail.contains("HTTP 404") || detail.contains("VM not found") {
                return Err(WorkcellError::NotFound(detail.into()));
            }
            return Err(WorkcellError::OperationFailed(format!(
                "Arrakis client command failed ({}): {detail}",
                command.args.join(" ")
            )));
        }

        Ok(ArrakisCommandOutput { stdout, stderr })
    }
}

pub trait ArrakisHostProbe: Send + Sync {
    fn local_kvm_available(&self) -> bool;
}

#[derive(Clone, Copy, Debug, Default)]
pub struct SystemArrakisHostProbe;

impl ArrakisHostProbe for SystemArrakisHostProbe {
    fn local_kvm_available(&self) -> bool {
        cfg!(target_os = "linux") && std::path::Path::new("/dev/kvm").exists()
    }
}
