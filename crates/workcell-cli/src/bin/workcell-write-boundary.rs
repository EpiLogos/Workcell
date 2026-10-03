use epilogos_workcell_runtime::{
    capture_bounded_process, write_boundary_capabilities, PreparedWriteBoundary,
    WriteBoundaryRequirements,
};
use serde_json::{json, Value};
use std::{env, fs, time::Duration};

#[path = "../stdio_boundary.rs"]
mod stdio_boundary;

fn main() {
    let args = env::args().skip(1).collect::<Vec<_>>();
    if matches!(args.first().map(String::as_str), Some("exec" | "exec-runtime")) {
        if let Err(error) = stdio_boundary::execute(&args) {
            let mut result = json!({"schema":"workcell.write-boundary-result/v1","ok":false,"error":error.to_string(),"executed":false});
            if let Some(projection) = error.downcast_ref::<epilogos_workcell_runtime::RuntimeProjectionFailure>() {
                result["runtime_projection"] = projection.as_json();
            }
            eprintln!("{result}");
        }
        std::process::exit(2);
    }
    match run() {
        Ok((value, code)) => {
            println!("{value}");
            std::process::exit(code);
        }
        Err(error) => {
            println!(
                "{}",
                json!({"schema":"workcell.write-boundary-result/v1","ok":false,"error":error.to_string(),"executed":Value::Null,"effect_state":"refused or unverified; inspect before retry"})
            );
            std::process::exit(2);
        }
    }
}
fn run() -> Result<(Value, i32), Box<dyn std::error::Error>> {
    let args = env::args().skip(1).collect::<Vec<_>>();
    match args.first().map(String::as_str) {
        Some("capabilities") if args.len() == 1 => {
            let mut value = write_boundary_capabilities();
            value["protocol_exec"] = json!({"operation":stdio_boundary::USAGE,"stdin_stdout":"inherited pipes or sockets only","provider_stderr":"inherited only for pipe/socket diagnostic channel; otherwise discarded to null; caller owns capture and limits","session_lifetime":"owned by calling protocol host","limits":"no live revocation; admission is checked before exec"});
            return Ok((value, 0));
        }
        Some("--help" | "help") | None => {
            return Ok((
                json!({"usage":"workcell-write-boundary capabilities | inspect REQUIREMENTS.json CURRENT_POLICY_REVISION | run REQUIREMENTS.json CURRENT_POLICY_REVISION TIMEOUT_MS -- PROGRAM [ARG...]","protocol_exec":stdio_boundary::USAGE,"runtime_exec":stdio_boundary::RUNTIME_USAGE,"boundary":"explicit material requirements, not governance recognition; finite output is bounded to 64 KiB per stream; protocol exec preserves provider stdout; unknown coverage refuses execution"}),
                0,
            ))
        }
        Some("inspect") if args.len() == 3 => {}
        Some("run") if args.len() >= 6 && args[4] == "--" => {}
        _ => return Err("invalid write-boundary operation; use --help".into()),
    }
    if fs::metadata(&args[1])?.len() > 1_048_576 {
        return Err("requirements exceed 1 MiB".into());
    }
    let requirements = WriteBoundaryRequirements::from_json(&fs::read_to_string(&args[1])?)?;
    let boundary = PreparedWriteBoundary::prepare(requirements, &args[2])?;
    let inspection = boundary.inspect(&args[2])?;
    if args[0] == "inspect" {
        return Ok((inspection, 0));
    }
    let timeout = Duration::from_millis(args[3].parse::<u64>()?);
    let mut command = boundary.command(&args[5], &args[2])?;
    command.args(&args[6..]);
    let output = match capture_bounded_process(command, timeout, 65536) {
        Ok(output) => output,
        Err(failure) => {
            let facts = failure.observation();
            // This explicit command response is for the caller that authorised
            // capture. Generic Error formatting never carries these bytes.
            let value = json!({"schema":"workcell.write-boundary-result/v1",
                "ok":false,"executed":facts["executed"],
                "exit_code":facts["exit_code"],"status_observed":facts["status_observed"],
                "timed_out":failure.timed_out(),"output_complete":failure.output_complete(),
                "output_truncated":failure.output_truncated(),"capture_failure":facts,
                "error":failure.to_string(),"automatic_retry":false,
                "requirements_digest":boundary.requirements().digest(),
                "policy_ref":boundary.requirements().policy_ref,
                "policy_revision":boundary.requirements().policy_revision,
                "effect_state":"native capture failed; command effect unverified; inspect before retry",
                "stdout":String::from_utf8_lossy(failure.stdout()),
                "stderr":String::from_utf8_lossy(failure.stderr()),
                "stdout_bytes":failure.stdout(),"stderr_bytes":failure.stderr(),
                "capabilities":write_boundary_capabilities()});
            return Ok((value, if failure.status().is_some() { 1 } else { 2 }));
        }
    };
    let ok = output.status.success()
        && !output.timed_out
        && output.output_complete
        && !output.output_truncated;
    Ok((
        json!({"schema":"workcell.write-boundary-result/v1","ok":ok,"executed":true,
        "exit_code":output.status.code(),"timed_out":output.timed_out,
        "requirements_digest":boundary.requirements().digest(),"policy_ref":boundary.requirements().policy_ref,"policy_revision":boundary.requirements().policy_revision,
        "output_complete":output.output_complete,"output_truncated":output.output_truncated,
        "automatic_retry":false,"effect_state":if ok {"command exited successfully with complete capture"} else {"command executed; result unverified; inspect before retry"},
        "stdout":String::from_utf8_lossy(&output.stdout),"stderr":String::from_utf8_lossy(&output.stderr),"capabilities":write_boundary_capabilities()}),
        if ok { 0 } else { 1 },
    ))
}
