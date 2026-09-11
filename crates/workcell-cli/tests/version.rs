use std::process::Command;

fn run(args: &[&str]) -> std::process::Output {
    Command::new(env!("CARGO_BIN_EXE_workcell"))
        .args(args)
        .output()
        .expect("workcell binary should run")
}

fn assert_version_line(stdout: &str) {
    let plain = format!("workcell {}\n", env!("CARGO_PKG_VERSION"));
    let stamped = format!("workcell {} (", env!("CARGO_PKG_VERSION"));
    assert!(
        stdout == plain || stdout.starts_with(&stamped),
        "version must be '<pkg-version>' or '<pkg-version> (<build-revision>)': {stdout:?}"
    );
    if let Some(revision) = stdout
        .trim()
        .strip_prefix(&stamped)
        .and_then(|rest| rest.strip_suffix(')'))
    {
        assert!(
            !revision.is_empty() && revision.chars().all(|c| c.is_ascii_hexdigit()),
            "build revision must be non-empty hex: {revision:?}"
        );
    }
}

#[test]
fn all_version_spellings_report_the_same_native_identity() {
    for args in [["--version"], ["-V"], ["version"]] {
        let output = run(&args);
        assert!(output.status.success());
        assert_version_line(&String::from_utf8(output.stdout).unwrap());
        assert!(output.stderr.is_empty());
    }
}

#[test]
fn version_probe_does_not_depend_on_remote_endpoint_selection() {
    let output = run(&["--endpoint", "127.0.0.1:1", "--version"]);
    assert!(output.status.success());
    assert_version_line(&String::from_utf8(output.stdout).unwrap());
    assert!(output.stderr.is_empty());
}
