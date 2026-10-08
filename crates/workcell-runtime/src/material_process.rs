//! Native material execution handshake. The existing durable owner records
//! both trusted waiting processes before allowing untrusted exec. A retained
//! identity is observation, never authority to signal a recycled PID.
use epilogos_workcell_core::{Result, WorkcellError};
use serde_json::{json, Value};
use std::{
    path::Path,
    process::Command,
    time::{Duration, Instant},
};

pub const MATERIAL_PROCESS_GUARDIAN_FLAG: &str = "--owned-material-process-guardian";
pub const MATERIAL_PROCESS_COMMAND_FLAG: &str = "--owned-material-command";

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum MaterialProcessRole {
    Guardian,
    Command,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct MaterialProcessIdentity {
    pub role: MaterialProcessRole,
    pub pid: u32,
    pub process_group: i32,
    pub birth_marker: String,
}

impl MaterialProcessIdentity {
    pub fn value(&self) -> Value {
        json!({"schema":"workcell.material-process-identity/v1", "role": match self.role {MaterialProcessRole::Guardian=>"guardian",MaterialProcessRole::Command=>"command"}, "pid":self.pid,"process_group":self.process_group,"birth_marker":self.birth_marker})
    }
    pub fn from_value(value: &Value) -> Result<Self> {
        let invalid = || {
            WorkcellError::InvalidDemand("invalid retained native material process identity".into())
        };
        if value["schema"] != "workcell.material-process-identity/v1" {
            return Err(invalid());
        }
        let role = match value["role"].as_str() {
            Some("guardian") => MaterialProcessRole::Guardian,
            Some("command") => MaterialProcessRole::Command,
            _ => return Err(invalid()),
        };
        let pid = value["pid"]
            .as_u64()
            .and_then(|pid| u32::try_from(pid).ok())
            .filter(|pid| *pid > 1 && *pid <= i32::MAX as u32)
            .ok_or_else(invalid)?;
        let process_group = value["process_group"]
            .as_i64()
            .and_then(|pg| i32::try_from(pg).ok())
            .filter(|pg| *pg == pid as i32)
            .ok_or_else(invalid)?;
        let birth_marker = value["birth_marker"]
            .as_str()
            .filter(|s| !s.is_empty() && s.len() <= 256)
            .ok_or_else(invalid)?
            .into();
        Ok(Self {
            role,
            pid,
            process_group,
            birth_marker,
        })
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum MaterialProcessIdentityState {
    Absent,
    PresentSame,
    GenerationChanged,
}
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct MaterialProcessObservation {
    pub identity_state: MaterialProcessIdentityState,
    pub original_group_absent: bool,
}

pub fn material_process_identity_state(
    identity: &MaterialProcessIdentity,
) -> Result<MaterialProcessObservation> {
    #[cfg(any(target_os = "linux", target_os = "macos"))]
    {
        native::observe(identity)
    }
    #[cfg(not(any(target_os = "linux", target_os = "macos")))]
    {
        let _ = identity;
        Err(WorkcellError::Unsupported(
            "native material process observation is unsupported on this platform".into(),
        ))
    }
}

/// Called only by the trusted native binary's internal entrypoint.
pub fn material_process_guardian_main() -> Result<()> {
    #[cfg(any(target_os = "linux", target_os = "macos"))]
    {
        native::guardian()
    }
    #[cfg(not(any(target_os = "linux", target_os = "macos")))]
    {
        Err(WorkcellError::Unsupported(
            "native material guardian is unsupported on this platform".into(),
        ))
    }
}
/// The command trampoline stays trusted and waiting until its identity is
/// durably retained. A successful exec preserves that exact PID/birth/group.
pub fn material_process_command_main() -> Result<()> {
    #[cfg(any(target_os = "linux", target_os = "macos"))]
    {
        native::command_trampoline()
    }
    #[cfg(not(any(target_os = "linux", target_os = "macos")))]
    {
        Err(WorkcellError::Unsupported(
            "native material command trampoline is unsupported on this platform".into(),
        ))
    }
}

pub(crate) fn execute_guarded(
    executable: &Path,
    command: Command,
    timeout: Duration,
    output_limit: usize,
    recorder: &mut dyn FnMut(&MaterialProcessIdentity) -> Result<()>,
    boundary: Option<(&crate::PreparedWriteBoundary, &str)>,
    authority_expiry_unix_ms: Option<u64>,
) -> Result<Value> {
    #[cfg(any(target_os = "linux", target_os = "macos"))]
    {
        native::execute(
            executable,
            command,
            timeout,
            output_limit,
            recorder,
            boundary,
            authority_expiry_unix_ms,
        )
    }
    #[cfg(not(any(target_os = "linux", target_os = "macos")))]
    {
        let _ = (
            executable,
            command,
            timeout,
            output_limit,
            recorder,
            boundary,
            authority_expiry_unix_ms,
        );
        Err(WorkcellError::Unsupported(
            "native material guardian is unsupported on this platform; no effect".into(),
        ))
    }
}

#[cfg(any(target_os = "linux", target_os = "macos"))]
mod native {
    use super::*;
    use std::{
        os::{
            fd::{AsRawFd, RawFd},
            unix::process::CommandExt,
        },
        process::Stdio,
        thread,
        time::{SystemTime, UNIX_EPOCH},
    };
    const REQUEST_LIMIT: usize = 65_536;
    const IDENTITY_LIMIT: usize = 4096;
    const CLEANUP_GRACE: Duration = Duration::from_secs(2);
    fn safe(error: std::io::Error) -> WorkcellError {
        WorkcellError::OperationFailed(format!("native material process I/O failed: kind={:?}, os={:?}; retained operation requires reconciliation",error.kind(),error.raw_os_error()))
    }
    fn refusal(detail: &str) -> WorkcellError {
        WorkcellError::OperationFailed(format!(
            "native material process {detail}; retained operation requires reconciliation"
        ))
    }
    fn remaining(deadline: Instant) -> Result<Duration> {
        deadline
            .checked_duration_since(Instant::now())
            .filter(|left| !left.is_zero())
            .ok_or_else(|| refusal("deadline elapsed"))
    }
    fn nonblocking(fd: RawFd) -> Result<()> {
        let current = unsafe { libc::fcntl(fd, libc::F_GETFL) };
        if current < 0 || unsafe { libc::fcntl(fd, libc::F_SETFL, current | libc::O_NONBLOCK) } < 0
        {
            return Err(safe(std::io::Error::last_os_error()));
        }
        Ok(())
    }
    fn read_line(fd: RawFd, deadline: Instant, limit: usize) -> Result<Vec<u8>> {
        nonblocking(fd)?;
        let mut bytes = Vec::new();
        loop {
            remaining(deadline)?;
            let mut byte = 0u8;
            let rc = unsafe { libc::read(fd, (&mut byte as *mut u8).cast(), 1) };
            if rc == 1 {
                if byte == b'\n' {
                    return Ok(bytes);
                }
                if bytes.len() == limit {
                    return Err(refusal("protocol byte limit exceeded"));
                }
                bytes.push(byte);
            } else if rc == 0 {
                return Err(refusal("protocol closed before a complete gated record"));
            } else {
                let error = std::io::Error::last_os_error();
                if !matches!(
                    error.kind(),
                    std::io::ErrorKind::WouldBlock | std::io::ErrorKind::Interrupted
                ) {
                    return Err(safe(error));
                }
                thread::sleep(Duration::from_millis(2));
            }
        }
    }
    fn write_record(fd: RawFd, value: &Value, deadline: Instant, limit: usize) -> Result<()> {
        let mut bytes =
            serde_json::to_vec(value).map_err(|_| refusal("protocol serialization failed"))?;
        if bytes.len() > limit {
            return Err(refusal("protocol byte limit exceeded"));
        }
        bytes.push(b'\n');
        nonblocking(fd)?;
        let mut offset = 0;
        while offset < bytes.len() {
            remaining(deadline)?;
            let rc =
                unsafe { libc::write(fd, bytes[offset..].as_ptr().cast(), bytes.len() - offset) };
            if rc > 0 {
                offset += rc as usize;
            } else if rc == 0 {
                return Err(refusal("protocol write returned zero"));
            } else {
                let error = std::io::Error::last_os_error();
                if !matches!(
                    error.kind(),
                    std::io::ErrorKind::WouldBlock | std::io::ErrorKind::Interrupted
                ) {
                    return Err(safe(error));
                }
                thread::sleep(Duration::from_millis(2));
            }
        }
        Ok(())
    }
    fn read_record(fd: RawFd, deadline: Instant, limit: usize) -> Result<Value> {
        serde_json::from_slice(&read_line(fd, deadline, limit)?)
            .map_err(|_| refusal("protocol record is not valid JSON"))
    }

    #[cfg(target_os = "linux")]
    fn process_fact(pid: u32) -> Result<Option<(String, i32)>> {
        let text = match std::fs::read_to_string(format!("/proc/{pid}/stat")) {
            Ok(text) => text,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(error) => return Err(safe(error)),
        };
        if text.len() > 16_384 {
            return Err(refusal("native process fact exceeds kernel record bound"));
        }
        let fields = text
            .rsplit_once(") ")
            .ok_or_else(|| refusal("native process fact is unreadable"))?
            .1
            .split_whitespace()
            .collect::<Vec<_>>();
        let group = fields
            .get(2)
            .and_then(|s| s.parse::<i32>().ok())
            .ok_or_else(|| refusal("native process group is unreadable"))?;
        let birth = fields
            .get(19)
            .filter(|s| s.chars().all(|ch| ch.is_ascii_digit()))
            .ok_or_else(|| refusal("native process birth is unreadable"))?;
        let boot = std::fs::read_to_string("/proc/sys/kernel/random/boot_id").map_err(safe)?;
        if boot.len() > 64 {
            return Err(refusal("native boot fact exceeds kernel record bound"));
        }
        Ok(Some((
            format!("linux.proc-stat/v1:{}:{}", boot.trim(), birth),
            group,
        )))
    }
    #[cfg(target_os = "macos")]
    fn process_fact(pid: u32) -> Result<Option<(String, i32)>> {
        let mut info: libc::proc_bsdinfo = unsafe { std::mem::zeroed() };
        let size = std::mem::size_of::<libc::proc_bsdinfo>() as i32;
        let rc = unsafe {
            libc::proc_pidinfo(
                pid as i32,
                libc::PROC_PIDTBSDINFO,
                0,
                (&mut info as *mut libc::proc_bsdinfo).cast(),
                size,
            )
        };
        if rc == size && info.pbi_pid == pid {
            return Ok(Some((
                format!(
                    "darwin.proc-bsdinfo/v1:{}:{}",
                    info.pbi_start_tvsec, info.pbi_start_tvusec
                ),
                info.pbi_pgid as i32,
            )));
        }
        // A failed proc read does not itself prove absence.
        if unsafe { libc::kill(pid as i32, 0) } < 0
            && std::io::Error::last_os_error().raw_os_error() == Some(libc::ESRCH)
        {
            return Ok(None);
        }
        Err(refusal(
            "native process birth is unreadable while PID remains observable",
        ))
    }
    fn identity(pid: u32, role: MaterialProcessRole) -> Result<MaterialProcessIdentity> {
        let (birth_marker, process_group) = process_fact(pid)?.ok_or_else(|| {
            refusal("trusted waiting process disappeared before identity retention")
        })?;
        if process_group != pid as i32 {
            return Err(refusal(
                "trusted waiting process has no exclusively owned original group",
            ));
        }
        Ok(MaterialProcessIdentity {
            role,
            pid,
            process_group,
            birth_marker,
        })
    }
    pub(super) fn observe(
        selected: &MaterialProcessIdentity,
    ) -> Result<MaterialProcessObservation> {
        MaterialProcessIdentity::from_value(&selected.value())?;
        let identity_state = match process_fact(selected.pid)? {
            None => MaterialProcessIdentityState::Absent,
            Some((birth, group))
                if birth == selected.birth_marker && group == selected.process_group =>
            {
                MaterialProcessIdentityState::PresentSame
            }
            Some(_) => MaterialProcessIdentityState::GenerationChanged,
        };
        let rc = unsafe { libc::kill(-selected.process_group, 0) };
        let original_group_absent = if rc == 0 {
            false
        } else {
            let error = std::io::Error::last_os_error();
            match error.raw_os_error() {
                Some(libc::ESRCH) => true,
                Some(libc::EPERM) => false,
                _ => return Err(safe(error)),
            }
        };
        Ok(MaterialProcessObservation {
            identity_state,
            original_group_absent,
        })
    }
    fn unix_ms() -> Result<u64> {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_millis() as u64)
            .map_err(|_| refusal("clock precedes epoch"))
    }
    fn safe_environment() -> Vec<(String, Option<String>)> {
        ["PATH", "LANG", "LC_ALL", "LC_CTYPE", "TZ"]
            .into_iter()
            .filter_map(|key| {
                std::env::var(key)
                    .ok()
                    .map(|value| (key.into(), Some(value)))
            })
            .collect()
    }
    fn wire_command(command: &Command, timeout: Duration, limit: usize) -> Result<Value> {
        let text = |value: &std::ffi::OsStr| {
            value.to_str().map(str::to_owned).ok_or_else(|| {
                WorkcellError::InvalidDemand(
                    "native material command contains non-UTF8 source; no command effect".into(),
                )
            })
        };
        let program = text(command.get_program())?;
        let args = command.get_args().map(text).collect::<Result<Vec<_>>>()?;
        let cwd = command
            .get_current_dir()
            .map(|p| text(p.as_os_str()))
            .transpose()?;
        let mut environment = safe_environment();
        let explicit_environment = command
            .get_envs()
            .map(|(name, value)| Ok((text(name)?, value.map(text).transpose()?)))
            .collect::<Result<Vec<_>>>()?;
        environment.extend(explicit_environment);
        Ok(
            json!({"schema":"workcell.material-command-gate/v1","program":program,"args":args,"cwd":cwd,"environment":environment,"deadline_unix_ms":unix_ms()?.saturating_add(timeout.as_millis() as u64),"output_limit_bytes":limit}),
        )
    }
    fn gate_deadline(packet: &Value) -> Result<Instant> {
        if packet["schema"] != "workcell.material-command-gate/v1" {
            return Err(refusal("command gate schema differs"));
        }
        let left = packet["deadline_unix_ms"]
            .as_u64()
            .ok_or_else(|| refusal("command gate has no deadline"))?
            .checked_sub(unix_ms()?)
            .filter(|left| *left > 0 && *left <= 60_000)
            .ok_or_else(|| refusal("command gate deadline is expired or unbounded"))?;
        Ok(Instant::now() + Duration::from_millis(left))
    }
    fn unpack_command(packet: &Value) -> Result<Command> {
        let program = packet["program"]
            .as_str()
            .filter(|s| !s.is_empty())
            .ok_or_else(|| refusal("command gate has no program"))?;
        let args = packet["args"]
            .as_array()
            .ok_or_else(|| refusal("command gate args are unreadable"))?;
        let mut cmd = if !packet["write_boundary"].is_null() {
            let requirements =
                crate::WriteBoundaryRequirements::from_json(&packet["write_boundary"].to_string())?;
            let revision = packet["write_policy_revision"]
                .as_str()
                .ok_or_else(|| refusal("native write policy basis is absent"))?;
            let boundary = crate::PreparedWriteBoundary::prepare(requirements, revision)?;
            boundary.command(program, revision)?
        } else {
            Command::new(program)
        };
        cmd.env_clear();
        for arg in args {
            cmd.arg(
                arg.as_str()
                    .ok_or_else(|| refusal("command gate arg is unreadable"))?,
            );
        }
        match &packet["cwd"] {
            Value::Null => {}
            Value::String(cwd) => {
                cmd.current_dir(cwd);
            }
            _ => return Err(refusal("command gate cwd is unreadable")),
        }
        for pair in packet["environment"]
            .as_array()
            .ok_or_else(|| refusal("command gate environment is unreadable"))?
        {
            let values = pair
                .as_array()
                .filter(|p| p.len() == 2)
                .ok_or_else(|| refusal("command gate environment tuple is unreadable"))?;
            let name = values[0]
                .as_str()
                .ok_or_else(|| refusal("command gate environment name is unreadable"))?;
            match &values[1] {
                Value::Null => {
                    cmd.env_remove(name);
                }
                Value::String(value) => {
                    cmd.env(name, value);
                }
                _ => return Err(refusal("command gate environment value is unreadable")),
            }
        }
        cmd.stdin(Stdio::null());
        Ok(cmd)
    }
    pub(super) fn command_trampoline() -> Result<()> {
        let packet = read_record(
            libc::STDIN_FILENO,
            Instant::now() + Duration::from_secs(60),
            REQUEST_LIMIT,
        )?;
        gate_deadline(&packet)?;
        let mut cmd = unpack_command(&packet)?;
        gate_deadline(&packet)?;
        let error = cmd.exec();
        Err(safe(error))
    }
    pub(super) fn guardian() -> Result<()> {
        #[cfg(target_os = "linux")]
        if unsafe { libc::prctl(libc::PR_SET_CHILD_SUBREAPER, 1, 0, 0, 0) } != 0 {
            return Err(safe(std::io::Error::last_os_error()));
        }
        let executable = std::env::current_exe().map_err(safe)?;
        let startup = Instant::now() + Duration::from_secs(60);
        write_record(
            libc::STDOUT_FILENO,
            &identity(std::process::id(), MaterialProcessRole::Guardian)?.value(),
            startup,
            IDENTITY_LIMIT,
        )?;
        let packet = read_record(libc::STDIN_FILENO, startup, REQUEST_LIMIT)?;
        let deadline = gate_deadline(&packet)?;
        let limit = packet["output_limit_bytes"]
            .as_u64()
            .filter(|n| *n > 0 && *n <= 1_048_576)
            .ok_or_else(|| refusal("command gate output bound is invalid"))?
            as usize;
        // Body remains trusted and blocked; exact identity is retained before exec.
        let mut child = Command::new(executable)
            .arg(MATERIAL_PROCESS_COMMAND_FLAG)
            .process_group(0)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(safe)?;
        let mut body_gate = child
            .stdin
            .take()
            .ok_or_else(|| refusal("command gate pipe is absent"))?;
        let admitted = (|| {
            write_record(
                libc::STDOUT_FILENO,
                &identity(child.id(), MaterialProcessRole::Command)?.value(),
                deadline,
                IDENTITY_LIMIT,
            )?;
            let ack = read_record(libc::STDIN_FILENO, deadline, IDENTITY_LIMIT)?;
            if ack != json!({"execute":true}) {
                return Err(refusal("exact command gate acknowledgement is absent"));
            }
            write_record(body_gate.as_raw_fd(), &packet, deadline, REQUEST_LIMIT)
        })();
        drop(body_gate);
        nonblocking(libc::STDIN_FILENO)?;
        // Even a refused/disconnected gate retains and retires the actual Child.
        // Closing the original owner lifeline is cancellation, never replay.
        let captured = crate::bounded_process::capture_material_child(
            child,
            if admitted.is_ok() {
                deadline
            } else {
                Instant::now()
            },
            limit / 12,
            libc::STDIN_FILENO,
        );
        if let Err(error) = admitted {
            let _ = captured;
            return Err(error);
        }
        let result = match captured {
            Ok(output) => {
                use std::os::unix::process::ExitStatusExt;
                let stdout = String::from_utf8(output.stdout)
                    .map_err(|_| refusal("completed stdout is not UTF-8"))?;
                let stderr = String::from_utf8(output.stderr)
                    .map_err(|_| refusal("completed stderr is not UTF-8"))?;
                json!({"ok":true,"exit_code":output.status.code(),"exit_signal":output.status.signal(),"success":output.status.success(),"stdout":stdout,"stderr":stderr,"output_complete":output.output_complete,"output_truncated":output.output_truncated,"cleanup_observation":output.cleanup_observation})
            }
            Err(error) => json!({"ok":false,"failure":error.observation()}),
        };
        write_record(
            libc::STDOUT_FILENO,
            &result,
            Instant::now() + CLEANUP_GRACE,
            limit.saturating_add(8192),
        )
    }

    /// Runs only the trusted guardian before recorder completion. Untrusted
    /// command bytes use private pipes and are never error formatting material.
    pub(super) fn execute(
        executable: &Path,
        command: Command,
        timeout: Duration,
        limit: usize,
        recorder: &mut dyn FnMut(&MaterialProcessIdentity) -> Result<()>,
        boundary: Option<(&crate::PreparedWriteBoundary, &str)>,
        authority_expiry_unix_ms: Option<u64>,
    ) -> Result<Value> {
        if timeout.is_zero() || timeout > Duration::from_secs(60) || limit == 0 || limit > 1_048_576
        {
            return Err(WorkcellError::InvalidDemand(
                "native material guardian limits refused before effect".into(),
            ));
        }
        let deadline = Instant::now() + timeout;
        let mut packet = wire_command(&command, timeout, limit)?;
        if let Some(expiry) = authority_expiry_unix_ms {
            let original = packet["deadline_unix_ms"]
                .as_u64()
                .expect("native command packet has deadline");
            packet["deadline_unix_ms"] = json!(original.min(expiry));
            gate_deadline(&packet)?;
        }
        if let Some((boundary, revision)) = boundary {
            boundary.revalidate(revision)?;
            packet["write_boundary"] = boundary.requirements().as_json();
            packet["write_policy_revision"] = json!(revision);
        }
        if serde_json::to_vec(&packet)
            .map_err(|_| refusal("command gate serialization failed"))?
            .len()
            > REQUEST_LIMIT
        {
            return Err(WorkcellError::InvalidDemand(
                "native material command gate exceeds byte budget; no effect".into(),
            ));
        }
        let mut child = Command::new(executable)
            .arg(MATERIAL_PROCESS_GUARDIAN_FLAG)
            .env_clear()
            .envs(
                safe_environment()
                    .into_iter()
                    .filter_map(|(name, value)| value.map(|value| (name, value))),
            )
            .process_group(0)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .map_err(safe)?;
        let mut lifeline = child
            .stdin
            .take()
            .ok_or_else(|| refusal("guardian lifeline is absent"))?;
        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| refusal("guardian response pipe is absent"))?;
        let mut command_identity = None;
        let result = (|| {
            let received = MaterialProcessIdentity::from_value(&read_record(
                stdout.as_raw_fd(),
                deadline,
                IDENTITY_LIMIT,
            )?)?;
            let actual = identity(child.id(), MaterialProcessRole::Guardian)?;
            if received != actual {
                return Err(refusal(
                    "guardian identity differs from retained native Child",
                ));
            }
            recorder(&actual)?;
            write_record(lifeline.as_raw_fd(), &packet, deadline, REQUEST_LIMIT)?;
            let body = MaterialProcessIdentity::from_value(&read_record(
                stdout.as_raw_fd(),
                deadline,
                IDENTITY_LIMIT,
            )?)?;
            if body.role != MaterialProcessRole::Command
                || identity(body.pid, MaterialProcessRole::Command)? != body
                || body.pid == actual.pid
            {
                return Err(refusal(
                    "command trampoline identity differs from native waiting process",
                ));
            }
            command_identity = Some(body.clone());
            recorder(&body)?;
            write_record(
                lifeline.as_raw_fd(),
                &json!({"execute":true}),
                deadline,
                IDENTITY_LIMIT,
            )?;
            let result = read_record(
                stdout.as_raw_fd(),
                deadline + CLEANUP_GRACE,
                limit.saturating_add(8192),
            )?;
            if result["ok"] != true {
                return Err(refusal(&format!(
                    "capture was not completed: {}",
                    result["failure"]
                )));
            }
            let observed = observe(&body)?;
            if !observed.original_group_absent {
                return Err(refusal("command group extinction is not observed"));
            }
            Ok(result)
        })();
        // Closing is the guardian cancellation request. Never signal a recorded
        // command/group from this restarted/indirect owner.
        drop(lifeline);
        drop(stdout);
        let cleanup_deadline = Instant::now() + CLEANUP_GRACE;
        let mut status = None;
        while Instant::now() < cleanup_deadline {
            match child.try_wait() {
                Ok(Some(observed)) => {
                    status = Some(observed);
                    break;
                }
                Ok(None) => thread::sleep(Duration::from_millis(2)),
                Err(error) => return Err(safe(error)),
            }
        }
        if status.is_none() {
            // The exact retained direct Child is still owned. Killing the
            // guardian does not imply its recorded command was retired.
            child.kill().map_err(safe)?;
            let deadline = Instant::now() + CLEANUP_GRACE;
            while Instant::now() < deadline {
                match child.try_wait() {
                    Ok(Some(_)) => break,
                    Ok(None) => thread::sleep(Duration::from_millis(2)),
                    Err(error) => return Err(safe(error)),
                }
            }
            return Err(refusal(
                "guardian retirement was uncertain; command identity remains pending",
            ));
        }
        if result.is_ok() && !status.is_some_and(|s| s.success()) {
            return Err(refusal(
                "guardian returned nonzero after captured command; inspect retained identities",
            ));
        }
        if let Some(body) = command_identity {
            if result.is_ok() && !observe(&body)?.original_group_absent {
                return Err(refusal(
                    "command group is observable after guardian retirement",
                ));
            }
        }
        result
    }
}

#[cfg(all(test, any(target_os = "linux", target_os = "macos")))]
mod tests {
    use super::*;
    use crate::bounded_process::status_test_support::{isolated, Fixture};
    use std::{
        fs,
        io::Write,
        path::PathBuf,
        process::{Child, Stdio},
        thread,
    };

    fn executable() -> PathBuf {
        let path = PathBuf::from(
            std::env::var_os("WORKCELL_TEST_NATIVE_EXECUTABLE")
                .expect("guardian regression requires actual built native workcell binary"),
        );
        assert!(path.is_absolute() && path.is_file());
        fs::canonicalize(path).unwrap()
    }
    fn record(
        root: &Path,
        identities: &mut Vec<MaterialProcessIdentity>,
        identity: &MaterialProcessIdentity,
    ) -> Result<()> {
        identities.push(identity.clone());
        let staged = root.join("identities.next");
        let mut file = fs::File::create(&staged).unwrap();
        file.write_all(
            serde_json::to_vec(
                &identities
                    .iter()
                    .map(MaterialProcessIdentity::value)
                    .collect::<Vec<_>>(),
            )
            .unwrap()
            .as_slice(),
        )
        .unwrap();
        file.sync_all().unwrap();
        fs::rename(staged, root.join("identities.json")).unwrap();
        fs::File::open(root).unwrap().sync_all().unwrap();
        Ok(())
    }
    fn read_identities(root: &Path) -> Vec<MaterialProcessIdentity> {
        let values: Vec<Value> =
            serde_json::from_slice(&fs::read(root.join("identities.json")).unwrap()).unwrap();
        values
            .iter()
            .map(|value| MaterialProcessIdentity::from_value(value).unwrap())
            .collect()
    }
    fn wait_file(root: &Path, name: &str) -> Value {
        let deadline = Instant::now() + Duration::from_secs(10);
        let file = root.join(name);
        while !file.is_file() && Instant::now() < deadline {
            thread::sleep(Duration::from_millis(5));
        }
        serde_json::from_slice(&fs::read(file).unwrap()).unwrap()
    }
    fn wait_owned(child: &mut Child) -> std::process::ExitStatus {
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            match child.try_wait().unwrap() {
                Some(status) => return status,
                None if Instant::now() < deadline => thread::sleep(Duration::from_millis(5)),
                None => panic!("actual test owner retirement exceeded finite budget"),
            }
        }
    }
    fn wait_groups_absent(identities: &[MaterialProcessIdentity]) {
        let deadline = Instant::now() + Duration::from_secs(5);
        for identity in identities {
            loop {
                #[cfg(target_os = "linux")]
                {
                    // This isolated test subreaper owns only its actual orphaned helper.
                    let rc = unsafe {
                        libc::waitpid(identity.pid as i32, std::ptr::null_mut(), libc::WNOHANG)
                    };
                    if rc < 0 {
                        assert_eq!(
                            std::io::Error::last_os_error().raw_os_error(),
                            Some(libc::ECHILD)
                        );
                    }
                }
                let reading = material_process_identity_state(identity).unwrap();
                if reading.original_group_absent {
                    break;
                }
                assert!(
                    Instant::now() < deadline,
                    "actual original group remains: {identity:?} {reading:?}"
                );
                thread::sleep(Duration::from_millis(5));
            }
        }
    }
    fn body(root: &Path, success: bool) -> Command {
        let mut command = Command::new("python3");
        command.args(["-S","-c", if success {r#"import json,os,pathlib,subprocess,time
root=pathlib.Path(os.environ['WORKCELL_GUARDIAN_CASE_ROOT'])
assert len(json.loads((root/'identities.json').read_text()))==2
child=subprocess.Popen(['python3','-S','-c','import time;time.sleep(20)'],stdin=subprocess.DEVNULL,stdout=subprocess.DEVNULL,stderr=subprocess.DEVNULL)
staged=root/'body.next';staged.write_text(json.dumps({'pid':os.getpid(),'pgid':os.getpgrp(),'child':child.pid}));staged.rename(root/'body.json')
print('source-bound-real-native-result',flush=True)
"#} else {r#"import json,os,pathlib,time
root=pathlib.Path(os.environ['WORKCELL_GUARDIAN_CASE_ROOT'])
identities=json.loads((root/'identities.json').read_text());assert len(identities)==2
if os.environ.get('WORKCELL_GUARDIAN_MOVE_GROUP')=='1':os.setpgid(0,identities[0]['pid'])
with (root/'effect-count').open('a') as stream:stream.write('once\n');stream.flush();os.fsync(stream.fileno())
staged=root/'body.next';staged.write_text(json.dumps({'pid':os.getpid(),'pgid':os.getpgrp()}));staged.rename(root/'body.json')
while not (root/'release-command').exists():time.sleep(.01)
print('released-owned-native-command',flush=True)
"#}]).env("WORKCELL_GUARDIAN_CASE_ROOT",root);
        command
    }

    #[test]
    fn actual_recorded_native_gate_refuses_missing_journal_and_retires_closed_descendants() {
        const NAME:&str="material_process::tests::actual_recorded_native_gate_refuses_missing_journal_and_retires_closed_descendants";
        if !isolated(NAME, Duration::from_secs(30)) {
            return;
        }
        let native = executable();
        for accept in [false, true] {
            let fixture = Fixture::new(if accept {
                "guardian-success"
            } else {
                "guardian-refusal"
            });
            let mut identities = Vec::new();
            let result = execute_guarded(
                &native,
                body(&fixture.root, true),
                Duration::from_secs(10),
                65_536,
                &mut |identity| {
                    record(&fixture.root, &mut identities, identity)?;
                    if !accept && identity.role == MaterialProcessRole::Command {
                        return Err(WorkcellError::OperationFailed(
                            "test-owned journal refused before command gate".into(),
                        ));
                    }
                    Ok(())
                },
                None,
                None,
            );
            assert_eq!(identities.len(), 2);
            if accept {
                let result = result.unwrap();
                assert_eq!(result["success"], true);
                assert_eq!(result["stdout"], "source-bound-real-native-result\n");
                assert_eq!(result["output_truncated"], false);
                let actual = wait_file(&fixture.root, "body.json");
                assert_eq!(actual["pid"], identities[1].pid);
                assert_eq!(actual["pgid"], identities[1].process_group);
                assert!(unsafe { libc::kill(actual["child"].as_i64().unwrap() as i32, 0) } < 0);
                assert_eq!(
                    std::io::Error::last_os_error().raw_os_error(),
                    Some(libc::ESRCH)
                );
            } else {
                assert!(result.unwrap_err().to_string().contains("journal refused"));
                assert!(!fixture.root.join("body.json").exists());
            }
            wait_groups_absent(&identities);
            fixture.finish();
        }
    }

    fn crash_case(name: &str, kill_guardian: bool, move_group: bool) {
        if std::env::var("WORKCELL_GUARDIAN_CHILD_CASE").as_deref() == Ok(name) {
            let root = PathBuf::from(std::env::var_os("WORKCELL_GUARDIAN_SHARED_ROOT").unwrap());
            let mut identities = Vec::new();
            let mut command = body(&root, false);
            if move_group {
                command.env("WORKCELL_GUARDIAN_MOVE_GROUP", "1");
            }
            let result = execute_guarded(
                &executable(),
                command,
                Duration::from_secs(30),
                65_536,
                &mut |identity| record(&root, &mut identities, identity),
                None,
                None,
            );
            if kill_guardian {
                assert!(result.is_err());
                let command = identities
                    .iter()
                    .find(|identity| identity.role == MaterialProcessRole::Command)
                    .unwrap();
                let observed = material_process_identity_state(command).unwrap();
                if move_group {
                    assert_eq!(
                        observed.identity_state,
                        MaterialProcessIdentityState::GenerationChanged
                    );
                    assert!(
                        observed.original_group_absent,
                        "moved group aperture must be exercised"
                    );
                } else {
                    assert!(
                        !observed.original_group_absent,
                        "guardian loss alone must not imply child extinction"
                    );
                }
                let next = root.join("outcome.next");
                fs::write(
                    &next,
                    serde_json::to_vec(&json!({"uncertain":true,"command_group_present":true}))
                        .unwrap(),
                )
                .unwrap();
                fs::rename(next, root.join("outcome.json")).unwrap();
            } else {
                panic!("crash test owner should be killed before native completion");
            }
            return;
        }
        if !isolated(name, Duration::from_secs(45)) {
            return;
        }
        let fixture = Fixture::new(if kill_guardian {
            "guardian-killed"
        } else {
            "owner-killed"
        });
        let mut owner = Command::new(std::env::current_exe().unwrap())
            .args(["--exact", name, "--nocapture", "--test-threads=1"])
            .env("WORKCELL_GUARDIAN_CHILD_CASE", name)
            .env("WORKCELL_GUARDIAN_SHARED_ROOT", &fixture.root)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        let actual = wait_file(&fixture.root, "body.json");
        let identities = read_identities(&fixture.root);
        assert_eq!(identities.len(), 2);
        assert_eq!(actual["pid"], identities[1].pid);
        if kill_guardian {
            // Native birth and group still match the retained trusted guardian.
            let guardian = &identities[0];
            assert_eq!(
                material_process_identity_state(guardian)
                    .unwrap()
                    .identity_state,
                MaterialProcessIdentityState::PresentSame
            );
            assert_eq!(unsafe { libc::kill(guardian.pid as i32, libc::SIGKILL) }, 0);
            let outcome = wait_file(&fixture.root, "outcome.json");
            assert_eq!(outcome["uncertain"], true);
            let reading = material_process_identity_state(&identities[1]).unwrap();
            if move_group {
                assert_eq!(
                    reading.identity_state,
                    MaterialProcessIdentityState::GenerationChanged
                );
                assert!(reading.original_group_absent);
            } else {
                assert!(!reading.original_group_absent);
            }
            assert!(wait_owned(&mut owner).success());
            // Test-owned cooperative release, no restarted owner numeric signal.
            fs::write(
                fixture.root.join("release-command"),
                b"release own test command",
            )
            .unwrap();
        } else {
            owner.kill().unwrap();
            assert!(!wait_owned(&mut owner).success());
        }
        wait_groups_absent(&identities);
        assert_eq!(
            fs::read_to_string(fixture.root.join("effect-count")).unwrap(),
            "once\n"
        );
        fixture.finish();
    }
    #[test]
    fn actual_owner_crash_closes_lifeline_and_retires_recorded_command_without_replay() {
        crash_case("material_process::tests::actual_owner_crash_closes_lifeline_and_retires_recorded_command_without_replay",false,false);
    }
    #[test]
    fn actual_guardian_crash_preserves_live_command_uncertainty() {
        crash_case(
            "material_process::tests::actual_guardian_crash_preserves_live_command_uncertainty",
            true,
            false,
        );
    }
    #[test]
    fn actual_guardian_crash_with_moved_group_keeps_original_absence_uncertain() {
        crash_case("material_process::tests::actual_guardian_crash_with_moved_group_keeps_original_absence_uncertain",true,true);
    }
    #[test]
    fn actual_command_environment_excludes_server_authority_and_retains_explicit_child_binding() {
        const NAME:&str="material_process::tests::actual_command_environment_excludes_server_authority_and_retains_explicit_child_binding";
        if !isolated(NAME, Duration::from_secs(20)) {
            return;
        }
        let fixture = Fixture::new("guardian-environment");
        // Controlled canary, never a credential. Only this isolated test process changes env.
        std::env::set_var("CENTRAL_NATIVE_TOKEN", "PRIVATE-SERVER-CANARY");
        std::env::set_var("SSH_AUTH_SOCK", "PRIVATE-SERVER-SOCKET-CANARY");
        std::env::set_var("OPENAI_API_KEY", "PRIVATE-PROVIDER-CANARY");
        let mut command = Command::new("python3");
        command.args(["-S","-c","import json,os;print(json.dumps({'native':os.environ.get('CENTRAL_NATIVE_TOKEN'),'ssh':os.environ.get('SSH_AUTH_SOCK'),'provider':os.environ.get('OPENAI_API_KEY'),'child':os.environ['EXPLICIT_CHILD_BINDING']}),flush=True)"]).env("EXPLICIT_CHILD_BINDING","source-bound-child-value");
        let mut identities = Vec::new();
        let result = execute_guarded(
            &executable(),
            command,
            Duration::from_secs(10),
            65_536,
            &mut |identity| record(&fixture.root, &mut identities, identity),
            None,
            None,
        )
        .unwrap();
        let observed: Value = serde_json::from_str(result["stdout"].as_str().unwrap()).unwrap();
        assert_eq!(
            observed,
            json!({"native":null,"ssh":null,"provider":null,"child":"source-bound-child-value"})
        );
        assert!(!result.to_string().contains("PRIVATE-"));
        wait_groups_absent(&identities);
        fixture.finish();
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn actual_gated_command_preserves_native_write_boundary_after_exec() {
        const NAME:&str="material_process::tests::actual_gated_command_preserves_native_write_boundary_after_exec";
        if !isolated(NAME, Duration::from_secs(25)) {
            return;
        }
        let fixture = Fixture::new("guardian-protected-write");
        let writable = fixture.root.join("T");
        fs::create_dir(&writable).unwrap();
        let protected = fixture.root.join("candidate.txt");
        fs::write(&protected, b"exact selected source").unwrap();
        let revision = "test-owned-actual-native-boundary-r1";
        let requirements = crate::WriteBoundaryRequirements {
            policy_ref: "test-owned-native-material-source".into(),
            policy_revision: revision.into(),
            authority_ref: "test-owned-execution".into(),
            writable_paths: vec![writable.clone()],
            protected_paths: vec![fixture.root.clone(), protected.clone()],
            required_coverage: crate::WRITE_BOUNDARY_COVERAGE
                .iter()
                .map(|s| (*s).into())
                .collect(),
            expires_at_unix_ms: std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_millis() as u64
                + 60_000,
        };
        let boundary = crate::PreparedWriteBoundary::prepare(requirements, revision).unwrap();
        let mut command = Command::new("python3");
        command
            .args([
                "-S",
                "-c",
                r#"import json,pathlib,sys
pathlib.Path(sys.argv[1]).write_text('actual admitted output')
try:pathlib.Path(sys.argv[2]).write_text('forbidden replacement');denied=False
except PermissionError:denied=True
print(json.dumps({'protected_denied':denied}),flush=True)
"#,
            ])
            .arg(writable.join("output.txt"))
            .arg(&protected);
        let mut identities = Vec::new();
        let result = execute_guarded(
            &executable(),
            command,
            Duration::from_secs(10),
            65_536,
            &mut |identity| record(&fixture.root, &mut identities, identity),
            Some((&boundary, revision)),
            None,
        )
        .unwrap();
        assert_eq!(result["success"], true);
        assert_eq!(
            serde_json::from_str::<Value>(result["stdout"].as_str().unwrap()).unwrap()
                ["protected_denied"],
            true
        );
        assert_eq!(fs::read(protected).unwrap(), b"exact selected source");
        assert_eq!(
            fs::read(writable.join("output.txt")).unwrap(),
            b"actual admitted output"
        );
        wait_groups_absent(&identities);
        fixture.finish();
    }
    #[test]
    fn actual_authority_expiry_during_journal_retention_never_executes_command() {
        const NAME:&str="material_process::tests::actual_authority_expiry_during_journal_retention_never_executes_command";
        if !isolated(NAME, Duration::from_secs(25)) {
            return;
        }
        let fixture = Fixture::new("guardian-expired-admission");
        let mut identities = Vec::new();
        let expiry = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_millis() as u64
            + 5_000;
        let result = execute_guarded(
            &executable(),
            body(&fixture.root, true),
            Duration::from_secs(15),
            65_536,
            &mut |identity| {
                record(&fixture.root, &mut identities, identity)?;
                if identity.role == MaterialProcessRole::Command {
                    thread::sleep(Duration::from_millis(5_100));
                }
                Ok(())
            },
            None,
            Some(expiry),
        );
        assert!(result.is_err());
        assert_eq!(identities.len(), 2);
        assert!(!fixture.root.join("body.json").exists());
        wait_groups_absent(&identities);
        fixture.finish();
    }
}
