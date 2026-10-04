//! Output of an explicitly requested bounded command, not passive telemetry.
use epilogos_workcell_core::{Result, WorkcellError};
use std::{
    process::{Command, ExitStatus},
    time::Duration,
};

#[derive(Debug)]
pub struct BoundedProcessOutput {
    pub status: ExitStatus,
    pub timed_out: bool,
    pub stdout: Vec<u8>,
    pub stderr: Vec<u8>,
    pub output_truncated: bool,
    pub output_complete: bool,
}
/// Consume only pipes (never pre-open writable files). Drain beyond the bounded
/// returned prefix so excessive output cannot deadlock execution. Every pipe
/// is owned by this call and closes before return, even when an escaped writer
/// retains its end. Incomplete capture is not completion or daemon retirement.
/// This owner exclusively reaps its spawned Child. An ambient SIGCHLD handler
/// or unrelated waiter must not reap it. Observed ownership loss refuses cleanup;
/// Unix does not offer atomic group delivery against a hostile external reaper.

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BoundedCaptureFailureKind {
    InvalidLimits,
    UnsupportedPlatform,
    SpawnFailed,
    MissingPipes,
    PipeAdmissionFailed,
    ReadFailed,
    WaitFailed,
    DeadlineElapsed,
    CleanupFailed,
}

/// Actual bounded capture facts. This is neither a native Return nor authority
/// to retry the command. No absence/exit status is inferred from a failed wait.
pub struct BoundedProcessFailure {
    inner: Box<BoundedProcessFailureData>,
}

struct BoundedProcessFailureData {
    kind: BoundedCaptureFailureKind,
    operation: &'static str,
    contract: Option<&'static str>,
    cause: Option<std::io::Error>,
    cleanup_cause: Option<std::io::Error>,
    capture_cause: Option<std::io::Error>,
    spawn_attempted: bool,
    spawned: bool,
    status: Option<ExitStatus>,
    timed_out: bool,
    stdout: Vec<u8>,
    stderr: Vec<u8>,
    stdout_eof: bool,
    stderr_eof: bool,
    output_truncated: bool,
    output_capture_requested: bool,
    cleanup_observation: &'static str,
    termination_requested: bool,
    termination_request_accepted: bool,
    reaped_by_owner: bool,
}

impl BoundedProcessFailure {
    fn contract(
        kind: BoundedCaptureFailureKind,
        operation: &'static str,
        detail: &'static str,
    ) -> Self {
        Self {
            inner: Box::new(BoundedProcessFailureData {
                kind,
                operation,
                contract: Some(detail),
                cause: None,
                cleanup_cause: None,
                capture_cause: None,
                spawn_attempted: false,
                spawned: false,
                status: None,
                timed_out: false,
                stdout: Vec::new(),
                stderr: Vec::new(),
                stdout_eof: false,
                stderr_eof: false,
                output_truncated: false,
                output_capture_requested: true,
                cleanup_observation: "not-attempted",
                termination_requested: false,
                termination_request_accepted: false,
                reaped_by_owner: false,
            }),
        }
    }

    pub fn kind(&self) -> BoundedCaptureFailureKind {
        self.inner.kind
    }
    pub fn status(&self) -> Option<ExitStatus> {
        self.inner.status
    }
    pub fn spawn_attempted(&self) -> bool {
        self.inner.spawn_attempted
    }
    pub fn spawned(&self) -> bool {
        self.inner.spawned
    }
    pub fn timed_out(&self) -> bool {
        self.inner.timed_out
    }
    pub fn output_complete(&self) -> bool {
        self.inner.stdout_eof && self.inner.stderr_eof
    }
    pub fn output_truncated(&self) -> bool {
        self.inner.output_truncated
    }
    /// Explicit byte access for the caller that authorised this command.
    /// Generic error formatting never exposes these potentially private bytes.
    pub fn stdout(&self) -> &[u8] {
        &self.inner.stdout
    }
    pub fn stderr(&self) -> &[u8] {
        &self.inner.stderr
    }
    pub fn native_cause(&self) -> Option<&std::io::Error> {
        self.inner.cause.as_ref()
    }
    pub fn cleanup_cause(&self) -> Option<&std::io::Error> {
        self.inner.cleanup_cause.as_ref()
    }
    pub fn capture_cause(&self) -> Option<&std::io::Error> {
        self.inner.capture_cause.as_ref()
    }

    /// Safe structural facts, not private error text or captured bytes.
    pub fn observation(&self) -> serde_json::Value {
        let native = |cause: Option<&std::io::Error>| {
            cause.map(|error|
            serde_json::json!({"kind":format!("{:?}",error.kind()),"raw_os_error":error.raw_os_error()}))
        };
        #[cfg(unix)]
        let signal = {
            use std::os::unix::process::ExitStatusExt;
            self.inner.status.and_then(|status| status.signal())
        };
        #[cfg(not(unix))]
        let signal: Option<i32> = None;
        serde_json::json!({
            "schema":"workcell.bounded-capture-failure/v1","kind":format!("{:?}",self.inner.kind),
            "operation":self.inner.operation,"spawn_attempted":self.inner.spawn_attempted,
            "spawn_observed":self.inner.spawned,
            "executed":if self.inner.spawned {Some(true)} else if !self.inner.spawn_attempted {Some(false)} else {None},
            "status_observed":self.inner.status.is_some(),"exit_code":self.inner.status.and_then(|s|s.code()),
            "exit_signal":signal,"timed_out":self.inner.timed_out,
            "stdout_bytes":self.inner.stdout.len(),"stderr_bytes":self.inner.stderr.len(),
            "stdout_eof":self.inner.stdout_eof,"stderr_eof":self.inner.stderr_eof,
            "output_complete":self.output_complete(),"output_truncated":self.inner.output_truncated,
            "output_capture_requested":self.inner.output_capture_requested,
            "cleanup_observation":self.inner.cleanup_observation,
            "termination_requested":self.inner.termination_requested,
            "termination_request_accepted":self.inner.termination_request_accepted,
            "reaped_by_owner":self.inner.reaped_by_owner,
            "native_cause":native(self.inner.cause.as_ref()),
            "cleanup_cause":native(self.inner.cleanup_cause.as_ref()),
            "capture_cause":native(self.inner.capture_cause.as_ref()),
            "automatic_retry":false,"effect_state":"failed or unverified; inspect before retry"
        })
    }
}

impl std::fmt::Display for BoundedProcessFailure {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "bounded capture {:?} at {}; spawned={}; status={:?}; timed_out={}; capture_complete={}; truncated={}; cleanup={}",
            self.inner.kind,self.inner.operation,self.inner.spawned,self.inner.status,self.inner.timed_out,
            self.output_complete(),self.inner.output_truncated,self.inner.cleanup_observation)?;
        if !self.inner.output_capture_requested {
            write!(f, "; output capture not requested")?;
        }
        if let Some(detail) = self.inner.contract {
            write!(f, "; {detail}")?;
        }
        if let Some(cause) = &self.inner.cause {
            write!(
                f,
                "; native cause kind={:?}, raw_os_error={:?}",
                cause.kind(),
                cause.raw_os_error()
            )?;
        }
        if let Some(cause) = &self.inner.cleanup_cause {
            write!(
                f,
                "; secondary cleanup cause kind={:?}, raw_os_error={:?}",
                cause.kind(),
                cause.raw_os_error()
            )?;
        }
        if let Some(cause) = &self.inner.capture_cause {
            write!(
                f,
                "; secondary capture cause kind={:?}, raw_os_error={:?}",
                cause.kind(),
                cause.raw_os_error()
            )?;
        }
        write!(f, "; bytes withheld; no automatic retry")
    }
}

impl std::fmt::Debug for BoundedProcessFailure {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("BoundedProcessFailure")
            .field("observation", &self.observation())
            .field("stdout", &"[private bounded bytes withheld]")
            .field("stderr", &"[private bounded bytes withheld]")
            .finish()
    }
}

