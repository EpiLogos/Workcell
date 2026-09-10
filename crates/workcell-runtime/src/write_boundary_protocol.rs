//! Native protocol bodies use the same prepared material boundary as finite
//! commands. No ACP/Pi/session semantics live here. Successful exec replaces this
//! process, retaining the process identity/lifetime owned by the caller.
use crate::PreparedWriteBoundary;
use epilogos_workcell_core::{Result, WorkcellError};
use std::process::Command;

impl PreparedWriteBoundary {
    /// Replace the calling process with an explicitly supplied protocol body.
    /// Input/output must be ordinary one-way pipes, never inherited writable
    /// regular files or terminals. Stderr becomes /dev/null; all other inherited
    /// descriptors close at exec through the existing Linux boundary.
    ///
    /// Success does not return. This operation enforces the supplied material
    /// requirements, not Central recognition or semantic execution authority.
    /// The caller must recheck current authority before each subsequent turn.
    #[cfg(target_os = "linux")]
    pub fn exec_protocol(
        &self,
        command: &mut Command,
        current_policy_revision: &str,
    ) -> Result<()> {
        use std::os::unix::process::CommandExt;
        use std::process::Stdio;
        self.revalidate(current_policy_revision)?;
        for (fd, direction) in [
            (libc::STDIN_FILENO, libc::O_RDONLY),
            (libc::STDOUT_FILENO, libc::O_WRONLY),
        ] {
            let mut stat = std::mem::MaybeUninit::<libc::stat>::uninit();
            // SAFETY: fstat initializes this local stat on success; no pointer
            // escapes. The access mode inspection neither duplicates nor closes
            // the caller's descriptor.
            if unsafe { libc::fstat(fd, stat.as_mut_ptr()) } != 0 {
                return Err(WorkcellError::Unavailable(
                    "protocol requires inspectable stdin/stdout pipes".into(),
                ));
            }
            let stat = unsafe { stat.assume_init() };
            let flags = unsafe { libc::fcntl(fd, libc::F_GETFL) };
            if stat.st_mode & libc::S_IFMT != libc::S_IFIFO
                || flags < 0
                || flags & libc::O_ACCMODE != direction
            {
                return Err(WorkcellError::Unsupported(
                    "protocol stdin/stdout must be one-way pipes; inherited files, sockets and terminals are refused".into(),
                ));
            }
        }
        self.configure_command(command, current_policy_revision)?;
        command
            .stdin(Stdio::inherit())
            .stdout(Stdio::inherit())
            .stderr(Stdio::null());
        // CommandExt::exec runs the prepared pre-exec confinement before execve.
        // A failed restriction or exec never launches an unconfined substitute.
        let failure = command.exec();
        Err(WorkcellError::OperationFailed(format!(
            "confined protocol exec failed: {failure}"
        )))
    }

    #[cfg(not(target_os = "linux"))]
    pub fn exec_protocol(
        &self,
        _command: &mut Command,
        _current_policy_revision: &str,
    ) -> Result<()> {
        Err(WorkcellError::Unsupported(
            "native write-confined protocol exec is implemented on Linux only".into(),
        ))
    }
}
