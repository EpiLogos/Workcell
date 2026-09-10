//! Output of an explicitly requested bounded command, not passive telemetry.
use epilogos_workcell_core::{Result, WorkcellError};
use std::{
    io::Read,
    process::{Child, Command, ExitStatus},
    sync::mpsc,
    thread,
    time::{Duration, Instant},
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
/// returned prefix so excessive output cannot deadlock execution. A descendant
/// retaining a pipe cannot hold the caller indefinitely; incomplete output is
/// reported explicitly. This does not promise containment of escaped daemons.
pub fn run_bounded_process(
    mut command: Command,
    timeout: Duration,
    output_limit: usize,
) -> Result<BoundedProcessOutput> {
    if timeout.is_zero() || timeout > Duration::from_secs(60) || output_limit > 1_048_576 {
        return Err(WorkcellError::InvalidDemand(
            "command limits require 0 < timeout <= 60s and output <= 1 MiB".into(),
        ));
    }
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        command.process_group(0);
    }
    let mut child = command
        .spawn()
        .map_err(|e| WorkcellError::OperationFailed(format!("spawn bounded command: {e}")))?;
    let out = child.stdout.take();
    let err = child.stderr.take();
    if out.is_none() || err.is_none() {
        terminate(&mut child);
        let _ = child.wait();
        return Err(WorkcellError::InvalidDemand(
            "bounded command requires piped stdout and stderr".into(),
        ));
    }
    let readers = [
        reader(out.unwrap(), output_limit),
        reader(err.unwrap(), output_limit),
    ];
    let deadline = Instant::now() + timeout;
    let (status, timed_out) = loop {
        match child.try_wait() {
            Ok(Some(status)) => break (status, false),
            Ok(None) if Instant::now() < deadline => thread::sleep(Duration::from_millis(5)),
            Ok(None) => {
                terminate(&mut child);
                break (child.wait().map_err(process_error)?, true);
            }
            Err(e) => {
                terminate(&mut child);
                let _ = child.wait();
                return Err(process_error(e));
            }
        }
    };
    let mut output_complete = true;
    let mut output_truncated = false;
    let mut outputs = Vec::new();
    for receiver in readers {
        match receiver.recv_timeout(Duration::from_millis(100)) {
            Ok((bytes, truncated, complete)) => {
                outputs.push(bytes);
                output_truncated |= truncated;
                output_complete &= complete;
            }
            Err(_) => {
                outputs.push(Vec::new());
                output_complete = false;
            }
        }
    }
    Ok(BoundedProcessOutput {
        status,
        timed_out,
        stdout: outputs.remove(0),
        stderr: outputs.remove(0),
        output_truncated,
        output_complete,
    })
}
fn reader(
    mut pipe: impl Read + Send + 'static,
    limit: usize,
) -> mpsc::Receiver<(Vec<u8>, bool, bool)> {
    let (send, receive) = mpsc::sync_channel(1);
    thread::spawn(move || {
        let mut output = Vec::new();
        let mut chunk = [0u8; 8192];
        let mut truncated = false;
        let mut complete = true;
        loop {
            match pipe.read(&mut chunk) {
                Ok(0) => break,
                Ok(n) => {
                    let keep = n.min(limit.saturating_sub(output.len()));
                    output.extend_from_slice(&chunk[..keep]);
                    truncated |= keep < n;
                }
                Err(e) if e.kind() == std::io::ErrorKind::Interrupted => {}
                Err(_) => {
                    complete = false;
                    break;
                }
            }
        }
        let _ = send.send((output, truncated, complete));
    });
    receive
}
fn terminate(child: &mut Child) {
    #[cfg(unix)]
    unsafe {
        libc::kill(-(child.id() as i32), libc::SIGKILL);
    }
    let _ = child.kill();
}
fn process_error(e: std::io::Error) -> WorkcellError {
    WorkcellError::OperationFailed(format!("bounded command wait: {e}"))
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
}