impl std::error::Error for BoundedProcessFailure {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        self.inner
            .cause
            .as_ref()
            .map(|cause| cause as &(dyn std::error::Error + 'static))
    }
}

/// Preserve bounded capture and original native causality from the one owner.
/// A caller must not treat this failure as permission to retry or as a Return.
pub fn capture_bounded_process(
    command: Command,
    timeout: Duration,
    output_limit: usize,
) -> std::result::Result<BoundedProcessOutput, BoundedProcessFailure> {
    if timeout.is_zero() || timeout > Duration::from_secs(60) || output_limit > 1_048_576 {
        return Err(BoundedProcessFailure::contract(
            BoundedCaptureFailureKind::InvalidLimits,
            "validate limits",
            "command limits require 0 < timeout <= 60s and output <= 1 MiB",
        ));
    }
    #[cfg(any(unix, windows))]
    {
        native::run(command, timeout, output_limit)
    }
    #[cfg(not(any(unix, windows)))]
    {
        let _ = command;
        Err(BoundedProcessFailure::contract(
            BoundedCaptureFailureKind::UnsupportedPlatform,
            "platform admission",
            "bounded native pipe capture unavailable; no command spawned",
        ))
    }
}

/// Crate-private legacy command route. Output is actually sent to the OS null
/// device; it is neither piped and discarded nor represented as observed EOF.
/// Spawn/wait/retirement remain owned by the same native Child driver.
pub(crate) fn run_status_only(
    command: Command,
    timeout: Duration,
) -> std::result::Result<ExitStatus, BoundedProcessFailure> {
    let result = if timeout.is_zero() || timeout > Duration::from_secs(60) {
        Err(BoundedProcessFailure::contract(
            BoundedCaptureFailureKind::InvalidLimits,
            "validate limits",
            "command limits require 0 < timeout <= 60s",
        ))
    } else {
        #[cfg(any(unix, windows))]
        {
            native::run_status_only(command, timeout)
        }
        #[cfg(not(any(unix, windows)))]
        {
            let _ = command;
            Err(BoundedProcessFailure::contract(
                BoundedCaptureFailureKind::UnsupportedPlatform,
                "platform admission",
                "bounded native status wait unavailable; no command spawned",
            ))
        }
    };
    result.map_err(|mut failure| {
        failure.inner.output_capture_requested = false;
        failure
    })
}

pub fn run_bounded_process(
    command: Command,
    timeout: Duration,
    output_limit: usize,
) -> Result<BoundedProcessOutput> {
    capture_bounded_process(command, timeout, output_limit).or_else(|failure| {
        // Preserve the existing known-status timeout DTO. A missing status or
        // native capture/cleanup error never manufactures one for compatibility.
        match failure.inner.status {
            Some(status)
                if failure.inner.kind == BoundedCaptureFailureKind::DeadlineElapsed
                    && failure.inner.cleanup_cause.is_none()
                    && failure.inner.capture_cause.is_none() =>
            {
                return Ok(BoundedProcessOutput {
                    status,
                    timed_out: true,
                    stdout: failure.inner.stdout,
                    stderr: failure.inner.stderr,
                    output_truncated: failure.inner.output_truncated,
                    output_complete: failure.inner.stdout_eof && failure.inner.stderr_eof,
                });
            }
            _ => {}
        }
        let message = failure.to_string();
        Err(match failure.inner.kind {
            BoundedCaptureFailureKind::InvalidLimits => WorkcellError::InvalidDemand(
                "command limits require 0 < timeout <= 60s and output <= 1 MiB".into(),
            ),
            BoundedCaptureFailureKind::MissingPipes
                if failure.inner.reaped_by_owner && failure.inner.cleanup_cause.is_none() =>
            {
                WorkcellError::InvalidDemand(
                    "bounded command requires piped stdout and stderr".into(),
                )
            }
            BoundedCaptureFailureKind::UnsupportedPlatform => WorkcellError::Unsupported(message),
            _ => WorkcellError::OperationFailed(message),
        })
    })
}

#[cfg(any(unix, windows))]
mod native {
    use super::*;
    #[cfg(windows)]
    use std::os::windows::io::AsRawHandle;
    use std::{
        io::{self, Read},
        process::Child,
        thread,
        time::Instant,
    };
    #[cfg(unix)]
    use std::{os::fd::AsRawFd, os::unix::process::CommandExt};

    #[cfg(unix)]
    trait NativePipe: Read + AsRawFd {}
    #[cfg(unix)]
    impl<T: Read + AsRawFd> NativePipe for T {}
    #[cfg(windows)]
    trait NativePipe: Read + AsRawHandle {}
    #[cfg(windows)]
    impl<T: Read + AsRawHandle> NativePipe for T {}

    #[cfg(windows)]
    #[link(name = "kernel32")]
    extern "system" {
        fn PeekNamedPipe(
            handle: std::os::windows::io::RawHandle,
            buffer: *mut std::ffi::c_void,
            size: u32,
            read: *mut u32,
            available: *mut u32,
            remaining: *mut u32,
        ) -> i32;
    }

    const OUTPUT_GRACE: Duration = Duration::from_millis(100);
    const CLEANUP_GRACE: Duration = Duration::from_millis(200);

    struct Capture<T> {
        pipe: Option<T>,
        output: Vec<u8>,
        limit: usize,
        truncated: bool,
    }

    impl<T: NativePipe> Capture<T> {
        fn new(pipe: T, limit: usize) -> io::Result<Self> {
            #[cfg(unix)]
            {
                let fd = pipe.as_raw_fd();
                let flags = unsafe { libc::fcntl(fd, libc::F_GETFL) };
                if flags < 0
                    || unsafe { libc::fcntl(fd, libc::F_SETFL, flags | libc::O_NONBLOCK) } < 0
                {
                    return Err(io::Error::last_os_error());
                }
            }
            Ok(Self {
                pipe: Some(pipe),
                output: Vec::new(),
                limit,
                truncated: false,
            })
        }

        fn drain(&mut self, deadline: Instant) -> io::Result<()> {
            let mut chunk = [0u8; 8192];
            // Bounded work per stream keeps an infinite stdout writer from
            // starving stderr, the native wait observation or the deadline.
            for _ in 0..32 {
                if Instant::now() >= deadline {
                    break;
                }
                let Some(pipe) = &mut self.pipe else { break };
                #[cfg(unix)]
                let available = chunk.len();
                #[cfg(windows)]
                let available = {
                    // This call exclusively owns the anonymous read handle;
                    // no simultaneous blocking read uses it. Peek observes
                    // existing bytes before any synchronous ReadFile call.
                    let mut available = 0u32;
                    if unsafe {
                        PeekNamedPipe(
                            pipe.as_raw_handle(),
                            std::ptr::null_mut(),
                            0,
                            std::ptr::null_mut(),
                            &mut available,
                            std::ptr::null_mut(),
                        )
                    } == 0
                    {
                        let error = io::Error::last_os_error();
                        if error.raw_os_error() == Some(109) {
                            // ERROR_BROKEN_PIPE: all writers closed.
                            self.pipe = None;
                            break;
                        }
                        return Err(error);
                    }
                    if available == 0 {
                        break;
                    }
                    (available as usize).min(chunk.len())
                };
                match pipe.read(&mut chunk[..available]) {
                    Ok(0) => {
                        self.pipe = None;
                        break;
                    }
                    Ok(n) => {
                        let keep = n.min(self.limit.saturating_sub(self.output.len()));
                        self.output.extend_from_slice(&chunk[..keep]);
                        self.truncated |= keep < n;
                    }
                    Err(e) if e.kind() == io::ErrorKind::Interrupted => {}
                    Err(e) if e.kind() == io::ErrorKind::WouldBlock => break,
                    Err(e) => return Err(e),
                }
            }
            Ok(())
        }

        fn complete(&self) -> bool {
            self.pipe.is_none()
        }
    }

    struct Cleanup {
        status: Option<ExitStatus>,
        observation: &'static str,
        requested: bool,
        accepted: bool,
        reaped: bool,
        error: Option<io::Error>,
    }

    impl Cleanup {
        fn none() -> Self {
            Self {
                status: None,
                observation: "not-attempted",
                requested: false,
                accepted: false,
                reaped: false,
                error: None,
            }
        }
        fn wait_refusal(&mut self, error: io::Error) {
            #[cfg(unix)]
            let lost = error.raw_os_error() == Some(libc::ECHILD);
            #[cfg(windows)]
            let lost = false;
            self.observation = if lost {
                "ownership-lost-no-further-signal-or-reap"
            } else {
                "wait-observation-failed-no-further-signal-or-reap"
            };
            self.error = Some(error);
        }
    }

    /// Normal group cancellation requires exclusive direct-Child reaping.
    /// Every failed observation stops before further numeric signalling.
    /// The final observation is not atomic against a hostile external waiter.
    fn retire_owned(child: &mut Child, deadline: Instant) -> Cleanup {
        let mut observed = Cleanup::none();
        match child.try_wait() {
            Ok(Some(status)) => {
                observed.status = Some(status);
                observed.reaped = true;
                observed.observation = "already-retired-by-owned-wait";
                return observed;
            }
            Ok(None) => {}
            Err(error) => {
                observed.wait_refusal(error);
                return observed;
            }
        }
        #[cfg(unix)]
        {
            let pid = child.id() as libc::pid_t;
            let group = unsafe { libc::getpgid(pid) };
            if group < 0 {
                observed.observation = "original-group-observation-failed-no-signal";
                observed.error = Some(io::Error::last_os_error());
                return observed;
            }
            if group != pid {
                observed.observation = "original-group-changed-no-signal";
                observed.error = Some(io::Error::other(
                    "retained Child changed its original owned process group",
                ));
                return observed;
            }
            match child.try_wait() {
                Ok(Some(status)) => {
                    observed.status = Some(status);
                    observed.reaped = true;
                    observed.observation = "retired-before-group-request";
                    return observed;
                }
                Ok(None) => {}
                Err(error) => {
                    observed.wait_refusal(error);
                    return observed;
                }
            }
            observed.requested = true;
            if unsafe { libc::kill(-group, libc::SIGKILL) } < 0 {
                observed.observation = "group-termination-request-failed";
                observed.error = Some(io::Error::last_os_error());
                return observed;
            }
            observed.accepted = true;
        }
        #[cfg(windows)]
        {
            observed.requested = true;
            if let Err(error) = child.kill() {
                observed.observation = "native-handle-termination-request-failed";
                observed.error = Some(error);
                return observed;
            }
            observed.accepted = true;
        }
        loop {
            match child.try_wait() {
                Ok(Some(status)) => {
                    observed.status = Some(status);
                    observed.reaped = true;
                    observed.observation = "termination-request-accepted-and-owned-child-reaped";
                    return observed;
                }
                Ok(None) if Instant::now() < deadline => thread::sleep(Duration::from_millis(2)),
                Ok(None) => {
                    observed.observation = "termination-request-accepted-retirement-uncertain";
                    observed.error = Some(io::Error::new(
                        io::ErrorKind::TimedOut,
                        "finite owned cleanup deadline elapsed before native reap",
                    ));
                    return observed;
                }
                Err(error) => {
                    observed.wait_refusal(error);
                    return observed;
                }
            }
        }
    }

