//! Real native protocol exec. Pipe bytes and provider PID are observed, not a
//! fabricated material receipt. Unsupported hosts exercise refusal only.
use epilogos_workcell_runtime::{write_boundary_capabilities, WriteBoundaryRequirements};
use serde_json::{json, Value};
use std::{
    fs,
    io::{BufRead, BufReader, Write},
    path::PathBuf,
    process::{Command, Stdio},
    time::{SystemTime, UNIX_EPOCH},
};

struct World(PathBuf);
impl World {
    fn new() -> Self {
        let root = std::env::temp_dir().join(format!(
            "workcell-protocol-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir_all(root.join("T")).unwrap();
        fs::write(root.join("human.txt"), "untouched human source").unwrap();
        let request = WriteBoundaryRequirements {
            policy_ref: "central:source:policy".into(),
            policy_revision: "revision:one".into(),
            authority_ref: "authority:controlled-task".into(),
            writable_paths: vec![root.join("T")],
            protected_paths: vec![root.join("human.txt")],
            required_coverage: vec!["file-content".into(), "file-creation".into()],
            expires_at_unix_ms: u64::MAX,
        };
        fs::write(root.join("request.json"), request.as_json().to_string()).unwrap();
        Self(root)
    }
    fn command(&self) -> Command {
        let mut command = Command::new(env!("CARGO_BIN_EXE_workcell-write-boundary"));
        command.arg("protocol").arg(self.0.join("request.json"));
        command.args(["revision:one", "--", "python3", "-u", "-c"]);
        command.arg(r#"
import json, os, sys
from pathlib import Path
root = Path(sys.argv[1])
for line in sys.stdin:
    request = json.loads(line)
    try:
        (root/'human.txt').write_text('forbidden')
    except PermissionError:
        denied = True
    else:
        denied = False
    (root/'T'/str(request['sequence'])).write_text('actual artifact')
    print(json.dumps({'pid': os.getpid(), 'sequence': request['sequence'], 'denied': denied}), flush=True)
"#);
        command.arg(&self.0);
        command
    }
}
impl Drop for World {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

#[test]
fn protocol_keeps_real_bidirectional_pipes_and_confines_the_provider_itself() {
    let world = World::new();
    let caps = write_boundary_capabilities();
    if caps["supported"] != true {
        let output = world.command().stdin(Stdio::piped()).output().unwrap();
        assert!(!output.status.success());
        assert!(output.stdout.is_empty());
        assert_ne!(
            std::env::var("WORKCELL_REQUIRE_LANDLOCK").ok().as_deref(),
            Some("1"),
            "{caps}"
        );
        return;
    }
    let mut child = world
        .command()
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let mut input = child.stdin.take().unwrap();
    let mut output = BufReader::new(child.stdout.take().unwrap());
    for sequence in 0..2 {
        writeln!(input, "{}", json!({"sequence":sequence})).unwrap();
        input.flush().unwrap();
        let mut line = String::new();
        output.read_line(&mut line).unwrap();
        let response: Value = serde_json::from_str(&line).expect("native protocol output only");
        assert_eq!(response["pid"], child.id());
        assert_eq!(response["sequence"], sequence);
        assert_eq!(response["denied"], true);
        assert_eq!(
            fs::read_to_string(world.0.join("T").join(sequence.to_string())).unwrap(),
            "actual artifact"
        );
    }
    drop(input);
    assert!(child.wait().unwrap().success());
    assert_eq!(
        fs::read_to_string(world.0.join("human.txt")).unwrap(),
        "untouched human source"
    );
    eprintln!("PROTOCOL_BOUNDARY_EXECUTED: actual provider PID, two bidirectional turns, protected source unchanged");
}

#[test]
fn regular_file_input_is_not_a_protocol_pipe_and_never_starts_the_body() {
    let world = World::new();
    let output = world
        .command()
        .stdin(fs::File::open(world.0.join("human.txt")).unwrap())
        .output()
        .unwrap();
    assert!(!output.status.success());
    assert!(output.stdout.is_empty());
    assert_eq!(fs::read_dir(world.0.join("T")).unwrap().count(), 0);
}
