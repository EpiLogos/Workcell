use std::{collections::BTreeMap, io, path::PathBuf, process::Command};

use epilogos_workcell_core::{Result, WorkcellError};

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DockerCommand {
    pub args: Vec<String>,
    pub cwd: Option<PathBuf>,
    pub env: BTreeMap<String, String>,
}

impl DockerCommand {
    pub fn new(args: impl IntoIterator<Item = impl Into<String>>) -> Self {
        Self {
            args: args.into_iter().map(Into::into).collect(),
            cwd: None,
            env: BTreeMap::new(),
        }
    }

    pub fn with_cwd(mut self, cwd: impl Into<PathBuf>) -> Self {
        self.cwd = Some(cwd.into());
        self
    }

    pub fn with_env(mut self, key: impl Into<String>, value: impl Into<String>) -> Self {
        self.env.insert(key.into(), value.into());
        self
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DockerCommandOutput {
    pub stdout: String,
    pub stderr: String,
}

impl DockerCommandOutput {
    pub fn empty() -> Self {
        Self {
            stdout: String::new(),
            stderr: String::new(),
        }
    }
}

pub trait DockerCommandRunner: Send + Sync {
    fn run(&self, command: &DockerCommand) -> Result<DockerCommandOutput>;
}

#[derive(Clone, Copy, Debug, Default)]
pub struct SystemDockerCommandRunner;

impl DockerCommandRunner for SystemDockerCommandRunner {
    fn run(&self, command: &DockerCommand) -> Result<DockerCommandOutput> {
        let mut process = Command::new("docker");
        process.args(&command.args);
        if let Some(cwd) = &command.cwd {
            process.current_dir(cwd);
        }
        process.envs(&command.env);

        let output = process.output().map_err(|error| match error.kind() {
            io::ErrorKind::NotFound => {
                WorkcellError::Unavailable("Docker CLI executable `docker` is not available".into())
            }
            _ => WorkcellError::OperationFailed(format!("failed to launch Docker CLI: {error}")),
        })?;

        let stdout = String::from_utf8_lossy(&output.stdout).into_owned();
        let stderr = String::from_utf8_lossy(&output.stderr).into_owned();
        if !output.status.success() {
            let detail = if stderr.trim().is_empty() {
                stdout.trim()
            } else {
                stderr.trim()
            };
            return Err(WorkcellError::OperationFailed(format!(
                "Docker CLI command failed ({}): {detail}",
                command.args.join(" ")
            )));
        }

        Ok(DockerCommandOutput { stdout, stderr })
    }
}