    struct Captured {
        out: Option<Capture<std::process::ChildStdout>>,
        err: Option<Capture<std::process::ChildStderr>>,
        status: Option<ExitStatus>,
        timed_out: bool,
        output_capture_requested: bool,
    }
    fn captured(
        out: Option<Capture<std::process::ChildStdout>>,
        err: Option<Capture<std::process::ChildStderr>>,
        status: Option<ExitStatus>,
        timed_out: bool,
    ) -> Captured {
        captured_for_intent(out, err, status, timed_out, true)
    }
    fn captured_for_intent(
        out: Option<Capture<std::process::ChildStdout>>,
        err: Option<Capture<std::process::ChildStderr>>,
        status: Option<ExitStatus>,
        timed_out: bool,
        output_capture_requested: bool,
    ) -> Captured {
        Captured {
            out,
            err,
            status,
            timed_out,
            output_capture_requested,
        }
    }
    fn failure(
        kind: BoundedCaptureFailureKind,
        operation: &'static str,
        cause: Option<io::Error>,
        captured: Captured,
        cleanup: Cleanup,
    ) -> BoundedProcessFailure {
        let Captured {
            out,
            err,
            status,
            timed_out,
            output_capture_requested,
        } = captured;
        let mut result = BoundedProcessFailure::contract(
            kind,
            operation,
            "failed or unverified native activity; no automatic retry",
        );
        result.inner.cause = cause;
        result.inner.spawn_attempted = true;
        result.inner.spawned = true;
        result.inner.status = cleanup.status.or(status);
        result.inner.timed_out = timed_out;
        result.inner.output_capture_requested = output_capture_requested;
        result.inner.cleanup_observation = cleanup.observation;
        result.inner.termination_requested = cleanup.requested;
        result.inner.termination_request_accepted = cleanup.accepted;
        result.inner.reaped_by_owner = cleanup.reaped || status.is_some();
        result.inner.cleanup_cause = cleanup.error;
        if let Some(out) = out {
            result.inner.stdout_eof = out.complete();
            result.inner.output_truncated |= out.truncated;
            result.inner.stdout = out.output;
        }
        if let Some(err) = err {
            result.inner.stderr_eof = err.complete();
            result.inner.output_truncated |= err.truncated;
            result.inner.stderr = err.output;
        }
        result
    }

    #[derive(Clone, Copy)]
    enum OutputIntent {
        Capture(usize),
        NullStatus,
    }

    enum NativeOutput {
        Captured(BoundedProcessOutput),
        Status(ExitStatus),
    }

    fn spawn_owned(
        mut command: Command,
        timeout: Duration,
        intent: OutputIntent,
    ) -> std::result::Result<(Child, Instant), BoundedProcessFailure> {
        let deadline = Instant::now() + timeout;
        if matches!(intent, OutputIntent::NullStatus) {
            command
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null());
        }
        #[cfg(unix)]
        command.process_group(0);
        let child = command.spawn().map_err(|cause| {
            let mut failure = BoundedProcessFailure::contract(
                BoundedCaptureFailureKind::SpawnFailed,
                "spawn",
                "spawn failed before a retained Child was observed; execution unknown",
            );
            failure.inner.spawn_attempted = true;
            failure.inner.cause = Some(cause);
            failure.inner.output_capture_requested = matches!(intent, OutputIntent::Capture(_));
            failure
        })?;
        Ok((child, deadline))
    }

    pub(super) fn run(
        command: Command,
        timeout: Duration,
        limit: usize,
    ) -> std::result::Result<BoundedProcessOutput, BoundedProcessFailure> {
        let (child, deadline) = spawn_owned(command, timeout, OutputIntent::Capture(limit))?;
        capture(child, deadline, limit)
    }

    pub(super) fn run_status_only(
        command: Command,
        timeout: Duration,
    ) -> std::result::Result<ExitStatus, BoundedProcessFailure> {
        let (child, deadline) = spawn_owned(command, timeout, OutputIntent::NullStatus)?;
        capture_status_only(child, deadline)
    }

    // Both wrappers enter the SAME production post-spawn loop. Tests can also
    // enter it after a genuine external waitpid without injecting a wait result.
    pub(super) fn capture(
        child: Child,
        deadline: Instant,
        limit: usize,
    ) -> std::result::Result<BoundedProcessOutput, BoundedProcessFailure> {
        match drive(child, deadline, OutputIntent::Capture(limit))? {
            NativeOutput::Captured(output) => Ok(output),
            NativeOutput::Status(_) => {
                unreachable!("closed capture intent returned status-only output")
            }
        }
    }

    pub(super) fn capture_status_only(
        child: Child,
        deadline: Instant,
    ) -> std::result::Result<ExitStatus, BoundedProcessFailure> {
        match drive(child, deadline, OutputIntent::NullStatus)? {
            NativeOutput::Status(status) => Ok(status),
            NativeOutput::Captured(_) => {
                unreachable!("closed null intent returned captured output")
            }
        }
    }

    fn pipes_complete(
        out: &Option<Capture<std::process::ChildStdout>>,
        err: &Option<Capture<std::process::ChildStderr>>,
    ) -> bool {
        out.as_ref().is_some_and(Capture::complete) && err.as_ref().is_some_and(Capture::complete)
    }

