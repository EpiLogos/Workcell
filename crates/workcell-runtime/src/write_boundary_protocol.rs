//! Transparent protected exec for an owner's existing stdio protocol process.
//! No proxy, event store, timeout policy or second session identity is created.
use crate::PreparedWriteBoundary;
use epilogos_workcell_core::{Result, WorkcellError};
use std::process::Command;

impl PreparedWriteBoundary {
    /// Replace this launcher with the protected program, preserving its PID and
    /// the owner's three protocol pipes. Nothing is written to those pipes by
    /// this adapter. The native session owner retains cancellation and lifetime.
    ///
    /// Unlike bounded `run`, every inherited standard descriptor must be a pipe
    /// or socket, never a writable file/device opened before confinement. Other
    /// descriptors become close-on-exec in the existing native boundary hook.
    pub fn exec_protocol(&self, mut command: Command, current_revision: &str) -> Result<()> {
        #[cfg(target_os = "linux")]
        {
            use std::os::unix::process::CommandExt;
            use std::process::Stdio;
            for fd in 0..=2 {
                let mut stat = std::mem::MaybeUninit::<libc::stat>::uninit();
                // SAFETY: fstat initialises this owned output on success. No
                // descriptor ownership or lifetime is changed by inspection.
                if unsafe { libc::fstat(fd, stat.as_mut_ptr()) } != 0 {
                    return Err(WorkcellError::Unavailable(format!(
                        "inspect protocol descriptor {fd}: {}",
                        std::io::Error::last_os_error()
                    )));
                }
                // SAFETY: successful fstat initialised the complete structure.
                let kind = unsafe { stat.assume_init() }.st_mode & libc::S_IFMT;
                if kind != libc::S_IFIFO && kind != libc::S_IFSOCK {
                    return Err(WorkcellError::Unsupported(format!(
                        "protocol descriptor {fd} must be a pipe or socket; pre-opened files/devices are not a protected transport"
                    )));
                }
            }
            self.configure_command(&mut command, current_revision)?;
            command
                .stdin(Stdio::inherit())
                .stdout(Stdio::inherit())
                .stderr(Stdio::inherit());
            let error = command.exec();
            Err(WorkcellError::OperationFailed(format!(
                "protected protocol exec failed before a resident program was established: {error}"
            )))
        }
        #[cfg(not(target_os = "linux"))]
        {
            let _ = (&mut command, current_revision);
            Err(WorkcellError::Unsupported(
                "protected protocol exec requires the supported Linux write boundary".into(),
            ))
        }
    }
}
