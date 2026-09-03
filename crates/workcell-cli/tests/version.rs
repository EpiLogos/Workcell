use std::process::Command;

fn run(args: &[&str]) -> std::process::Output {
    Command::new(env!("CARGO_BIN_EXE_workcell"))
        .args(args)
        .output()
        .expect("workcell binary should run")
}

#[test]
fn all_version_spellings_report_the_same_native_identity() {
    let expected = format!("workcell {}\n", env!("CARGO_PKG_VERSION"));
    for args in [["--version"], ["-V"], ["version"]] {
        let output = run(&args);
        assert!(output.status.success());
        assert_eq!(String::from_utf8(output.stdout).unwrap(), expected);
        assert!(output.stderr.is_empty());
    }
}

#[test]
fn version_probe_does_not_depend_on_remote_endpoint_selection() {
    let expected = format!("workcell {}\n", env!("CARGO_PKG_VERSION"));
    let output = run(&["--endpoint", "127.0.0.1:1", "--version"]);
    assert!(output.status.success());
    assert_eq!(String::from_utf8(output.stdout).unwrap(), expected);
    assert!(output.stderr.is_empty());
}