    fn drive(
        mut child: Child,
        deadline: Instant,
        intent: OutputIntent,
    ) -> std::result::Result<NativeOutput, BoundedProcessFailure> {
        let output_capture_requested = matches!(intent, OutputIntent::Capture(_));
        let (mut out, mut err) = match intent {
            OutputIntent::NullStatus => (None, None),
            OutputIntent::Capture(limit) => {
                let (out, err) = match (child.stdout.take(), child.stderr.take()) {
                    (Some(out), Some(err)) => (out, err),
                    _ => {
                        let cleanup = retire_owned(&mut child, Instant::now() + CLEANUP_GRACE);
                        return Err(failure(
                            BoundedCaptureFailureKind::MissingPipes,
                            "pipe admission",
                            None,
                            captured(None, None, None, false),
                            cleanup,
                        ));
                    }
                };
                let out = match Capture::new(out, limit) {
                    Ok(out) => out,
                    Err(cause) => {
                        let cleanup = retire_owned(&mut child, Instant::now() + CLEANUP_GRACE);
                        return Err(failure(
                            BoundedCaptureFailureKind::PipeAdmissionFailed,
                            "stdout native pipe admission",
                            Some(cause),
                            captured(None, None, None, false),
                            cleanup,
                        ));
                    }
                };
                let err = match Capture::new(err, limit) {
                    Ok(err) => err,
                    Err(cause) => {
                        let cleanup = retire_owned(&mut child, Instant::now() + CLEANUP_GRACE);
                        return Err(failure(
                            BoundedCaptureFailureKind::PipeAdmissionFailed,
                            "stderr native pipe admission",
                            Some(cause),
                            captured(Some(out), None, None, false),
                            cleanup,
                        ));
                    }
                };
                (Some(out), Some(err))
            }
        };
        let mut status = None;
        let mut capture_deadline = deadline;
        loop {
            let drained = out
                .as_mut()
                .map_or(Ok(()), |stream| stream.drain(capture_deadline));
            if let Err(cause) = drained {
                let cleanup = retire_owned(&mut child, Instant::now() + CLEANUP_GRACE);
                return Err(failure(
                    BoundedCaptureFailureKind::ReadFailed,
                    "stdout native read",
                    Some(cause),
                    captured_for_intent(out, err, status, false, output_capture_requested),
                    cleanup,
                ));
            }
            let drained = err
                .as_mut()
                .map_or(Ok(()), |stream| stream.drain(capture_deadline));
            if let Err(cause) = drained {
                let cleanup = retire_owned(&mut child, Instant::now() + CLEANUP_GRACE);
                return Err(failure(
                    BoundedCaptureFailureKind::ReadFailed,
                    "stderr native read",
                    Some(cause),
                    captured_for_intent(out, err, status, false, output_capture_requested),
                    cleanup,
                ));
            }
            if status.is_none() {
                match child.try_wait() {
                    Ok(Some(observed)) => {
                        status = Some(observed);
                        capture_deadline = (Instant::now() + OUTPUT_GRACE).min(deadline);
                    }
                    Ok(None) if Instant::now() < deadline => {}
                    Ok(None) => {
                        let cleanup_deadline = Instant::now() + CLEANUP_GRACE;
                        let cleanup = retire_owned(&mut child, cleanup_deadline);
                        let mut capture_cause = None;
                        while output_capture_requested
                            && Instant::now() < cleanup_deadline
                            && !pipes_complete(&out, &err)
                        {
                            if let Err(cause) = out
                                .as_mut()
                                .map_or(Ok(()), |stream| stream.drain(cleanup_deadline))
                            {
                                capture_cause = Some(cause);
                                break;
                            }
                            if let Err(cause) = err
                                .as_mut()
                                .map_or(Ok(()), |stream| stream.drain(cleanup_deadline))
                            {
                                capture_cause = Some(cause);
                                break;
                            }
                            if !pipes_complete(&out, &err) {
                                thread::sleep(Duration::from_millis(2));
                            }
                        }
                        let mut result = failure(
                            BoundedCaptureFailureKind::DeadlineElapsed,
                            "command deadline",
                            None,
                            captured_for_intent(out, err, status, true, output_capture_requested),
                            cleanup,
                        );
                        result.inner.contract = Some(if output_capture_requested {
                            "actual command deadline elapsed; cleanup and capture observations retained"
                        } else {
                            "actual command deadline elapsed; cleanup observations retained; output capture not requested"
                        });
                        result.inner.capture_cause = capture_cause;
                        return Err(result);
                    }
                    Err(cause) => {
                        let mut cleanup = Cleanup::none();
                        #[cfg(unix)]
                        let lost = cause.raw_os_error() == Some(libc::ECHILD);
                        #[cfg(windows)]
                        let lost = false;
                        cleanup.observation = if lost {
                            "ownership-lost-no-signal-or-reap"
                        } else {
                            "wait-observation-failed-no-signal-or-reap"
                        };
                        let mut result = failure(
                            BoundedCaptureFailureKind::WaitFailed,
                            "native child wait",
                            Some(cause),
                            captured_for_intent(out, err, status, false, output_capture_requested),
                            cleanup,
                        );
                        result.inner.contract =
                            Some("no signal or reap attempted after failed observation");
                        return Err(result);
                    }
                }
            }
            if status.is_some()
                && (!output_capture_requested
                    || pipes_complete(&out, &err)
                    || Instant::now() >= capture_deadline)
            {
                break;
            }
            thread::sleep(Duration::from_millis(2));
        }
        let status = status.expect("actual owned wait observed status");
        match intent {
            OutputIntent::NullStatus => Ok(NativeOutput::Status(status)),
            OutputIntent::Capture(_) => {
                let output_complete = pipes_complete(&out, &err);
                let out = out.expect("capture intent admitted actual stdout pipe");
                let err = err.expect("capture intent admitted actual stderr pipe");
                Ok(NativeOutput::Captured(BoundedProcessOutput {
                    status,
                    timed_out: false,
                    stdout: out.output,
                    stderr: err.output,
                    output_truncated: out.truncated || err.truncated,
                    output_complete,
                }))
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::process::Stdio;
    #[test]
    fn output_and_runtime_are_bounded_without_reporting_success_on_timeout() {
        let mut command = Command::new("python3");
        command
            .args(["-S", "-c", "import sys; sys.stdout.write('x'*100000)"])
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        let output = run_bounded_process(command, Duration::from_secs(5), 128).unwrap();
        assert!(!output.timed_out);
        assert!(output.status.success());
        assert_eq!(output.stdout.len(), 128);
        assert!(output.output_truncated);
        assert!(output.output_complete);
        let mut command = Command::new("python3");
        command
            .args(["-S", "-c", "import time;time.sleep(10)"])
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        let output = run_bounded_process(command, Duration::from_millis(100), 128).unwrap();
        assert!(output.timed_out);
        assert!(!output.status.success());
    }

    #[cfg(unix)]
    static CASE_SEQUENCE: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

    #[cfg(unix)]
    struct WriterFixture {
        root: std::path::PathBuf,
        script: std::path::PathBuf,
        finished: std::cell::Cell<bool>,
    }

    #[cfg(unix)]
    impl WriterFixture {
        fn new() -> Self {
            let base = std::path::PathBuf::from(
                std::env::var_os("WORKCELL_TEST_ARTIFACT_ROOT")
                    .expect("real capture gate requires an allocated artifact root"),
            );
            assert!(base.is_absolute() && base.is_dir());
            let base =
                std::fs::canonicalize(base).expect("native artifact root must resolve physically");
            let manifest_dir = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"));
            for ancestor in manifest_dir.ancestors() {
                let clearings = ancestor.join("Control/agents/now/clearings");
                if clearings.is_dir() {
                    let relative = base
                        .strip_prefix(std::fs::canonicalize(clearings).unwrap())
                        .expect("Central native evidence must remain below an allocated clearing");
                    let parts: Vec<_> = relative.components().collect();
                    assert!(
                        parts.len() >= 3 && parts[1].as_os_str() == "T",
                        "Central native evidence must be below an existing clearing T"
                    );
                    break;
                }
            }
            let root = base.join(format!(
                "capture-{}-{}",
                std::process::id(),
                CASE_SEQUENCE.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
            ));
            std::fs::create_dir(&root).unwrap();
            let script = root.join("writer.py");
            std::fs::write(
                &script,
                include_str!("../tests/fixtures/bounded_pipe_writer.py"),
            )
            .unwrap();
            Self {
                root,
                script,
                finished: std::cell::Cell::new(false),
            }
        }

        fn command(&self, escaped: bool) -> Command {
            let mut command = Command::new("python3");
            command
                .arg("-S")
                .arg(&self.script)
                .env("WORKCELL_PIPE_FIXTURE_ROOT", &self.root)
                .env("WORKCELL_PIPE_ESCAPE", if escaped { "1" } else { "0" })
                .stdin(Stdio::null())
                .stdout(Stdio::piped())
                .stderr(Stdio::piped());
            command
        }

        fn finish(&self) -> serde_json::Value {
            std::fs::write(self.root.join("release"), b"release owned writer").unwrap();
            let deadline = std::time::Instant::now() + Duration::from_secs(3);
            let result_path = self.root.join("writer-result.json");
            while !result_path.exists() && std::time::Instant::now() < deadline {
                std::thread::sleep(Duration::from_millis(5));
            }
            let result: serde_json::Value =
                serde_json::from_slice(&std::fs::read(result_path).unwrap()).unwrap();
            assert_eq!(result["released"], true);
            assert_eq!(result["writes"]["1"]["epipe"], true);
            assert_eq!(result["writes"]["2"]["epipe"], true);
            let pid = result["pid"].as_i64().unwrap() as libc::pid_t;
            loop {
                // Only this isolated test process became the Linux subreaper,
                // so this is its exact adopted writer, never an arbitrary PID.
                #[cfg(target_os = "linux")]
                {
                    let mut status = 0;
                    let reaped = unsafe { libc::waitpid(pid, &mut status, libc::WNOHANG) };
                    if reaped == pid {
                        assert!(libc::WIFEXITED(status) && libc::WEXITSTATUS(status) == 0);
                    } else if reaped < 0 {
                        assert_eq!(
                            std::io::Error::last_os_error().raw_os_error(),
                            Some(libc::ECHILD)
                        );
                    }
                }
                let mut query = Command::new("/bin/ps");
                query
                    .args(["-p", &pid.to_string(), "-o", "pid=,uid=,lstart=,stat="])
                    .env("LC_ALL", "C")
                    .stdin(Stdio::null())
                    .stdout(Stdio::piped())
                    .stderr(Stdio::piped());
                let observation = run_bounded_process(query, Duration::from_secs(1), 4096).unwrap();
                std::fs::write(self.root.join("writer-last-ps.stdout"), &observation.stdout)
                    .unwrap();
                std::fs::write(self.root.join("writer-last-ps.stderr"), &observation.stderr)
                    .unwrap();
                assert!(
                    !observation.timed_out
                        && observation.output_complete
                        && !observation.output_truncated
                );
                if observation.status.code() == Some(1)
                    && observation.stdout.is_empty()
                    && observation.stderr.is_empty()
                {
                    break;
                }
                assert!(std::time::Instant::now() < deadline,
                    "actual writer still present (including zombie); retained evidence, no guessed cleanup");
                std::thread::sleep(Duration::from_millis(5));
            }
            self.finished.set(true);
            result
        }
    }

    #[cfg(unix)]
    impl Drop for WriterFixture {
        fn drop(&mut self) {
            if let Err(error) = std::fs::write(self.root.join("release"), b"release owned writer") {
                eprintln!(
                    "owned writer release failed: {error}; retained fixture {}",
                    self.root.display()
                );
                if !std::thread::panicking() {
                    panic!("owned writer release failed; physical cleanup remains uncertain");
                }
            }
            if !self.finished.get() {
                let retained = serde_json::json!({"release_attempted":true,
                    "physical_retirement_verified":false,"owner_panic":std::thread::panicking(),
                    "standing":"failed fixture; writer has finite self-deadline; absence/reap still unverified"});
                if let Err(error) = std::fs::write(
                    self.root.join("cleanup-uncertainty.json"),
                    retained.to_string(),
                ) {
                    eprintln!("failed to retain real fixture cleanup uncertainty: {error}");
                    if !std::thread::panicking() {
                        panic!("fixture cleanup evidence persistence failed");
                    }
                }
            }
        }
    }

    #[cfg(unix)]
    fn isolated_case(test_name: &str) -> bool {
        if std::env::var("WORKCELL_CAPTURE_ISOLATED_CASE").as_deref() == Ok(test_name) {
            #[cfg(target_os = "linux")]
            assert_eq!(
                unsafe { libc::prctl(libc::PR_SET_CHILD_SUBREAPER, 1, 0, 0, 0) },
                0
            );
            return true;
        }
        let mut command = Command::new(std::env::current_exe().unwrap());
        command
            .args(["--exact", test_name, "--nocapture", "--test-threads=1"])
            .env("WORKCELL_CAPTURE_ISOLATED_CASE", test_name)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        let output = run_bounded_process(command, Duration::from_secs(12), 65536).unwrap();
        assert!(
            output.status.success()
                && !output.timed_out
                && output.output_complete
                && !output.output_truncated
                && String::from_utf8_lossy(&output.stdout)
                    .contains("1 passed; 0 failed; 0 ignored;"),
            "isolated real OS gate refused: stdout={} stderr={}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        false
    }

    #[cfg(unix)]
    #[test]
    fn escaped_writer_cannot_keep_capture_readers_alive_after_return() {
        if !isolated_case(
            "bounded_process::tests::escaped_writer_cannot_keep_capture_readers_alive_after_return",
        ) {
            return;
        }
        let fixture = WriterFixture::new();
        let output =
            run_bounded_process(fixture.command(true), Duration::from_secs(3), 4096).unwrap();
        assert!(output.status.success() && !output.timed_out);
        assert!(!output.output_complete && !output.output_truncated);
        assert_eq!(output.stdout, b"out-prefix\n");
        assert_eq!(output.stderr, b"err-prefix\n");
        let result = fixture.finish();
        std::fs::write(
            fixture.root.join("native-capture-result.json"),
            serde_json::to_vec(&result).unwrap(),
        )
        .unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn actual_external_reap_never_authorises_a_late_group_signal() {
        if !isolated_case(
            "bounded_process::tests::actual_external_reap_never_authorises_a_late_group_signal",
        ) {
            return;
        }
        use std::os::unix::process::CommandExt;
        let fixture = WriterFixture::new();
        let mut command = fixture.command(false);
        command.process_group(0);
        let child = command.spawn().unwrap();
        let direct_pid = child.id() as libc::pid_t;
        let deadline = std::time::Instant::now() + Duration::from_secs(3);
        let mut native_status = 0;
        loop {
            let reaped = unsafe { libc::waitpid(direct_pid, &mut native_status, libc::WNOHANG) };
            if reaped == direct_pid {
                break;
            }
            assert_eq!(reaped, 0, "actual external reap failed");
            assert!(
                std::time::Instant::now() < deadline,
                "direct child did not exit"
            );
            std::thread::sleep(Duration::from_millis(5));
        }
        assert!(libc::WIFEXITED(native_status) && libc::WEXITSTATUS(native_status) == 0);
        // Use the very same post-spawn owner loop as production after a real
        // external waitpid. No injected wait response or invented ExitStatus.
        let refusal = native::capture(child, deadline, 4096).unwrap_err();
        assert_eq!(refusal.kind(), BoundedCaptureFailureKind::WaitFailed);
        assert_eq!(
            refusal.native_cause().unwrap().raw_os_error(),
            Some(libc::ECHILD)
        );
        assert!(std::error::Error::source(&refusal).is_some());
        assert!(refusal.status().is_none());
        assert_eq!(refusal.stdout(), b"out-prefix\n");
        assert_eq!(refusal.stderr(), b"err-prefix\n");
        assert_eq!(refusal.observation()["termination_requested"], false);
        assert_eq!(refusal.observation()["reaped_by_owner"], false);
        assert!(!refusal.to_string().contains("out-prefix"));
        assert!(!format!("{refusal:?}").contains("err-prefix"));
        // Capture closed its own read ends before returning; releasing the
        // actual background writer still proves no late group signal occurred.
        let result = fixture.finish();
        std::fs::write(
            fixture.root.join("external-reap-result.json"),
            serde_json::to_vec(&serde_json::json!({
                "external_waitpid":direct_pid,"direct_exit":libc::WEXITSTATUS(native_status),
                "owner_refusal":refusal.observation(),"background_result":result
            }))
            .unwrap(),
        )
        .unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn ordinary_descendant_timeout_preserves_owned_group_retirement() {
        if !isolated_case(
            "bounded_process::tests::ordinary_descendant_timeout_preserves_owned_group_retirement",
        ) {
            return;
        }
        use std::os::unix::process::ExitStatusExt;
        let fixture = WriterFixture::new();
        let mut command = fixture.command(false);
        command.env("WORKCELL_PIPE_TIMEOUT_GROUP", "1");
        let failure = capture_bounded_process(command, Duration::from_secs(2), 4096).unwrap_err();
        assert_eq!(failure.kind(), BoundedCaptureFailureKind::DeadlineElapsed);
        assert!(failure.timed_out());
        assert_eq!(failure.status().unwrap().signal(), Some(libc::SIGKILL));
        assert!(failure.output_complete() && !failure.output_truncated());
        assert_eq!(failure.stdout(), b"out-prefix\n");
        assert_eq!(failure.stderr(), b"err-prefix\n");
        assert_eq!(failure.observation()["reaped_by_owner"], true);
        assert_eq!(failure.observation()["termination_request_accepted"], true);
        let writer: serde_json::Value =
            serde_json::from_slice(&std::fs::read(fixture.root.join("writer.json")).unwrap())
                .unwrap();
        assert_eq!(
            writer["pgid"], writer["ppid"],
            "actual descendant joined its direct parent's original group"
        );
        let pid = writer["pid"].as_i64().unwrap() as libc::pid_t;
        let deadline = std::time::Instant::now() + Duration::from_secs(3);
        let adopted_reap = std::cell::Cell::new(None::<i32>);
        loop {
            #[cfg(target_os = "linux")]
            {
                let mut status = 0;
                let reaped = unsafe { libc::waitpid(pid, &mut status, libc::WNOHANG) };
                if reaped == pid {
                    assert!(libc::WIFSIGNALED(status));
                    assert_eq!(libc::WTERMSIG(status), libc::SIGKILL);
                    adopted_reap.set(Some(status));
                } else if reaped < 0 {
                    assert_eq!(
                        std::io::Error::last_os_error().raw_os_error(),
                        Some(libc::ECHILD)
                    );
                }
            }
            let mut query = Command::new("/bin/ps");
            query
                .args(["-p", &pid.to_string(), "-o", "pid=,uid=,lstart=,stat="])
                .env("LC_ALL", "C")
                .stdin(Stdio::null())
                .stdout(Stdio::piped())
                .stderr(Stdio::piped());
            let observation = run_bounded_process(query, Duration::from_secs(1), 4096).unwrap();
            std::fs::write(
                fixture.root.join("group-last-ps.stdout"),
                &observation.stdout,
            )
            .unwrap();
            std::fs::write(
                fixture.root.join("group-last-ps.stderr"),
                &observation.stderr,
            )
            .unwrap();
            assert!(
                !observation.timed_out
                    && observation.output_complete
                    && !observation.output_truncated
            );
            if observation.status.code() == Some(1)
                && observation.stdout.is_empty()
                && observation.stderr.is_empty()
            {
                break;
            }
            assert!(
                std::time::Instant::now() < deadline,
                "actual ordinary descendant still present; no cleanup proof"
            );
            std::thread::sleep(Duration::from_millis(5));
        }
        assert!(
            !fixture.root.join("writer-result.json").exists(),
            "killed writer cannot be relabelled as a late successful output"
        );
        fixture.finished.set(true);
        std::fs::write(
            fixture.root.join("group-retirement-result.json"),
            serde_json::to_vec(&serde_json::json!({"capture_failure":failure.observation(),
                "actual_writer":writer,"adopted_wait_status":adopted_reap.get(),
                "physical_absence_observed":true}))
            .unwrap(),
        )
        .unwrap();
    }
}

// New real OS definitions share the existing coordinator-supplied native
// artifact root. No product allocator, supervisor or default system temp.
#[cfg(all(test, unix))]
pub(crate) mod status_test_support {
    use super::*;
    use sha2::{Digest, Sha256};
    use std::os::unix::fs::MetadataExt;
    use std::{
        cell::Cell,
        fs,
        path::{Path, PathBuf},
        process::Stdio,
        time::Instant,
    };
    static SEQUENCE: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    // The coordinator selects an existing owner source and placement. These
    // transient inputs qualify test artifacts; they create no product authority.
    // Optional replay qualifies disposable fixture placement with the actual
    // current owner and compiled image basis. It creates no native authority.
    pub(crate) fn admitted_artifact_root() -> std::io::Result<PathBuf> {
        admitted_artifact_root_with_replay(None)
    }

    #[cfg(target_os = "linux")]
    pub(crate) fn admitted_artifact_root_for_replay(
        compiled_source: Option<&str>,
        test_source: &[u8],
        lock: &[u8],
    ) -> std::io::Result<PathBuf> {
        admitted_artifact_root_with_replay(Some((compiled_source, test_source, lock)))
    }

    fn replay_digest_input(name: &str) -> std::io::Result<String> {
        let invalid = || {
            std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "explicit replay requires the exact admitted source/image digest",
            )
        };
        let value = std::env::var(name).map_err(|_| invalid())?;
        if value.len() != 64 || !value.bytes().all(|value| value.is_ascii_hexdigit()) {
            return Err(invalid());
        }
        Ok(value)
    }

    fn replay_image(
        path: &Path,
        context: &Path,
        expected: &str,
        actual_self: bool,
        deadline: std::time::Instant,
    ) -> std::io::Result<()> {
        use std::io::Read;
        use std::os::unix::fs::OpenOptionsExt;
        let invalid = || {
            std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "explicit replay image does not match the admitted compiled owner basis",
            )
        };
        if !path.is_absolute() || !path.starts_with(context) || path.canonicalize()? != path {
            return Err(invalid());
        }
        let mut file = fs::OpenOptions::new()
            .read(true)
            .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK | libc::O_CLOEXEC)
            .open(path)?;
        let initial = file.metadata()?;
        let identity = |metadata: &fs::Metadata| {
            (
                metadata.dev(),
                metadata.ino(),
                metadata.len(),
                metadata.mode(),
                metadata.nlink(),
                metadata.uid(),
                metadata.mtime(),
                metadata.mtime_nsec(),
                metadata.ctime(),
                metadata.ctime_nsec(),
            )
        };
        let named = fs::symlink_metadata(path)?;
        const LIMIT: u64 = 1_073_741_824;
        if !initial.is_file()
            || !named.is_file()
            || initial.mode() & 0o111 == 0
            || initial.uid() != unsafe { libc::geteuid() }
            || initial.len() > LIMIT
            || identity(&initial) != identity(&named)
        {
            return Err(invalid());
        }
        let is_actual_self = || -> std::io::Result<bool> {
            let actual = fs::metadata("/proc/self/exe")?;
            Ok((actual.dev(), actual.ino()) == (initial.dev(), initial.ino()))
        };
        if actual_self && !is_actual_self()? {
            return Err(invalid());
        }
        let mut digest = Sha256::new();
        let mut buffer = [0_u8; 65_536];
        let mut observed = 0_u64;
        loop {
            if std::time::Instant::now() >= deadline {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::TimedOut,
                    "explicit replay image observation deadline elapsed",
                ));
            }
            let count = file.read(&mut buffer)?;
            if count == 0 {
                break;
            }
            observed += count as u64;
            if observed > LIMIT {
                return Err(invalid());
            }
            digest.update(&buffer[..count]);
        }
        let named = fs::symlink_metadata(path)?;
        if std::time::Instant::now() >= deadline {
            return Err(std::io::Error::new(
                std::io::ErrorKind::TimedOut,
                "explicit replay image observation deadline elapsed",
            ));
        }
        if !named.is_file()
            || identity(&initial) != identity(&file.metadata()?)
            || identity(&initial) != identity(&named)
            || observed != initial.len()
            || path.canonicalize()? != path
            || format!("{:x}", digest.finalize()) != expected
            || (actual_self && !is_actual_self()?)
        {
            return Err(invalid());
        }
        Ok(())
    }

    fn admitted_artifact_root_with_replay(
        replay_basis: Option<(Option<&str>, &[u8], &[u8])>,
    ) -> std::io::Result<PathBuf> {
        use std::io::Read;
        use std::os::unix::fs::{MetadataExt, OpenOptionsExt};
        let invalid = |message| std::io::Error::new(std::io::ErrorKind::InvalidInput, message);
        let input = |name| {
            std::env::var_os(name).ok_or_else(|| {
                invalid(format!(
                    "required artifact admission input {name} is absent"
                ))
            })
        };
        let selected = PathBuf::from(input("WORKCELL_TEST_ARTIFACT_ROOT")?);
        let source = PathBuf::from(input("WORKCELL_TEST_ARTIFACT_OWNER_SOURCE")?);
        if !selected.is_absolute() || !source.is_absolute() {
            return Err(invalid(
                "artifact placement and owner source must be absolute".into(),
            ));
        }
        let base = selected.canonicalize()?;
        let base_metadata = fs::metadata(&base)?;
        if !base_metadata.is_dir() || base_metadata.uid() != unsafe { libc::geteuid() } {
            return Err(invalid(
                "artifact root must be an existing coordinator-owned directory".into(),
            ));
        }
        let owner_ref = std::env::var("WORKCELL_TEST_ARTIFACT_OWNER_REF")
            .map_err(|_| invalid("artifact owner reference is missing or not UTF-8".into()))?;
        let admission = std::env::var("WORKCELL_TEST_ARTIFACT_ADMISSION")
            .map_err(|_| invalid("artifact admission kind is missing or not UTF-8".into()))?;
        let mut file = fs::OpenOptions::new()
            .read(true)
            .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK | libc::O_CLOEXEC)
            .open(&source)?;
        let initial = file.metadata()?;
        if !initial.is_file() || initial.nlink() != 1 || initial.len() > 65_536 {
            return Err(invalid(
                "artifact owner source must be a bounded regular single-link file".into(),
            ));
        }
        let mut bytes = Vec::new();
        (&mut file).take(65_537).read_to_end(&mut bytes)?;
        if bytes.len() > 65_536 {
            return Err(invalid(
                "artifact owner source exceeded the admission byte limit".into(),
            ));
        }
        let identity = |m: &fs::Metadata| {
            (
                m.dev(),
                m.ino(),
                m.len(),
                m.mtime(),
                m.mtime_nsec(),
                m.ctime(),
                m.ctime_nsec(),
            )
        };
        let named = fs::symlink_metadata(&source)?;
        if !named.is_file()
            || identity(&initial) != identity(&file.metadata()?)
            || identity(&initial) != identity(&named)
            || source.canonicalize()? != source
        {
            return Err(invalid(
                "artifact owner source changed or is not the selected canonical regular source"
                    .into(),
            ));
        }
        let text = std::str::from_utf8(&bytes)
            .map_err(|error| std::io::Error::new(std::io::ErrorKind::InvalidData, error))?;
        let explicit_context = std::env::var_os("WORKCELL_TEST_CONTEXT_ROOT");
        const REPLAY_INPUTS: [&str; 5] = [
            "WORKCELL_TEST_REPLAY_SOURCE_REF",
            "WORKCELL_TEST_REPLAY_TEST_SOURCE_SHA256",
            "WORKCELL_TEST_REPLAY_LOCK_SHA256",
            "WORKCELL_TEST_REPLAY_TEST_IMAGE_SHA256",
            "WORKCELL_TEST_REPLAY_OWNER_IMAGE_SHA256",
        ];
        if explicit_context.is_none()
            && REPLAY_INPUTS
                .iter()
                .any(|name| std::env::var_os(name).is_some())
        {
            return Err(invalid(
                "explicit replay has no actual owner context".into(),
            ));
        }
        let (repository, context_witness) = if let Some(context) = explicit_context {
            if admission == "hosted-runner" {
                return Err(invalid(
                    "hosted staging cannot substitute an explicit replay owner context".into(),
                ));
            }
            let context = PathBuf::from(context);
            if !context.is_absolute() || context.canonicalize()? != context {
                return Err(invalid(
                    "explicit replay context must be an existing canonical absolute owner".into(),
                ));
            }
            let held = fs::OpenOptions::new()
                .read(true)
                .custom_flags(
                    libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_NONBLOCK | libc::O_CLOEXEC,
                )
                .open(&context)?;
            let initial = held.metadata()?;
            let named = fs::symlink_metadata(&context)?;
            if !initial.is_dir()
                || !named.is_dir()
                || (initial.dev(), initial.ino()) != (named.dev(), named.ino())
            {
                return Err(invalid(
                    "explicit replay owner affiliation is unavailable".into(),
                ));
            }
            (context, Some((held, initial)))
        } else {
            (
                PathBuf::from(env!("CARGO_MANIFEST_DIR"))
                    .parent()
                    .and_then(Path::parent)
                    .ok_or_else(|| invalid("missing compiled product repository".into()))?
                    .canonicalize()?,
                None,
            )
        };
        let mut central_context = None;
        for ancestor in repository.ancestors() {
            match fs::metadata(ancestor.join("Control/user/native-action-authority.json")) {
                Ok(metadata) if metadata.is_file() => {
                    central_context = Some(ancestor.to_path_buf());
                    break;
                }
                Ok(_) => {
                    return Err(invalid(
                        "compiled Central context marker is not a regular file".into(),
                    ))
                }
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                Err(error) => return Err(error),
            }
        }
        match admission.as_str() {
            "central-clearing" => {
                let record: serde_json::Value = serde_json::from_slice(&bytes)
                    .map_err(|error| std::io::Error::new(std::io::ErrorKind::InvalidData, error))?;
                let clearing = source
                    .parent()
                    .ok_or_else(|| invalid("missing clearing owner".into()))?;
                let central = clearing
                    .ancestors()
                    .nth(5)
                    .ok_or_else(|| invalid("missing Central root owner".into()))?;
                let id = clearing
                    .file_name()
                    .and_then(|value| value.to_str())
                    .ok_or_else(|| invalid("native clearing identity is not UTF-8".into()))?;
                let expected = central
                    .join("Control/agents/now/clearings")
                    .join(id)
                    .join("now.json");
                let source_ref = format!(
                    "central:source:control:root:Control/agents/now/clearings/{id}/now.json"
                );
                if context_witness.is_some() && repository != central {
                    return Err(invalid(
                        "explicit replay context is not the selected actual clearing World".into(),
                    ));
                }
                // An explicit replay already holds the exact native clearing World
                // above; private grant files do not establish portable World identity.
                if context_witness.is_none() && central_context.as_deref() != Some(central) {
                    return Err(invalid(
                        "clearing source is outside the actual compiled Central working context"
                            .into(),
                    ));
                }
                if source != expected
                    || owner_ref != format!("central:now:control:root:{id}")
                    || record["schema"] != "central.now-clearing/v1"
                    || record["now_ref"] != owner_ref
                    || record["source_ref"] != source_ref
                    || record["scope_ref"] != "control:root"
                    || !record["policy_revision_at_allocation"]
                        .as_str()
                        .is_some_and(|value| !value.is_empty())
                    || !record["participant_refs"]
                        .as_array()
                        .is_some_and(|values| !values.is_empty())
                {
                    return Err(invalid(
                        "selected owner source does not establish the actual root clearing allocation"
                            .into(),
                    ));
                }
                if !base.starts_with(clearing.join("T").canonicalize()?) {
                    return Err(invalid(
                        "artifact root is outside the selected actual clearing T".into(),
                    ));
                }
            }
            "product-scratch" => {
                let record: serde_json::Value = serde_json::from_slice(&bytes)
                    .map_err(|error| std::io::Error::new(std::io::ErrorKind::InvalidData, error))?;
                let project = source
                    .parent()
                    .and_then(Path::parent)
                    .ok_or_else(|| invalid("missing declared product owner".into()))?;
                let id = record["project_id"].as_str().ok_or_else(|| {
                    invalid("declared product has no native project identity".into())
                })?;
                let actual_product_context = match &central_context {
                    Some(central) => project.parent() == Some(central.join("Work").as_path()),
                    None => project == repository,
                };
                if context_witness.is_some()
                    && project != repository
                    && central_context.as_deref() != Some(repository.as_path())
                {
                    return Err(invalid(
                        "explicit context is not its actual Project or World".into(),
                    ));
                }
                if !actual_product_context {
                    return Err(invalid(
                        "product scratch source is outside the actual compiled product owner context"
                            .into(),
                    ));
                }
                if record["schema"] != "central.project/v1"
                    || id.is_empty()
                    || record["human_source"] != "ProjectCentral/user"
                    || owner_ref != format!("project:{id}")
                    || source != project.join("ProjectCentral/project.json")
                    || !base.starts_with(project.join("ProjectCentral/now/tmp").canonicalize()?)
                {
                    return Err(invalid(
                        "artifact root does not match the selected actual product scratch owner"
                            .into(),
                    ));
                }
                // This is authored product scratch, not proof of a Run allocation.
            }
            "hosted-runner" => {
                let workspace = PathBuf::from(input("GITHUB_WORKSPACE")?).canonicalize()?;
                if std::env::var("GITHUB_ACTIONS").as_deref() != Ok("true")
                    || std::env::var("CI").as_deref() != Ok("true")
                    || !std::env::var("GITHUB_RUN_ID").is_ok_and(|value| !value.is_empty())
                    || !std::env::var("GITHUB_REPOSITORY").is_ok_and(|value| !value.is_empty())
                    || workspace != repository
                    || base != workspace.join("evidence/native-processes").canonicalize()?
                    || source != workspace.join("evidence/source-commit.txt")
                    || text.trim() != owner_ref
                    || owner_ref.len() != 40
                    || !owner_ref.bytes().all(|value| value.is_ascii_hexdigit())
                {
                    return Err(invalid("artifact root is not the explicitly admitted source-pinned hosted staging exception".into()));
                }
                // CI staging is not a native Project, World, Run or NOW identity.
            }
            _ => return Err(invalid("unrecognised artifact admission kind".into())),
        }
        if let Some((held, context_initial)) = context_witness {
            let expected_source = std::env::var("WORKCELL_TEST_REPLAY_SOURCE_REF")
                .map_err(|_| invalid("explicit replay source association is absent".into()))?;
            let (compiled_source, test_source, lock) = replay_basis
                .ok_or_else(|| invalid("this fixture has no compiled replay basis".into()))?;
            let compiled_source = compiled_source
                .ok_or_else(|| invalid("image lacks compiled source association".into()))?;
            if expected_source.len() != 40
                || !expected_source
                    .bytes()
                    .all(|value| value.is_ascii_hexdigit())
                || expected_source != compiled_source
            {
                return Err(invalid("explicit replay source association differs".into()));
            }
            let expected_test_source =
                replay_digest_input("WORKCELL_TEST_REPLAY_TEST_SOURCE_SHA256")?;
            let expected_lock = replay_digest_input("WORKCELL_TEST_REPLAY_LOCK_SHA256")?;
            if format!("{:x}", Sha256::digest(test_source)) != expected_test_source
                || format!("{:x}", Sha256::digest(lock)) != expected_lock
            {
                return Err(invalid(
                    "explicit replay compiled source/lock basis differs".into(),
                ));
            }
            let expected_test = replay_digest_input("WORKCELL_TEST_REPLAY_TEST_IMAGE_SHA256")?;
            let expected_owner = replay_digest_input("WORKCELL_TEST_REPLAY_OWNER_IMAGE_SHA256")?;
            let deadline = std::time::Instant::now() + Duration::from_secs(30);
            replay_image(
                &std::env::current_exe()?,
                &repository,
                &expected_test,
                true,
                deadline,
            )?;
            let owner = PathBuf::from(input("WORKCELL_RUNTIME_PROJECTION_BIN")?);
            replay_image(&owner, &repository, &expected_owner, false, deadline)?;
            // Image observation may take time. Requalify the SAME held owner source
            // before any fixture allocation; changed/missing source never falls back.
            let owner_named = fs::symlink_metadata(&source)?;
            if !owner_named.is_file()
                || owner_named.nlink() != 1
                || identity(&initial) != identity(&file.metadata()?)
                || identity(&initial) != identity(&owner_named)
                || source.canonicalize()? != source
            {
                return Err(invalid("actual replay owner source changed".into()));
            }
            let named = fs::symlink_metadata(&repository)?;
            let current = held.metadata()?;
            if !named.is_dir()
                || (context_initial.dev(), context_initial.ino()) != (named.dev(), named.ino())
                || (context_initial.dev(), context_initial.ino()) != (current.dev(), current.ino())
                || repository.canonicalize()? != repository
            {
                return Err(invalid("explicit replay owner affiliation changed".into()));
            }
        }
        Ok(base)
    }

