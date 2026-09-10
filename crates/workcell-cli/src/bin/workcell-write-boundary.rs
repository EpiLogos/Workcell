use epilogos_workcell_runtime::{
    run_bounded_process, write_boundary_capabilities, PreparedWriteBoundary,
    WriteBoundaryRequirements,
};
use serde_json::{json, Value};
use std::{env, fs, process::Command, time::Duration};

fn main() {
    let protocol = env::args().nth(1).as_deref() == Some("protocol");
    match run() {
        Ok((value, code)) => {
            println!("{value}");
            std::process::exit(code);
        }
        Err(error) => {
            if protocol {
                // Never contaminate the provider's protocol stdout with an
                // implementation receipt or a JSON error from the material host.
                eprintln!("workcell.protocol_refused: {error}");
            } else {
                println!(
                    "{}",
                    json!({"schema":"workcell.write-boundary-result/v1","ok":false,"error":error.to_string(),"executed":Value::Null,"effect_state":"refused or unverified; inspect before retry"})
                );
            }
            std::process::exit(2);
        }
    }
}
fn run() -> Result<(Value, i32), Box<dyn std::error::Error>> {
    let args = env::args().skip(1).collect::<Vec<_>>();
    match args.first().map(String::as_str) {
        Some("capabilities") if args.len() == 1 => {
            let mut capabilities = write_boundary_capabilities();
            capabilities["protocol_exec"] = json!({"operation":"protocol REQUIREMENTS.json CURRENT_POLICY_REVISION -- PROGRAM [ARG...]","stdio":"one-way stdin/stdout pipes; stderr discarded; no inherited writable file descriptors","lifetime":"same process via exec, caller-owned","live_revocation":false});
            return Ok((capabilities, 0));
        }
        Some("--help" | "help") | None => {
            return Ok((
                json!({"usage":"workcell-write-boundary capabilities | inspect REQUIREMENTS.json CURRENT_POLICY_REVISION | run REQUIREMENTS.json CURRENT_POLICY_REVISION TIMEOUT_MS -- PROGRAM [ARG...] | protocol REQUIREMENTS.json CURRENT_POLICY_REVISION -- PROGRAM [ARG...]","boundary":"explicit material requirements, not governance recognition; finite run output is bounded to 64 KiB per stream; protocol exec retains only pipe stdin/stdout; unknown coverage refuses execution"}),
                0,
            ))
        }
        Some("inspect") if args.len() == 3 => {}
        Some("run") if args.len() >= 6 && args[4] == "--" => {}
        Some("protocol") if args.len() >= 5 && args[3] == "--" => {}
        _ => return Err("invalid write-boundary operation; use --help".into()),
    }
    if fs::metadata(&args[1])?.len() > 1_048_576 {
        return Err("requirements exceed 1 MiB".into());
    }
    let requirements = WriteBoundaryRequirements::from_json(&fs::read_to_string(&args[1])?)?;
    let boundary = PreparedWriteBoundary::prepare(requirements, &args[2])?;
    if args[0] == "protocol" {
        let mut command = Command::new(&args[4]);
        command.args(&args[5..]);
        boundary.exec_protocol(&mut command, &args[2])?;
        return Err("protocol exec unexpectedly returned without replacing the process".into());
    }
    let inspection = boundary.inspect(&args[2])?;
    if args[0] == "inspect" {
        return Ok((inspection, 0));
    }
    let timeout = Duration::from_millis(args[3].parse::<u64>()?);
    let mut command = Command::new(&args[5]);
    command.args(&args[6..]);
    boundary.configure_command(&mut command, &args[2])?;
    let output = run_bounded_process(command, timeout, 65536)?;
    let ok = output.status.success() && !output.timed_out;
    Ok((
        json!({"schema":"workcell.write-boundary-result/v1","ok":ok,"executed":true,
        "exit_code":output.status.code(),"timed_out":output.timed_out,
        "requirements_digest":boundary.requirements().digest(),"policy_ref":boundary.requirements().policy_ref,"policy_revision":boundary.requirements().policy_revision,
        "output_complete":output.output_complete,"output_truncated":output.output_truncated,
        "stdout":String::from_utf8_lossy(&output.stdout),"stderr":String::from_utf8_lossy(&output.stderr),"capabilities":write_boundary_capabilities()}),
        if ok { 0 } else { 1 },
    ))
}
