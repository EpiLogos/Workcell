//! A seat's landing (`workcell.lane.commit`, `.push` and `.land`; O-I #1006,
//! manual §19.16), run through `workcell action run` as the O:I registry runs
//! it: in a scratch World under a home of the test's own, whose project's
//! primary has a workcell declared by Workcell and its seat held by a lane, as
//! the seat instrument holds one. Nothing of the person's is read or written.

use std::{
    fs,
    path::{Path, PathBuf},
    process::{Command, Output},
    time::{SystemTime, UNIX_EPOCH},
};

use serde_json::{json, Value};

const REGISTER: &str = "Control/machines/workcells.register.json";

struct Scratch(PathBuf);

impl Scratch {
    fn new(label: &str) -> Self {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let path = std::env::temp_dir().join(format!(
            "workcell-landing-{label}-{}-{nonce}",
            std::process::id()
        ));
        fs::create_dir_all(&path).unwrap();
        Scratch(path.canonicalize().unwrap())
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

/// A World of the test's own: its home, its folder, its project's primary and
/// the checkout its declared seat holds.
struct World {
    scratch: Scratch,
    home: PathBuf,
    world: PathBuf,
    primary: PathBuf,
    checkout: PathBuf,
}

/// Every process the test starts reads this home, never the person's.
fn isolated(command: &mut Command, home: &Path) {
    command
        .env("HOME", home)
        .env("XDG_CONFIG_HOME", home.join(".config"))
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_TERMINAL_PROMPT", "0")
        .env_remove("GIT_DIR")
        .env_remove("GIT_WORK_TREE")
        .env_remove("GIT_INDEX_FILE")
        .env_remove("CENTRAL_ROOT");
}

fn git(cwd: &Path, home: &Path, args: &[&str]) -> String {
    let mut command = Command::new("git");
    command.args(args).current_dir(cwd);
    isolated(&mut command, home);
    let output = command.output().unwrap();
    assert!(
        output.status.success(),
        "git {args:?} in {}: {}",
        cwd.display(),
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8_lossy(&output.stdout).trim().to_owned()
}

impl World {
    fn new(label: &str, with_origin: bool) -> Self {
        let scratch = Scratch::new(label);
        let home = scratch.0.join("home");
        let world = scratch.0.join("Central");
        let primary = world.join("Work/notes");
        fs::create_dir_all(&home).unwrap();
        fs::create_dir_all(world.join("Control")).unwrap();
        fs::create_dir_all(&primary).unwrap();
        git(
            &primary,
            &home,
            &["init", "--quiet", "--initial-branch", "main"],
        );
        git(&primary, &home, &["config", "user.name", "Landing test"]);
        git(
            &primary,
            &home,
            &["config", "user.email", "landing@test.invalid"],
        );
        fs::write(primary.join("README.md"), "notes\n").unwrap();
        git(&primary, &home, &["add", "README.md"]);
        git(&primary, &home, &["commit", "--quiet", "-m", "Notes begin"]);
        if with_origin {
            let origin = scratch.0.join("notes.git");
            git(
                &scratch.0,
                &home,
                &[
                    "init",
                    "--quiet",
                    "--bare",
                    "--initial-branch",
                    "main",
                    origin.to_str().unwrap(),
                ],
            );
            git(
                &primary,
                &home,
                &["remote", "add", "origin", origin.to_str().unwrap()],
            );
            git(&primary, &home, &["push", "--quiet", "origin", "main"]);
        }
        let mut made = World {
            checkout: world.join("worktrees/env-1/notes"),
            scratch,
            home,
            world,
            primary,
        };
        let declared = answer(made.act(
            "workcell.declare",
            json!({"world": made.world, "project": "notes"}),
        ));
        made.checkout = PathBuf::from(declared["checkout"].as_str().unwrap());
        made
    }

    fn origin(&self) -> PathBuf {
        self.scratch.0.join("notes.git")
    }

    /// An Action of Workcell's field, run as the registry runs it, in this World (CENTRAL_ROOT).
    fn act(&self, id: &str, input: Value) -> Output {
        let mut command = Command::new(env!("CARGO_BIN_EXE_workcell"));
        command.args(["--json", "action", "run", id, &input.to_string()]);
        isolated(&mut command, &self.home);
        command.env("CENTRAL_ROOT", &self.world).output().unwrap()
    }

    /// The seat held by a lane as the seat instrument's join holds it: the lane's branch cut where the seat stands, and the
    /// register's seat claimed for it.
    fn hold_lane(&self, lane: &str) {
        git(
            &self.checkout,
            &self.home,
            &["switch", "--quiet", "-c", lane],
        );
        let path = self.world.join(REGISTER);
        let mut register: Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
        let seat = &mut register["workcells"][0]["seats"][0];
        seat["state"] = json!("claimed");
        seat["branch"] = json!(lane);
        seat["claimed_by"] = json!("agent:landing-test");
        fs::write(&path, serde_json::to_vec_pretty(&register).unwrap()).unwrap();
    }

    fn register(&self) -> Vec<u8> {
        fs::read(self.world.join(REGISTER)).unwrap()
    }

    fn seat(&self) -> Value {
        json!({"workcell": "env-1", "product": "notes"})
    }

    fn with(&self, key: &str, value: &str) -> Value {
        let mut input = self.seat();
        input[key] = json!(value);
        input
    }
}

fn answer(output: Output) -> Value {
    assert!(
        output.status.success(),
        "refused: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    serde_json::from_slice(&output.stdout).unwrap_or_else(|error| {
        panic!(
            "not JSON ({error}): {}",
            String::from_utf8_lossy(&output.stdout)
        )
    })
}

fn refusal(output: Output) -> String {
    assert!(
        !output.status.success(),
        "answered where a refusal was due: {}",
        String::from_utf8_lossy(&output.stdout)
    );
    String::from_utf8_lossy(&output.stderr).into_owned()
}

#[test]
fn the_field_lists_the_three_landing_acts_with_their_inputs() {
    let scratch = Scratch::new("field");
    let mut command = Command::new(env!("CARGO_BIN_EXE_workcell"));
    command.args(["actions", "--json"]);
    isolated(&mut command, &scratch.0);
    let field = answer(command.output().unwrap());
    for (id, takes) in [
        (
            "workcell.lane.commit",
            vec!["workcell", "product", "message"],
        ),
        ("workcell.lane.push", vec!["workcell", "product"]),
        ("workcell.lane.land", vec!["workcell", "product", "as"]),
    ] {
        let action = field["actions"]
            .as_array()
            .unwrap()
            .iter()
            .find(|action| action["id"] == id)
            .unwrap_or_else(|| panic!("{id} is not in Workcell's Action field"));
        let required = action["inputs"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|input| input["required"] == true)
            .map(|input| input["name"].as_str().unwrap())
            .collect::<Vec<_>>();
        assert_eq!(required, takes, "{id}");
        assert!(action["inputs"]
            .as_array()
            .unwrap()
            .iter()
            .any(|input| input["name"] == "world" && input["required"] == false));
    }
}

#[test]
fn a_lane_is_committed_pushed_and_landed_as_a_merge_into_its_remotes_main() {
    let world = World::new("remote", true);
    world.hold_lane("lane-1");
    let register = world.register();
    let nothing = refusal(world.act("workcell.lane.commit", world.with("message", "Nothing yet")));
    assert!(nothing.contains("nothing to commit"), "{nothing}");

    fs::write(world.checkout.join("NOTES.md"), "The lane's first note.\n").unwrap();
    let committed = answer(world.act(
        "workcell.lane.commit",
        world.with("message", "The lane's first note"),
    ));
    let head = git(&world.checkout, &world.home, &["rev-parse", "HEAD"]);
    assert_eq!(
        (committed["act"].as_str(), committed["head"].as_str()),
        (Some("commit"), Some(head.as_str()))
    );
    assert_eq!(
        (committed["branch"].as_str(), committed["files"].as_u64()),
        (Some("lane-1"), Some(1))
    );
    assert_eq!(
        git(&world.checkout, &world.home, &["log", "-1", "--format=%s"]),
        "The lane's first note"
    );
    assert_eq!(
        git(&world.checkout, &world.home, &["status", "--porcelain"]),
        ""
    );

    for land_as in ["mr", "merge"] {
        let early = refusal(world.act("workcell.lane.land", world.with("as", land_as)));
        assert!(early.contains("push it first"), "{land_as}: {early}");
    }
    let pushed = answer(world.act("workcell.lane.push", world.seat()));
    assert_eq!(pushed["upstream"], "origin/lane-1");
    assert_eq!(
        git(
            &world.origin(),
            &world.home,
            &["rev-parse", "refs/heads/lane-1"]
        ),
        head
    );

    let draft = refusal(world.act("workcell.lane.land", world.with("as", "draft")));
    assert!(draft.contains("is not on GitHub"), "{draft}");
    let merged = answer(world.act("workcell.lane.land", world.with("as", "merge")));
    assert_eq!(
        (merged["merged_into"].as_str(), merged["ahead"].as_u64()),
        (Some("origin/main"), Some(1))
    );
    assert_eq!(
        git(
            &world.origin(),
            &world.home,
            &["rev-parse", "refs/heads/main"]
        ),
        head
    );
    assert_eq!(
        git(&world.checkout, &world.home, &["rev-parse", "origin/main"]),
        head
    );
    let again = refusal(world.act("workcell.lane.land", world.with("as", "merge")));
    assert!(again.contains("nothing to land"), "{again}");
    assert_eq!(
        world.register(),
        register,
        "a landing never writes the register: the seat instrument keeps it"
    );
}

#[test]
fn a_lane_lands_in_the_primarys_own_main_where_the_project_has_no_remote() {
    let world = World::new("local", false);
    world.hold_lane("lane-1");
    fs::write(
        world.checkout.join("NOTES.md"),
        "A note that stays on this machine.\n",
    )
    .unwrap();
    let committed = answer(world.act(
        "workcell.lane.commit",
        world.with("message", "A local note"),
    ));
    let push = refusal(world.act("workcell.lane.push", world.seat()));
    assert!(push.contains("has no remote"), "{push}");
    let mr = refusal(world.act("workcell.lane.land", world.with("as", "mr")));
    assert!(mr.contains("has no remote"), "{mr}");
    let merged = answer(world.act("workcell.lane.land", world.with("as", "merge")));
    assert_eq!(merged["merged_into"], "main");
    assert_eq!(
        git(&world.primary, &world.home, &["rev-parse", "main"]),
        committed["head"].as_str().unwrap()
    );
    assert!(
        world.primary.join("NOTES.md").is_file(),
        "the primary stands on the main the lane landed in"
    );
}

#[test]
fn a_landing_is_refused_in_words_where_no_lane_holds_the_seat_or_no_world_is_named() {
    let world = World::new("refused", false);
    let free = refusal(world.act("workcell.lane.commit", world.with("message", "m")));
    assert!(free.contains("seat is free"), "{free}");

    let mut unnamed = Command::new(env!("CARGO_BIN_EXE_workcell"));
    unnamed.args([
        "--json",
        "action",
        "run",
        "workcell.lane.push",
        &world.seat().to_string(),
    ]);
    isolated(&mut unnamed, &world.home);
    let unnamed = refusal(unnamed.output().unwrap());
    assert!(unnamed.contains("no World is named"), "{unnamed}");

    world.hold_lane("lane-1");
    let elsewhere = refusal(world.act(
        "workcell.lane.push",
        json!({"workcell": "env-9", "product": "notes"}),
    ));
    assert!(elsewhere.contains("names no workcell env-9"), "{elsewhere}");
    let squash = refusal(world.act("workcell.lane.land", world.with("as", "squash")));
    assert!(squash.contains("Land as is draft, mr or merge"), "{squash}");
    let empty = refusal(world.act("workcell.lane.commit", world.with("message", "  ")));
    assert!(empty.contains("its message is empty"), "{empty}");
    git(
        &world.checkout,
        &world.home,
        &["switch", "--quiet", "-c", "elsewhere"],
    );
    let moved = refusal(world.act("workcell.lane.commit", world.with("message", "m")));
    assert!(moved.contains("the register's lane is lane-1"), "{moved}");
}