    pub(crate) struct Fixture {
        pub root: PathBuf,
        finished: Cell<bool>,
    }
    impl Fixture {
        pub fn new(label: &str) -> Self {
            let base = admitted_artifact_root().expect(
                "selected native/hosted artifact placement must be admitted before creation",
            );
            let root = base.join(format!(
                "null-status-{label}-{}-{}",
                std::process::id(),
                SEQUENCE.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
            ));
            fs::create_dir(&root).expect("test owns a fresh exclusive material directory");
            Self {
                root,
                finished: Cell::new(false),
            }
        }
        pub fn finish(&self) {
            self.finished.set(true);
        }
        pub fn release(&self) {
            fs::write(
                self.root.join("release"),
                b"release exact owned test writer",
            )
            .unwrap();
        }
        pub fn document(&self, name: &str, deadline: Instant) -> serde_json::Value {
            let path = self.root.join(name);
            while !path.exists() && Instant::now() < deadline {
                std::thread::sleep(Duration::from_millis(5));
            }
            serde_json::from_slice(
                &fs::read(path)
                    .expect("actual process did not publish its observation before deadline"),
            )
            .unwrap()
        }
        pub fn script(&self) -> PathBuf {
            let script = self.root.join("null-writer.py");
            fs::write(&script, NULL_WRITER).unwrap();
            script
        }
    }
    impl Drop for Fixture {
        fn drop(&mut self) {
            let released = fs::write(
                self.root.join("release"),
                b"release exact owned test writer",
            );
            if !self.finished.get() {
                let record = serde_json::json!({"release_attempted":true,"release_error":released.as_ref().err().map(ToString::to_string),
                    "physical_retirement_verified":false,"owner_panic":std::thread::panicking(),
                    "standing":"failed real fixture; finite writer self-deadline; no inferred retirement"});
                if let Err(error) = fs::write(
                    self.root.join("cleanup-uncertainty.json"),
                    record.to_string(),
                ) {
                    eprintln!(
                        "actual fixture cleanup evidence failed: {error}; retained {}",
                        self.root.display()
                    );
                    if !std::thread::panicking() {
                        panic!("actual owned fixture cleanup evidence lost");
                    }
                }
            }
            if let Err(error) = released {
                eprintln!(
                    "actual fixture release failed: {error}; retained {}",
                    self.root.display()
                );
                if !std::thread::panicking() {
                    panic!("owned writer release failed");
                }
            }
        }
    }
    // Fixture cleanup delegates to the same retained Child owner. This guard
    // neither supervises another process nor signals an unheld numeric PID.
    pub(crate) struct OwnedChild {
        child: Option<std::process::Child>,
        root: PathBuf,
    }
    impl OwnedChild {
        pub fn spawn(mut command: Command, root: &Path) -> Self {
            use std::os::unix::process::CommandExt;
            command.process_group(0);
            Self {
                child: Some(command.spawn().unwrap()),
                root: root.to_path_buf(),
            }
        }
        pub fn id(&self) -> u32 {
            self.child.as_ref().unwrap().id()
        }
        pub fn complete(
            &mut self,
            timeout: Duration,
        ) -> std::result::Result<BoundedProcessOutput, BoundedProcessFailure> {
            native::capture(
                self.child
                    .take()
                    .expect("same actual Child can be completed only once"),
                Instant::now() + timeout,
                65_536,
            )
        }
    }
    impl Drop for OwnedChild {
        fn drop(&mut self) {
            if let Some(child) = self.child.take() {
                let release = fs::write(
                    self.root.join("release"),
                    b"release original owned fixture child",
                );
                let retired =
                    native::capture(child, Instant::now() + Duration::from_secs(2), 65_536);
                let record = match &retired {
                    Ok(output) => {
                        serde_json::json!({"release_error":release.as_ref().err().map(ToString::to_string),
                        "actual_status":output.status.code(),"timed_out":output.timed_out,
                        "output_complete":output.output_complete,"output_truncated":output.output_truncated})
                    }
                    Err(failure) => {
                        serde_json::json!({"release_error":release.as_ref().err().map(ToString::to_string),
                        "retirement_failure":failure.observation()})
                    }
                };
                let saved = fs::write(
                    self.root.join("owned-child-cleanup.json"),
                    record.to_string(),
                );
                if let Err(error) = saved {
                    eprintln!("actual fixture retirement evidence failed: {error}");
                }
                if retired.is_err() || release.is_err() {
                    eprintln!(
                        "actual fixture Child retirement remains uncertain; retained {}",
                        self.root.display()
                    );
                    if !std::thread::panicking() {
                        panic!("actual fixture retirement failed");
                    }
                }
            }
        }
    }
    pub(crate) fn isolated(name: &str, timeout: Duration) -> bool {
        if std::env::var("WORKCELL_STATUS_ISOLATED_CASE").as_deref() == Ok(name) {
            #[cfg(target_os = "linux")]
            assert_eq!(
                unsafe { libc::prctl(libc::PR_SET_CHILD_SUBREAPER, 1, 0, 0, 0) },
                0
            );
            return true;
        }
        let mut command = Command::new(std::env::current_exe().unwrap());
        command
            .args(["--exact", name, "--nocapture", "--test-threads=1"])
            .env("WORKCELL_STATUS_ISOLATED_CASE", name)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        let output = capture_bounded_process(command, timeout, 65_536).unwrap();
        assert!(
            output.status.success()
                && !output.timed_out
                && output.output_complete
                && !output.output_truncated
                && String::from_utf8_lossy(&output.stdout)
                    .contains("1 passed; 0 failed; 0 ignored;"),
            "real status gate failed: stdout={} stderr={}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        false
    }
    pub(crate) fn wait_absent(
        pid: libc::pid_t,
        root: &Path,
        deadline: Instant,
        adopted: bool,
    ) -> Option<i32> {
        #[cfg(target_os = "linux")]
        let mut adopted_status = None;
        #[cfg(not(target_os = "linux"))]
        let adopted_status = None;
        loop {
            #[cfg(target_os = "linux")]
            if adopted {
                let mut status = 0;
                let reaped = unsafe { libc::waitpid(pid, &mut status, libc::WNOHANG) };
                if reaped == pid {
                    adopted_status = Some(status);
                } else if reaped < 0 {
                    assert_eq!(
                        std::io::Error::last_os_error().raw_os_error(),
                        Some(libc::ECHILD)
                    );
                } else {
                    assert_eq!(reaped, 0);
                }
            }
            #[cfg(not(target_os = "linux"))]
            let _ = adopted;
            let mut command = Command::new("/bin/ps");
            command
                .args(["-p", &pid.to_string(), "-o", "pid=,uid=,lstart=,stat="])
                .env("LC_ALL", "C")
                .stdin(Stdio::null())
                .stdout(Stdio::piped())
                .stderr(Stdio::piped());
            let output = capture_bounded_process(command, Duration::from_secs(1), 4096).unwrap();
            fs::write(root.join("actual-last-ps.stdout"), &output.stdout).unwrap();
            fs::write(root.join("actual-last-ps.stderr"), &output.stderr).unwrap();
            assert!(!output.timed_out && output.output_complete && !output.output_truncated);
            if output.status.code() == Some(1)
                && output.stdout.is_empty()
                && output.stderr.is_empty()
            {
                return adopted_status;
            }
            assert!(
                Instant::now() < deadline,
                "actual process still present, including zombie; no invented retirement"
            );
            std::thread::sleep(Duration::from_millis(5));
        }
    }
    pub(crate) const NULL_WRITER: &str = r#"import json,os,pathlib,time
root=pathlib.Path(os.environ['WORKCELL_STATUS_FIXTURE_ROOT'])
release=root/'release'
def publish(name,value):
    stage=root/(name+'.stage-'+str(os.getpid()))
    payload=json.dumps(value)
    with stage.open('x') as out:
        if os.environ.get('WORKCELL_STATUS_FACT_CHECKPOINT')==name:
            prefix=payload.index(':')+1
            out.write(payload[:prefix]);out.flush();os.fsync(out.fileno())
            publish('partial-stage-ready',{'pid':os.getpid(),'name':name})
            deadline=time.monotonic()+5
            while not release.exists() and time.monotonic()<deadline:time.sleep(.005)
            if not release.exists():raise RuntimeError('actual shared publication was not released')
            out.write(payload[prefix:])
        else:
            out.write(payload)
        out.flush();os.fsync(out.fileno())
    assert not (root/name).exists()
    os.rename(stage,root/name)
if os.environ.get('WORKCELL_STATUS_PUBLICATION_ONLY')=='1':
    publish('writer.json',{'pid':os.getpid(),'ppid':os.getppid(),'pgid':os.getpgrp(),'uid':os.getuid()})
    publish('writer-result.json',{'pid':os.getpid(),'released':release.exists()})
    os._exit(0)
child=os.fork()
if child==0:
    publish('writer.json',{'pid':os.getpid(),'ppid':os.getppid(),'pgid':os.getpgrp(),'uid':os.getuid()})
    deadline=time.monotonic()+30
    while not release.exists() and time.monotonic()<deadline:time.sleep(.005)
    written={str(fd):os.write(fd,b'late-bytes\n') for fd in (1,2)}
    publish('writer-result.json',{'pid':os.getpid(),'released':release.exists(),'writes':written})
    os._exit(0)
deadline=time.monotonic()+2
while not (root/'writer.json').exists():
    if time.monotonic()>=deadline:os._exit(3)
    time.sleep(.005)
publish('parent.json',{'pid':os.getpid(),'child':child,'pgid':os.getpgrp()})
if os.environ.get('WORKCELL_STATUS_PARENT_STAY')=='1':
    deadline=time.monotonic()+30
    while not release.exists() and time.monotonic()<deadline:time.sleep(.005)
    os._exit(4)
os._exit(0)
"#;
}

#[cfg(all(test, unix))]
mod status_only_tests {
    use super::status_test_support::{self as support, Fixture};
    use super::*;
    use std::{fs, process::Stdio, time::Instant};

