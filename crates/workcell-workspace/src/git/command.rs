use std::{
    path::Path,
    process::{Command, Output},
};

use epilogos_workcell_core::{Result, WorkcellError};

pub(super) fn available() -> bool {
    Command::new("git")
        .arg("--version")
        .output()
        .is_ok_and(|output| output.status.success())
}

pub(super) fn stdout(repository: &Path, args: &[&str], context: &str) -> Result<String> {
    let output = output(repository, args, context)?;
    Ok(String::from_utf8_lossy(&output.stdout).trim().to_owned())
}

pub(super) fn run(repository: &Path, args: &[&str], context: &str) -> Result<()> {
    output(repository, args, context).map(|_| ())
}

fn output(repository: &Path, args: &[&str], context: &str) -> Result<Output> {
    let output = Command::new("git")
        .arg("-C")
        .arg(repository)
        .args(args)
        .output()
        .map_err(|error| WorkcellError::Unavailable(format!("{context}: {error}")))?;
    if !output.status.success() {
        return Err(WorkcellError::OperationFailed(format!(
            "{context}: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        )));
    }
    Ok(output)
}

pub(super) fn path_arg(path: &Path) -> Result<&str> {
    path.to_str().ok_or_else(|| {
        WorkcellError::Unsupported(format!(
            "git provider cannot pass non-UTF8 path `{}` to git",
            path.display()
        ))
    })
}
