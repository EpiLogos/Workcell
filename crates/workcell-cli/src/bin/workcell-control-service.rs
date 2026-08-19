use std::{env, error::Error, path::PathBuf};

use epilogos_workcell_control::{ControlService, TcpControlServer};
use epilogos_workcell_core::WorkcellRef;
use epilogos_workcell_runtime::{CollapsedLocalConfig, CollapsedLocalWorkcell};

const DEFAULT_LISTEN: &str = "127.0.0.1:7777";
const DEFAULT_WORKCELL_REF: &str = "workcell:local";

fn main() -> Result<(), Box<dyn Error>> {
    let mut listen = DEFAULT_LISTEN.to_owned();
    let mut state_root = env::var_os("WORKCELL_STATE_ROOT")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(".workcell-state"));
    let mut workcell_ref = DEFAULT_WORKCELL_REF.to_owned();
    let mut authorization = env::var("WORKCELL_CONTROL_TOKEN").ok();

    let args = env::args().skip(1).collect::<Vec<_>>();
    let mut index = 0;
    while index < args.len() {
        match args[index].as_str() {
            "--listen" => {
                listen = required_value(&args, index, "--listen")?.to_owned();
                index += 2;
            }
            "--state-root" => {
                state_root = PathBuf::from(required_value(&args, index, "--state-root")?);
                index += 2;
            }
            "--workcell-ref" => {
                workcell_ref = required_value(&args, index, "--workcell-ref")?.to_owned();
                index += 2;
            }
            "--authorization" => {
                authorization = Some(required_value(&args, index, "--authorization")?.to_owned());
                index += 2;
            }
            "-h" | "--help" | "help" => {
                print_help();
                return Ok(());
            }
            other => return Err(format!("unknown argument `{other}`").into()),
        }
    }

    if authorization.is_none() && !is_loopback_listener(&listen) {
        return Err(
            "non-loopback Workcell Control Service listeners require WORKCELL_CONTROL_TOKEN or --authorization"
                .into(),
        );
    }

    let workcell = CollapsedLocalWorkcell::new(CollapsedLocalConfig::new(
        WorkcellRef::new(workcell_ref)?,
        state_root,
    ))?;
    let service = match authorization {
        Some(token) => ControlService::new(workcell).with_authorization(token),
        None => ControlService::new(workcell),
    };
    let mut server = TcpControlServer::bind(&listen, service)?;
    eprintln!(
        "Workcell Control Service listening on {} using {}",
        server.local_addr()?,
        epilogos_workcell_control::CONTROL_PROTOCOL_VERSION
    );
    server.serve()?;
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

fn is_loopback_listener(address: &str) -> bool {
    address.starts_with("127.")
        || address.starts_with("localhost:")
        || address.starts_with("[::1]:")
}

fn print_help() {
    println!(
        "workcell-control-service [--listen HOST:PORT] [--state-root PATH] [--workcell-ref REF] [--authorization TOKEN]\n\nRemote/non-loopback listeners require an authorization token. Prefer WORKCELL_CONTROL_TOKEN over passing the token on the command line."
    );
}