    #[test]
    fn actual_null_streams_preserve_writes_arguments_environment_and_exit_status() {
        let fixture = Fixture::new("descriptor");
        let program = r#"import json,os,pathlib,stat,sys
root=pathlib.Path(os.environ['WORKCELL_STATUS_FIXTURE_ROOT'])
null=os.stat(os.devnull)
record={'args':sys.argv[1:],'environment':os.environ['WORKCELL_STATUS_VALUE'],'cwd':os.getcwd(),'streams':{}}
for fd in (1,2):
    observed=os.fstat(fd)
    record['streams'][str(fd)]={'character':stat.S_ISCHR(observed.st_mode),'rdev_matches':observed.st_rdev==null.st_rdev,'written':os.write(fd,b'x'*2097152)}
with (root/'descriptor.json').open('x') as out:json.dump(record,out);out.flush();os.fsync(out.fileno())
sys.exit(int(sys.argv[1]))
"#;
        for code in [0, 7] {
            let round = fixture.root.join(format!("exit-{code}"));
            fs::create_dir(&round).unwrap();
            let mut command = Command::new("python3");
            command
                .args([
                    "-S",
                    "-c",
                    program,
                    &code.to_string(),
                    "private-null-argument",
                ])
                .env("WORKCELL_STATUS_FIXTURE_ROOT", &round)
                .env("WORKCELL_STATUS_VALUE", "retained-value")
                .current_dir(&round)
                .stdin(Stdio::null())
                .stdout(Stdio::piped())
                .stderr(Stdio::piped());
            // The private entry must enforce actual null descriptors even when
            // caller configuration was pipes; it never pipes/discards output.
            let status = run_status_only(command, Duration::from_secs(3)).unwrap();
            assert_eq!(status.code(), Some(code));
            let observed: serde_json::Value =
                serde_json::from_slice(&fs::read(round.join("descriptor.json")).unwrap()).unwrap();
            assert_eq!(
                observed["args"],
                serde_json::json!([code.to_string(), "private-null-argument"])
            );
            assert_eq!(observed["environment"], "retained-value");
            assert_eq!(observed["cwd"].as_str().unwrap(), round.to_str().unwrap());
            for fd in ["1", "2"] {
                assert_eq!(observed["streams"][fd]["character"], true);
                assert_eq!(observed["streams"][fd]["rdev_matches"], true);
                assert_eq!(observed["streams"][fd]["written"], 2_097_152);
            }
        }
        fixture.finish();
    }

