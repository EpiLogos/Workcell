//! Native macOS path-based write confinement. This deliberately does not claim
//! inode-bound grants after external rename/replacement, read confidentiality,
//! network/delegated-service restriction or live revocation.
use super::*;
use std::{
    ffi::OsStr,
    io,
    os::unix::{fs::MetadataExt, process::CommandExt},
    process::Stdio,
    time::{Duration, Instant},
};

const SANDBOX: &str = "/usr/bin/sandbox-exec";

pub(super) fn probe() -> Result<i32> {
    if unsafe { libc::geteuid() } == 0 {
        return Err(WorkcellError::Unsupported(
            "macOS write confinement requires an unprivileged process".into(),
        ));
    }
    let metadata = std::fs::symlink_metadata(SANDBOX)
        .map_err(|e| WorkcellError::Unsupported(format!("native sandbox unavailable: {e}")))?;
    if !metadata.is_file() || metadata.uid() != 0 || metadata.mode() & 0o022 != 0 {
        return Err(WorkcellError::Unsupported(
            "native sandbox executable is not an immutable root-owned regular file".into(),
        ));
    }
    let mut child = Command::new(SANDBOX)
        .args([
            "-p",
            "(version 1)(allow default)(deny file-write*)",
            "/usr/bin/true",
        ])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|e| WorkcellError::Unsupported(format!("native sandbox probe: {e}")))?;
    let start = Instant::now();
    loop {
        match child.try_wait() {
            Ok(Some(status)) if status.success() => return Ok(1),
            Ok(Some(status)) => {
                return Err(WorkcellError::Unsupported(format!(
                    "native sandbox refused: {status}"
                )))
            }
            Ok(None) if start.elapsed() < Duration::from_secs(2) => {
                std::thread::sleep(Duration::from_millis(5))
            }
            result => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(WorkcellError::Unsupported(format!(
                    "native sandbox probe timed out or failed: {result:?}"
                )));
            }
        }
    }
}

fn literal(path: &std::path::Path) -> Result<String> {
    let text = path
        .to_str()
        .ok_or_else(|| invalid("macOS sandbox paths must be UTF-8"))?;
    if text.chars().any(char::is_control) {
        return Err(invalid(
            "macOS sandbox paths cannot contain control characters",
        ));
    }
    // SBPL strings, not shell strings or executable expressions. Keep Unicode
    // literal; JSON's \u escape is not assumed to be SBPL syntax.
    Ok(format!(
        "\"{}\"",
        text.replace('\\', "\\\\").replace('"', "\\\"")
    ))
}

#[derive(Clone, Debug)]
pub(super) struct Ruleset {
    profile: String,
}

impl Ruleset {
    pub(super) fn prepare(paths: &[MaterialPath]) -> Result<Self> {
        probe()?;
        let mut profile = "(version 1)\n(allow default)\n(deny file-write*)\n".to_owned();
        for path in paths {
            profile.push_str(&format!(
                "(allow file-write* (subpath {}))\n",
                literal(&path.canonical)?
            ));
        }
        // Even nested grants cannot rename/remove a grant root and redirect the
        // meaning of a rule. External actors remain outside this boundary.
        for path in paths {
            profile.push_str(&format!(
                "(deny file-write-unlink (literal {}))\n",
                literal(&path.canonical)?
            ));
        }
        Ok(Self { profile })
    }

    pub(super) fn command(&self, program: &OsStr) -> Result<Command> {
        let null = null_device()?;
        let mut command = Command::new(SANDBOX);
        command.args(["-p", &self.profile, "--"]).arg(program);
        command
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        // Only kernel descriptor enumeration and fcntl run after fork. The
        // fixed buffer avoids allocation/locks; too many descriptors fail
        // closed. Enumerating in the child includes concurrent parent opens
        // before fork. CLOEXEC preserves Rust's exec-error pipe until exec.
        unsafe {
            command.pre_exec(move || {
                safe_stdio(null)?;
                const LIMIT: usize = 4096;
                let mut descriptors = [std::mem::MaybeUninit::<libc::proc_fdinfo>::uninit(); LIMIT];
                let bytes = std::mem::size_of_val(&descriptors);
                let pid = libc::getpid();
                let needed =
                    libc::proc_pidinfo(pid, libc::PROC_PIDLISTFDS, 0, std::ptr::null_mut(), 0);
                if needed <= 0 || needed as usize >= bytes {
                    return Err(io::Error::from_raw_os_error(libc::EMFILE));
                }
                let count = libc::proc_pidinfo(
                    pid,
                    libc::PROC_PIDLISTFDS,
                    0,
                    descriptors.as_mut_ptr().cast(),
                    bytes as i32,
                );
                if count <= 0
                    || count as usize >= bytes
                    || !(count as usize).is_multiple_of(std::mem::size_of::<libc::proc_fdinfo>())
                {
                    return Err(io::Error::from_raw_os_error(libc::EIO));
                }
                for descriptor in
                    &descriptors[..count as usize / std::mem::size_of::<libc::proc_fdinfo>()]
                {
                    let fd = descriptor.assume_init().proc_fd;
                    if fd >= 3 && libc::fcntl(fd, libc::F_SETFD, libc::FD_CLOEXEC) < 0 {
                        return Err(io::Error::last_os_error());
                    }
                }
                Ok(())
            });
        }
        Ok(command)
    }

    pub(super) fn configure(&self, _: &mut Command) -> Result<()> {
        Err(WorkcellError::Unsupported(
            "macOS requires PreparedWriteBoundary::command before adding arguments/environment; late wrapping cannot preserve Command environment semantics".into(),
        ))
    }
}
