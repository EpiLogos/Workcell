use std::{env, error::Error, process};

use epilogos_workcell_control::{ControlClient, TcpControlTransport};

fn main() {
    if let Err(error) = run() {
        eprintln!("workcell-control-client: {error}");
        process::exit(1);
    }
}

fn run() -> Result<(), Box<dyn Error>> {
    let mut endpoint = None;
    let mut authorization = env::var("WORKCELL_CONTROL_TOKEN").ok();
    let mut json = false;
    let mut command = None;

    let args = env::args().skip(1).collect::<Vec<_>>();
    let mut index = 0;
    while index < args.len() {
        match args[index].as_str() {
            "--endpoint" => {
                endpoint = Some(required_value(&args, index, "--endpoint")?.to_owned());
                index += 2;
            }
            "--authorization" => {
                authorization = Some(required_value(&args, index, "--authorization")?.to_owned());
                index += 2;
            }
            "--json" => {
                json = true;
                index += 1;
            }
            "-h" | "--help" | "help" => {
                print_help();
                return Ok(());
            }
            value if command.is_none() => {
                command = Some(value.to_owned());
                index += 1;
            }
            other => return Err(format!("unexpected argument `{other}`").into()),
        }
    }

    let endpoint = endpoint.ok_or("--endpoint HOST:PORT is required")?;
    let command = command.unwrap_or_else(|| "discover".into());
    let mut client = ControlClient::new(TcpControlTransport::new(endpoint));
    if let Some(token) = authorization {
        client = client.with_authorization(token);
    }

    let value = match command.as_str() {
        "status" => client.status()?,
        "discover" => client.discover()?,
        other => {
            return Err(
                format!("unsupported probe command `{other}`; expected status or discover").into(),
            )
        }
    };

    if json {
        println!("{}", serde_json::to_string_pretty(&value)?);
    } else if command == "status" {
        println!(
            "Workcell {} — {} ({} offers, {} providers)",
            value["workcell_ref"].as_str().unwrap_or("unknown"),
            value["health"].as_str().unwrap_or("unknown"),
            value["offers"].as_u64().unwrap_or(0),
            value["providers"].as_u64().unwrap_or(0)
        );
    } else {
        println!(
            "Workcell {} — {}",
            value["workcell_ref"].as_str().unwrap_or("unknown"),
            value["health"].as_str().unwrap_or("unknown")
        );
    }
    Ok(())
}

fn required_value<'a>(
    args: &'a [String],
    index: usize,
    flag: &str,
) -> Result<&'a str, Box<dyn Error>> {
    args.get(index + 1)
        .map(String::as_str)
        .ok_or_else(|| format!("{flag} requires a value").into())
}

fn print_help() {
    println!(
        "workcell-control-client --endpoint HOST:PORT [--authorization TOKEN] [--json] [status|discover]\n\nThis is a native deployment/control probe over workcell.control/v1. Full operation clients use epilogos-workcell-sdk::client::ControlClient. Prefer WORKCELL_CONTROL_TOKEN over passing credentials on the command line."
    );
}