    #[test]
    fn actual_fact_publication_withholds_final_name_until_complete_bytes() {
        let fixture = Fixture::new("publication-interleaving");
        // This is the SAME publisher used by the actual writer/ECHILD cases,
        // not a second implementation of the intended stage/rename algorithm.
        let script = fixture.script();
        let mut command = Command::new("python3");
        command
            .arg("-S")
            .arg(script)
            .env("WORKCELL_STATUS_FIXTURE_ROOT", &fixture.root)
            .env("WORKCELL_STATUS_PUBLICATION_ONLY", "1")
            .env("WORKCELL_STATUS_FACT_CHECKPOINT", "writer.json")
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        let mut child = support::OwnedChild::spawn(command, &fixture.root);
        let actual_pid = child.id();
        let checkpoint = fixture.document(
            "partial-stage-ready",
            Instant::now() + Duration::from_secs(2),
        );
        assert_eq!(checkpoint["pid"], actual_pid);
        assert_eq!(checkpoint["name"], "writer.json");
        let stage = fixture.root.join(format!("writer.json.stage-{actual_pid}"));
        assert_eq!(fs::read(&stage).unwrap(), b"{\"pid\":");
        assert!(
            !fixture.root.join("writer.json").exists(),
            "in-progress fixture bytes cannot acquire the final fact name"
        );
        fixture.release();
        let observed = fixture.document("writer.json", Instant::now() + Duration::from_secs(2));
        let result = fixture.document(
            "writer-result.json",
            Instant::now() + Duration::from_secs(2),
        );
        assert_eq!(observed["pid"], actual_pid);
        assert_eq!(result["pid"], actual_pid);
        assert_eq!(result["released"], true);
        let output = child.complete(Duration::from_secs(2)).unwrap();
        assert!(
            output.status.success()
                && !output.timed_out
                && output.output_complete
                && !output.output_truncated
        );
        assert!(!stage.exists());
        fs::write(
            fixture.root.join("publication-child.stdout"),
            &output.stdout,
        )
        .unwrap();
        fs::write(
            fixture.root.join("publication-child.stderr"),
            &output.stderr,
        )
        .unwrap();
        fixture.finish();
    }

