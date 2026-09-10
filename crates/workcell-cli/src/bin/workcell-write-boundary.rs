use epilogos_workcell_runtime::{
    run_bounded_process, write_boundary_capabilities, PreparedWriteBoundary,
    WriteBoundaryRequirements,
};
use serde_json::{json, Value};
use std::{env, fs, process::Command, time::Duration};

fn main() {
    if env::args().nth(1).as_deref() == Some("exec") {
        if let Err(error) = protocol_exec() {
            // Stdout belongs exclusively to the hosted protocol, even on error.
            eprintln!(
                "{}",
                json!({"schema":"workcell.write-boundary-result/v1",
                    "ok":false,"executed":false,"error":error.to_string()})
            );
            std::process::exit(2);
        }
        return;
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
            value["protocol_exec"] = json!(cfg!(target_os = "linux") && value["supported"] == true);
            value["protocol_stdio"] = json!("inherited pipes/sockets only; no launcher stdout; PID preserved; owner-managed lifetime");
            return Ok((value, 0));
        }
        Some("--help" | "help") | None => {
            return Ok((
                json!({"usage":"workcell-write-boundary capabilities | inspect REQUIREMENTS.json CURRENT_POLICY_REVISION | run REQUIREMENTS.json CURRENT_POLICY_REVISION TIMEOUT_MS -- PROGRAM [ARG...] | exec PREPARATION.json CURRENT_POLICY_REVISION -- PROGRAM [ARG...]","boundary":"explicit material requirements, not governance recognition; bounded run captures output, protocol exec retains owner pipes/lifetime; unknown coverage refuses execution"}),
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

fn protocol_exec() -> Result<(), Box<dyn std::error::Error>> {
    let args = env::args().skip(1).collect::<Vec<_>>();
    if args.len() < 5 || args[3] != "--" {
        return Err("expected exec PREPARATION.json CURRENT_POLICY_REVISION -- PROGRAM [ARG...]".into());
    }
    if fs::metadata(&args[1])?.len() > 1_048_576 {
        return Err("requirements exceed 1 MiB".into());
    }
    let preparation: Value = serde_json::from_str(&fs::read_to_string(&args[1])?)?;
    if preparation["schema"] != "workcell.prepared-write-boundary/v1"
        || preparation["state"] != "prepared-not-executed"
    {
        return Err("protocol exec requires an exact native prepared-write-boundary reading".into());
    }
    let requirements = WriteBoundaryRequirements::from_json(&preparation["requirements"].to_string())?;
    let boundary = PreparedWriteBoundary::prepare(requirements, &args[2])?;
    let current = boundary.inspect(&args[2])?;
    for field in ["requirements_digest", "objects", "protected_objects"] {
        if preparation[field] != current[field] {
            return Err(format!("prepared protocol {field} changed; re-resolve before execution").into());
        }
    }
    let mut command = Command::new(&args[4]);
    command.args(&args[5..]);
    boundary.exec_protocol(command, &args[2])?;
    Ok(())
}
