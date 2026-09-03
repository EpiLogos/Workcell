use std::{env, process::ExitCode};

mod combined {
    include!("workcell.rs");

    pub(super) fn invoke() -> ExitCode {
        main()
    }
}

fn version_requested(args: &[String]) -> bool {
    let mut positional = Vec::new();
    let mut index = 0;
    while index < args.len() {
        match args[index].as_str() {
            "--endpoint" | "--authorization" => {
                if args.get(index + 1).is_none() {
                    return false;
                }
                index += 2;
            }
            argument
                if argument.starts_with("--endpoint=")
                    || argument.starts_with("--authorization=") =>
            {
                index += 1;
            }
            _ => {
                positional.push(args[index].as_str());
                index += 1;
            }
        }
    }
    matches!(positional.as_slice(), ["--version"] | ["-V"] | ["version"])
}

fn main() -> ExitCode {
    let args = env::args().skip(1).collect::<Vec<_>>();
    if version_requested(&args) {
        println!("workcell {}", env!("CARGO_PKG_VERSION"));
        return ExitCode::SUCCESS;
    }
    combined::invoke()
}

#[cfg(test)]
mod tests {
    use super::version_requested;

    fn args(values: &[&str]) -> Vec<String> {
        values.iter().map(|value| (*value).to_owned()).collect()
    }

    #[test]
    fn version_is_global_across_local_and_remote_selector_forms() {
        assert!(version_requested(&args(&["--version"])));
        assert!(version_requested(&args(&["-V"])));
        assert!(version_requested(&args(&["version"])));
        assert!(version_requested(&args(&[
            "--endpoint",
            "127.0.0.1:7788",
            "--version"
        ])));
        assert!(version_requested(&args(&[
            "--endpoint=127.0.0.1:7788",
            "version"
        ])));
        assert!(version_requested(&args(&[
            "--authorization",
            "token",
            "-V"
        ])));
    }

    #[test]
    fn ordinary_product_commands_are_not_intercepted() {
        assert!(!version_requested(&args(&["status"])));
        assert!(!version_requested(&args(&["plan", "version"])));
        assert!(!version_requested(&args(&["--endpoint"])));
    }
}