    #[test]
    fn actual_status_only_external_reap_never_signals_a_surviving_group() {
        let name="bounded_process::status_only_tests::actual_status_only_external_reap_never_signals_a_surviving_group";
        if !support::isolated(name, Duration::from_secs(12)) {
            return;
        }
        use std::os::unix::process::CommandExt;
        let fixture = Fixture::new("external-reap");
        let script = fixture.script();
        let mut command = Command::new("python3");
        command
            .arg("-S")
            .arg(script)
            .env("WORKCELL_STATUS_FIXTURE_ROOT", &fixture.root)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .process_group(0);
        let child = command.spawn().unwrap();
        let pid = child.id() as libc::pid_t;
        let deadline = Instant::now() + Duration::from_secs(3);
        let mut native_status = 0;
        loop {
            let reaped = unsafe { libc::waitpid(pid, &mut native_status, libc::WNOHANG) };
            if reaped == pid {
                break;
            }
            assert_eq!(reaped, 0);
            assert!(Instant::now() < deadline);
            std::thread::sleep(Duration::from_millis(5));
        }
        assert!(libc::WIFEXITED(native_status) && libc::WEXITSTATUS(native_status) == 0);
        let failure = native::capture_status_only(child, deadline).unwrap_err();
        assert_eq!(failure.kind(), BoundedCaptureFailureKind::WaitFailed);
        assert_eq!(
            failure.native_cause().unwrap().raw_os_error(),
            Some(libc::ECHILD)
        );
        assert!(std::error::Error::source(&failure).is_some());
        assert!(failure.status().is_none());
        assert!(
            failure.stdout().is_empty()
                && failure.stderr().is_empty()
                && !failure.output_complete()
        );
        let facts = failure.observation();
        assert_eq!(facts["output_capture_requested"], false);
        assert_eq!(facts["stdout_eof"], false);
        assert_eq!(facts["stderr_eof"], false);
        assert_eq!(facts["termination_requested"], false);
        assert_eq!(facts["reaped_by_owner"], false);
        let writer = fixture.document("writer.json", Instant::now() + Duration::from_secs(2));
        assert_eq!(writer["ppid"], pid);
        assert_eq!(writer["pgid"], pid);
        fixture.release();
        let result = fixture.document(
            "writer-result.json",
            Instant::now() + Duration::from_secs(3),
        );
        assert_eq!(result["released"], true);
        assert_eq!(result["writes"]["1"], 11);
        assert_eq!(result["writes"]["2"], 11);
        let adopted = support::wait_absent(
            writer["pid"].as_i64().unwrap() as libc::pid_t,
            &fixture.root,
            Instant::now() + Duration::from_secs(3),
            true,
        );
        #[cfg(target_os = "linux")]
        assert!(
            adopted.is_some_and(|status| libc::WIFEXITED(status) && libc::WEXITSTATUS(status) == 0)
        );
        #[cfg(not(target_os = "linux"))]
        let _ = adopted;
        fs::write(
            fixture.root.join("owner-refusal.json"),
            serde_json::to_vec(&facts).unwrap(),
        )
        .unwrap();
        fixture.finish();
    }
}
