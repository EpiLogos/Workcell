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

/// git under the test's home, its output as it is: where git may refuse (a conflicted merge does), or where the output's
/// leading space counts (`git status`).
fn git_output(cwd: &Path, home: &Path, args: &[&str]) -> Output {
    let mut command = Command::new("git");
    command.args(args).current_dir(cwd);
    isolated(&mut command, home);
    command.output().unwrap()
}

fn git(cwd: &Path, home: &Path, args: &[&str]) -> String {
    let output = git_output(cwd, home, args);
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

    /// [`World::act`], its tools found first in `bin`: a stand-in for one of them (GitHub's `gh`).
    fn act_on_path(&self, id: &str, input: Value, bin: &Path) -> Output {
        let mut command = Command::new(env!("CARGO_BIN_EXE_workcell"));
        command.args(["--json", "action", "run", id, &input.to_string()]);
        isolated(&mut command, &self.home);
        let mut path = vec![bin.to_path_buf()];
        path.extend(std::env::split_paths(
            &std::env::var_os("PATH").unwrap_or_default(),
        ));
        command
            .env("PATH", std::env::join_paths(path).unwrap())
            .env("CENTRAL_ROOT", &self.world)
            .output()
            .unwrap()
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
        seat["workers"] = json!(["agent:landing-test"]);
        fs::write(&path, serde_json::to_vec_pretty(&register).unwrap()).unwrap();
    }

    /// A co-worker on the lane, as the seat instrument's `seat join --actor <actor> --region <regions>` attaches one: among
    /// the seat's `workers`, with its zone.
    fn attach(&self, actor: &str, regions: &[&str]) {
        let path = self.world.join(REGISTER);
        let mut register: Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
        register["workcells"][0]["seats"][0]["workers"]
            .as_array_mut()
            .unwrap()
            .push(json!(actor));
        fs::write(&path, serde_json::to_vec_pretty(&register).unwrap()).unwrap();
        if !regions.is_empty() {
            self.zone(actor, regions);
        }
    }

    /// A zone on the seat's claim, as the seat instrument's `seat join --region` records one: `{actor, regions,
    /// since_unix_seconds}` in the seat's `zones`. The instrument keeps the zone of a lane a `--new-lane` claim displaced, its
    /// actor no longer among the workers.
    fn zone(&self, actor: &str, regions: &[&str]) {
        let path = self.world.join(REGISTER);
        let mut register: Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
        let seat = &mut register["workcells"][0]["seats"][0];
        if !seat["zones"].is_array() {
            seat["zones"] = json!([]);
        }
        seat["zones"].as_array_mut().unwrap().push(json!({
            "actor": actor,
            "regions": regions,
            "since_unix_seconds": 1_794_000_000u64,
        }));
        fs::write(&path, serde_json::to_vec_pretty(&register).unwrap()).unwrap();
    }

    /// The checkout's changes, a line each, as `git status --porcelain --untracked-files=all` shows them.
    fn standing(&self) -> Vec<String> {
        let output = git_output(
            &self.checkout,
            &self.home,
            &["status", "--porcelain", "--untracked-files=all"],
        );
        String::from_utf8_lossy(&output.stdout)
            .lines()
            .map(str::to_owned)
            .collect()
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

#[test]
fn a_commit_leaves_out_the_seat_instruments_target_link() {
    // An env seat's `target` is the seat instrument's link into the primary's shared build tree (its ensure_target_pool), not the
    // lane's work. A repository whose .gitignore does not name `/target` sees the link untracked, so a commit leaves it out by name.
    let world = World::new("pool", false);
    world.hold_lane("lane-1");
    fs::create_dir_all(world.primary.join("target")).unwrap();
    std::os::unix::fs::symlink(world.primary.join("target"), world.checkout.join("target"))
        .unwrap();
    assert_eq!(
        git(&world.checkout, &world.home, &["status", "--porcelain"]),
        "?? target"
    );
    let link_only = refusal(world.act(
        "workcell.lane.commit",
        world.with("message", "The link alone"),
    ));
    assert!(link_only.contains("nothing to commit"), "{link_only}");

    fs::write(world.checkout.join("NOTES.md"), "The lane's note.\n").unwrap();
    let committed = answer(world.act(
        "workcell.lane.commit",
        world.with("message", "The lane's note"),
    ));
    assert_eq!(committed["files"].as_u64(), Some(1));
    assert_eq!(
        git(
            &world.checkout,
            &world.home,
            &["show", "--name-only", "--format=", "HEAD"]
        ),
        "NOTES.md",
        "the commit holds the lane's work and not the seat's link"
    );
    assert_eq!(
        git(&world.checkout, &world.home, &["status", "--porcelain"]),
        "?? target",
        "the link stays where the seat instrument put it: a landing never tidies the working files"
    );
}

#[test]
fn a_commit_takes_the_lanes_work_where_git_ignores_the_target_link() {
    // Every env seat on this machine ignores its `target` link: a .gitignore naming `/target` or `target`, or the primary's
    // info/exclude (as Factory's does). A commit that named the ignored link in a pathspec made `git add` exit 1 there, so
    // no commit was made on any of these seats (W130-1). The commit takes the lane's work, and never the link.
    for (label, place, rule) in [
        ("rooted", ".gitignore", "/target"),
        ("bare", ".gitignore", "target"),
        ("exclude", "info/exclude", "/target"),
    ] {
        let world = World::new(&format!("ignored-{label}"), false);
        world.hold_lane("lane-1");
        if place == ".gitignore" {
            fs::write(world.checkout.join(".gitignore"), format!("{rule}\n")).unwrap();
            git(&world.checkout, &world.home, &["add", ".gitignore"]);
            git(
                &world.checkout,
                &world.home,
                &["commit", "--quiet", "-m", "The build tree is ignored"],
            );
        } else {
            fs::create_dir_all(world.primary.join(".git/info")).unwrap();
            fs::write(world.primary.join(".git/info/exclude"), format!("{rule}\n")).unwrap();
        }
        fs::create_dir_all(world.primary.join("target")).unwrap();
        std::os::unix::fs::symlink(world.primary.join("target"), world.checkout.join("target"))
            .unwrap();
        assert!(world.standing().is_empty(), "{label}: git ignores the link");
        let link_only = refusal(world.act(
            "workcell.lane.commit",
            world.with("message", "The link alone"),
        ));
        assert!(
            link_only.contains("nothing to commit"),
            "{label}: {link_only}"
        );

        fs::write(world.checkout.join("NOTES.md"), "The lane's note.\n").unwrap();
        let committed = answer(world.act(
            "workcell.lane.commit",
            world.with("message", "The lane's note"),
        ));
        assert_eq!(committed["files"].as_u64(), Some(1), "{label}");
        assert_eq!(
            git(
                &world.checkout,
                &world.home,
                &["show", "--name-only", "--format=", "HEAD"]
            ),
            "NOTES.md",
            "{label}: the commit holds the lane's work and not the link"
        );
        assert!(world.standing().is_empty(), "{label}");
        assert!(
            world.checkout.join("target").is_symlink(),
            "{label}: the link stays where the seat instrument put it"
        );
    }
}

#[test]
fn a_commit_is_refused_mid_merge_or_cherry_pick_and_leaves_it_as_it_was() {
    // In the middle of a conflicted merge or cherry-pick, a commit staged the conflict markers and concluded it (W130-2). It
    // is refused in words before anything is staged, and the operation stands as it was, to be resolved or aborted.
    let world = World::new("conflict", false);
    world.hold_lane("lane-1");
    fs::write(
        world.checkout.join("README.md"),
        "notes, as the lane has them\n",
    )
    .unwrap();
    git(
        &world.checkout,
        &world.home,
        &["commit", "--quiet", "-am", "The lane's README"],
    );
    fs::write(world.primary.join("README.md"), "notes, as main has them\n").unwrap();
    git(
        &world.primary,
        &world.home,
        &["commit", "--quiet", "-am", "Main's README"],
    );
    let head = git(&world.checkout, &world.home, &["rev-parse", "HEAD"]);
    let operations: [(&str, &[&str], &str, &[&str]); 2] = [
        (
            "merge",
            &["merge", "--quiet", "main"],
            "MERGE_HEAD",
            &["merge", "--abort"],
        ),
        (
            "cherry-pick",
            &["cherry-pick", "main"],
            "CHERRY_PICK_HEAD",
            &["cherry-pick", "--abort"],
        ),
    ];
    for (operation, start, under_way, abort) in operations {
        assert!(
            !git_output(&world.checkout, &world.home, start)
                .status
                .success(),
            "the {operation} conflicts"
        );
        let conflicted = world.standing();
        assert_eq!(conflicted, ["UU README.md"], "{operation}");
        let refused =
            refusal(world.act("workcell.lane.commit", world.with("message", "Lane work")));
        assert!(
            refused.contains(&format!("a {operation} ({under_way})"))
                && refused.contains("unmerged README.md"),
            "{refused}"
        );
        assert_eq!(world.standing(), conflicted, "{operation}: nothing staged");
        let stands = |when: &str| {
            assert_eq!(
                git(&world.checkout, &world.home, &["rev-parse", "HEAD"]),
                head,
                "{operation}, {when}: nothing committed"
            );
            git(
                &world.checkout,
                &world.home,
                &["rev-parse", "--quiet", "--verify", under_way],
            );
        };
        stands("conflicted");

        // Resolved but not concluded: a commit would still conclude it, so it is still refused.
        fs::write(world.checkout.join("README.md"), "notes, resolved\n").unwrap();
        git(&world.checkout, &world.home, &["add", "README.md"]);
        let unconcluded =
            refusal(world.act("workcell.lane.commit", world.with("message", "Lane work")));
        assert!(
            unconcluded.contains(&format!("a {operation} ({under_way})")),
            "{unconcluded}"
        );
        stands("resolved");
        git(&world.checkout, &world.home, abort);
    }
}

#[test]
fn a_commit_takes_only_the_lanes_regions_and_names_what_it_left_outside() {
    // A shared checkout holds other work beside the lane's: another lane's edit, an untracked secret. The commit took all of
    // it (W130-3). Where the lane's claim names regions (the seat's `zones`, as `seat join --region` records them), it takes
    // the changes inside them and names what it left outside, which stays as it was; a region that names no path in the
    // checkout is refused by name rather than guessed at.
    let world = World::new("zones", false);
    world.hold_lane("lane-1");
    world.zone("agent:landing-test", &["crates/a"]);
    world.attach("agent:co-worker", &["docs/"]);
    for (path, text) in [
        ("crates/a/lib.rs", "the lane's crate\n"),
        ("docs/guide.md", "the co-worker's guide\n"),
        ("crates/ab/lib.rs", "a neighbour's crate\n"),
        ("crates/b/lib.rs", "another lane's crate\n"),
        (".env.local", "TOKEN=not-the-lanes\n"),
        ("README.md", "notes, as another lane edits them\n"),
    ] {
        let file = world.checkout.join(path);
        fs::create_dir_all(file.parent().unwrap()).unwrap();
        fs::write(file, text).unwrap();
    }
    let committed = answer(world.act(
        "workcell.lane.commit",
        world.with("message", "The lane's crate and its guide"),
    ));
    assert_eq!(committed["files"].as_u64(), Some(2));
    assert_eq!(committed["regions"], json!(["crates/a", "docs"]));
    assert_eq!(
        git(
            &world.checkout,
            &world.home,
            &["show", "--name-only", "--format=", "HEAD"]
        ),
        "crates/a/lib.rs\ndocs/guide.md"
    );
    let mut left = committed["left_outside"]
        .as_array()
        .unwrap_or_else(|| panic!("the answer names what it left: {committed}"))
        .iter()
        .map(|path| path.as_str().unwrap())
        .collect::<Vec<_>>();
    left.sort_unstable();
    assert_eq!(
        left,
        [
            ".env.local",
            "README.md",
            "crates/ab/lib.rs",
            "crates/b/lib.rs"
        ]
    );
    let outside = [
        " M README.md",
        "?? .env.local",
        "?? crates/ab/lib.rs",
        "?? crates/b/lib.rs",
    ];
    assert_eq!(
        world.standing(),
        outside,
        "what lies outside stays as it was"
    );
    let head = git(&world.checkout, &world.home, &["rev-parse", "HEAD"]);

    let nothing = refusal(world.act(
        "workcell.lane.commit",
        world.with("message", "Not the lane's"),
    ));
    assert!(
        nothing.contains("nothing to commit") && nothing.contains("crates/b/lib.rs"),
        "{nothing}"
    );

    let free_text = "native host field hand-off for live-engine's K8 node (new files first)";
    world.attach("agent:free-text", &[free_text]);
    fs::write(
        world.checkout.join("crates/a/lib.rs"),
        "the lane's crate, again\n",
    )
    .unwrap();
    let unread = refusal(world.act(
        "workcell.lane.commit",
        world.with("message", "The lane's crate, again"),
    ));
    assert!(unread.contains(free_text), "{unread}");
    assert_eq!(
        git(&world.checkout, &world.home, &["rev-parse", "HEAD"]),
        head,
        "nothing committed"
    );
    assert_eq!(
        world.standing(),
        [
            " M README.md",
            " M crates/a/lib.rs",
            "?? .env.local",
            "?? crates/ab/lib.rs",
            "?? crates/b/lib.rs",
        ],
        "nothing staged"
    );
}

/// An act's output, once it is read for the credential the test's origin carries: nothing in it shows it.
fn shown(output: Output) -> Output {
    for stream in [&output.stdout, &output.stderr] {
        let text = String::from_utf8_lossy(stream);
        assert!(!text.contains("s3cret"), "the credential is shown: {text}");
    }
    output
}

#[test]
fn no_answer_or_refusal_shows_a_credential_in_origins_url() {
    // An origin URL can carry a credential: userinfo, or an insteadOf rewrite that adds one. The answers and one refusal
    // printed it as git read it (W130-5). git pushes with the URL as it is; the acts name the remote without its userinfo,
    // as git's own messages do.
    let world = World::new("credential", true);
    world.hold_lane("lane-1");
    let bare = world.origin();
    let bare = bare.to_str().unwrap();
    // https: origin's URL carries the credential, and the push reaches the scratch remote through origin's pushurl.
    git(
        &world.primary,
        &world.home,
        &[
            "remote",
            "set-url",
            "origin",
            "https://landing:s3cret@git.example.invalid/notes.git",
        ],
    );
    git(
        &world.primary,
        &world.home,
        &["remote", "set-url", "--push", "origin", bare],
    );
    fs::write(world.checkout.join("NOTES.md"), "The lane's note.\n").unwrap();
    let committed = answer(world.act(
        "workcell.lane.commit",
        world.with("message", "The lane's note"),
    ));
    let pushed = answer(shown(world.act("workcell.lane.push", world.seat())));
    assert_eq!(pushed["remote"], "https://git.example.invalid/notes.git");
    for land_as in ["draft", "mr"] {
        let refused = refusal(shown(
            world.act("workcell.lane.land", world.with("as", land_as)),
        ));
        assert!(
            refused.contains("https://git.example.invalid/notes.git is not on GitHub"),
            "{land_as}: {refused}"
        );
    }

    // On GitHub, a merge request is opened with GitHub's CLI (a stand-in here, first on PATH), and its answer names the remote
    // without the credential too.
    git(
        &world.primary,
        &world.home,
        &[
            "remote",
            "set-url",
            "origin",
            "https://x-access-token:s3cret@github.com/EpiLogos/notes.git",
        ],
    );
    let bin = world.scratch.0.join("bin");
    fs::create_dir_all(&bin).unwrap();
    let gh = bin.join("gh");
    fs::write(
        &gh,
        "#!/bin/sh\necho https://github.com/EpiLogos/notes/pull/7\n",
    )
    .unwrap();
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&gh, fs::Permissions::from_mode(0o755)).unwrap();
    }
    for land_as in ["draft", "mr"] {
        let opened = answer(shown(world.act_on_path(
            "workcell.lane.land",
            world.with("as", land_as),
            &bin,
        )));
        assert_eq!(
            opened["remote"], "https://github.com/EpiLogos/notes.git",
            "{land_as}: {opened}"
        );
        assert_eq!(
            opened["merge_request"], "https://github.com/EpiLogos/notes/pull/7",
            "{land_as}: {opened}"
        );
    }

    // A clean URL that a credentialed insteadOf rewrites: git reads the rewritten URL as origin's.
    let rewrite = "url.https://landing:s3cret@git.example.invalid/.insteadOf";
    git(
        &world.primary,
        &world.home,
        &[
            "remote",
            "set-url",
            "origin",
            "https://git.example.invalid/notes.git",
        ],
    );
    git(
        &world.primary,
        &world.home,
        &[
            "config",
            "--global",
            rewrite,
            "https://git.example.invalid/",
        ],
    );
    let refused = refusal(shown(
        world.act("workcell.lane.land", world.with("as", "draft")),
    ));
    assert!(
        refused.contains("https://git.example.invalid/notes.git is not on GitHub"),
        "{refused}"
    );

    // file://: the merge's push and the fetch after it both reach the scratch remote through the credentialed URL.
    git(
        &world.primary,
        &world.home,
        &["config", "--global", "--unset", rewrite],
    );
    git(
        &world.primary,
        &world.home,
        &["config", "--unset", "remote.origin.pushurl"],
    );
    git(
        &world.primary,
        &world.home,
        &[
            "remote",
            "set-url",
            "origin",
            &format!("file://landing:s3cret@{bare}"),
        ],
    );
    let merged = answer(shown(
        world.act("workcell.lane.land", world.with("as", "merge")),
    ));
    assert_eq!(merged["remote"], format!("file://{bare}"));
    assert_eq!(
        git(
            &world.origin(),
            &world.home,
            &["rev-parse", "refs/heads/main"]
        ),
        committed["head"].as_str().unwrap()
    );
}

/// The paths a commit records, a line each (`<status>\t<path>[\t<new path>]`), as `git show --name-status` reads them.
fn recorded(world: &World, renames: &str) -> Vec<String> {
    git(
        &world.checkout,
        &world.home,
        &["show", renames, "--name-status", "--format=", "HEAD"],
    )
    .lines()
    .map(str::to_owned)
    .collect()
}

fn write(world: &World, path: &str, text: &str) {
    let file = world.checkout.join(path);
    fs::create_dir_all(file.parent().unwrap()).unwrap();
    fs::write(file, text).unwrap();
}

#[test]
fn a_zone_a_new_lane_displaced_is_not_the_lanes() {
    // A lane that takes a seat over (`seat join --new-lane --region crates/a`) becomes its only worker, and the seat
    // instrument keeps the displaced lane's zone on the claim. Counting that zone swept the displaced lane's unfinished work
    // and its secret into the new lane's commit (RR130-1). The lane's regions are its actors' zones alone.
    let world = World::new("displaced", false);
    world.hold_lane("lane-2");
    world.zone("agent:displaced", &["crates/b"]);
    world.zone("agent:landing-test", &["crates/a"]);
    write(&world, "crates/a/lib.rs", "the new lane's crate\n");
    write(
        &world,
        "crates/b/lib.rs",
        "the displaced lane's unfinished crate\n",
    );
    write(&world, "crates/b/.env.local", "TOKEN=the-displaced-lanes\n");
    let committed = answer(world.act(
        "workcell.lane.commit",
        world.with("message", "The new lane's crate"),
    ));
    assert_eq!(committed["regions"], json!(["crates/a"]), "{committed}");
    assert_eq!(recorded(&world, "--no-renames"), ["A\tcrates/a/lib.rs"]);
    assert_eq!(
        committed["left_outside"],
        json!(["crates/b/.env.local", "crates/b/lib.rs"]),
        "{committed}"
    );
    assert_eq!(
        world.standing(),
        ["?? crates/b/.env.local", "?? crates/b/lib.rs"],
        "the displaced lane's work stays as it was"
    );
}

#[test]
fn a_lead_with_no_zone_commits_its_work_beside_a_zoned_co_worker() {
    // A lead who claims without --region holds the whole seat: the instrument reads no region as `.`. A co-worker's zone
    // narrowed the commit to the co-worker's regions, and the lead's own work was left out of the landing (RR130-4).
    let world = World::new("zoneless", false);
    world.hold_lane("lane-1");
    world.attach("agent:co-worker", &["docs"]);
    write(&world, "crates/x/lib.rs", "the lead's crate\n");
    write(&world, "docs/guide.md", "the co-worker's guide\n");
    let committed = answer(world.act(
        "workcell.lane.commit",
        world.with("message", "The lead's crate and the guide"),
    ));
    assert_eq!(committed["files"].as_u64(), Some(2), "{committed}");
    assert_eq!(committed["regions"], json!([]), "{committed}");
    assert_eq!(committed["left_outside"], json!([]), "{committed}");
    assert_eq!(
        recorded(&world, "--no-renames"),
        ["A\tcrates/x/lib.rs", "A\tdocs/guide.md"]
    );
    assert!(world.standing().is_empty(), "{:?}", world.standing());
}

#[test]
fn a_region_not_made_yet_or_deleted_holds_no_change_and_never_stops_a_commit() {
    // The instrument records a lane's regions as the zones it will work, and reads one where nothing stands as holding
    // nothing. A region not made yet, or one the lane deleted, refused every commit of the lane (RR130-2).
    let world = World::new("regions", false);
    world.hold_lane("lane-1");
    world.zone(
        "agent:landing-test",
        &["crates/a", "crates/new", "crates/b"],
    );
    write(&world, "crates/a/lib.rs", "the lane's crate\n");
    write(&world, "crates/b/lib.rs", "a crate the lane will retire\n");
    let first = answer(world.act(
        "workcell.lane.commit",
        world.with("message", "Two crates, a third to come"),
    ));
    assert_eq!(
        first["regions"],
        json!(["crates/a", "crates/new", "crates/b"])
    );
    assert_eq!(first["files"].as_u64(), Some(2), "{first}");
    fs::remove_dir_all(world.checkout.join("crates/b")).unwrap();
    answer(world.act(
        "workcell.lane.commit",
        world.with("message", "The retired crate leaves"),
    ));
    assert_eq!(recorded(&world, "--no-renames"), ["D\tcrates/b/lib.rs"]);
    write(&world, "crates/a/lib.rs", "the lane's crate, again\n");
    answer(world.act(
        "workcell.lane.commit",
        world.with("message", "The lane's crate, again"),
    ));
    assert_eq!(recorded(&world, "--no-renames"), ["M\tcrates/a/lib.rs"]);
    assert!(world.standing().is_empty(), "{:?}", world.standing());
}

#[test]
fn a_rename_in_case_only_and_a_link_swapped_for_a_folder_are_committed_as_staged() {
    // git's partial commit (`commit --only`) reads each named path from disk again: on a checkout that ignores case (every
    // macOS seat), a staged rename in case only failed with "will not add file alias", and a tracked link swapped for a
    // folder failed as a submodule with no commit (RR130-3). The commit is made from what the lane staged.
    let world = World::new("case", false);
    world.hold_lane("lane-1");
    write(&world, "crates/a/lib.rs", "the lane's crate\n");
    std::os::unix::fs::symlink("README.md", world.checkout.join("alink")).unwrap();
    answer(world.act(
        "workcell.lane.commit",
        world.with("message", "A crate and a link"),
    ));
    git(
        &world.checkout,
        &world.home,
        &["mv", "crates/a/lib.rs", "crates/a/Lib.rs"],
    );
    write(&world, "README.md", "notes, renamed beside\n");
    write(&world, "docs/new.md", "a new page\n");
    let renamed = answer(world.act(
        "workcell.lane.commit",
        world.with("message", "The crate's file in its case"),
    ));
    assert_eq!(renamed["files"].as_u64(), Some(4), "{renamed}");
    assert_eq!(
        recorded(&world, "-M"),
        [
            "M\tREADME.md",
            "R100\tcrates/a/lib.rs\tcrates/a/Lib.rs",
            "A\tdocs/new.md"
        ]
    );
    assert!(world.standing().is_empty(), "{:?}", world.standing());
    fs::remove_file(world.checkout.join("alink")).unwrap();
    write(&world, "alink/f", "a folder where the link stood\n");
    answer(world.act(
        "workcell.lane.commit",
        world.with("message", "The link becomes a folder"),
    ));
    assert_eq!(recorded(&world, "--no-renames"), ["D\talink", "A\talink/f"]);
    assert!(world.standing().is_empty(), "{:?}", world.standing());
}

#[test]
fn a_tracked_file_inside_a_folder_git_ignores_is_committed() {
    // ai-kit tracks .aikit/profile.toml under its `/.aikit/` rule, and O-I tracks JSON under folders its `target` rule covers.
    // git add exits 1 on a path an ignore rule covers as a folder, tracked or not, though it stages it, so every commit of
    // such an edit was refused (F1). The lane's paths are staged whatever an ignore rule says: git status lists no ignored new
    // file, and a new file the rules ignore stays out.
    let world = World::new("ignored-folder", false);
    world.hold_lane("lane-1");
    write(&world, ".gitignore", "/.aikit/\ntarget\n");
    write(&world, ".aikit/profile.toml", "name = \"notes\"\n");
    write(&world, "ql/target/k8/basis.json", "{}\n");
    git(
        &world.checkout,
        &world.home,
        &[
            "add",
            "--force",
            ".gitignore",
            ".aikit/profile.toml",
            "ql/target/k8/basis.json",
        ],
    );
    git(
        &world.checkout,
        &world.home,
        &[
            "commit",
            "--quiet",
            "-m",
            "Files tracked under ignored folders",
        ],
    );
    write(&world, ".aikit/profile.toml", "name = \"notes, edited\"\n");
    write(&world, "ql/target/k8/basis.json", "{\"edited\": true}\n");
    write(&world, ".aikit/cache.toml", "a new file the rules ignore\n");
    let committed = answer(world.act(
        "workcell.lane.commit",
        world.with("message", "Edits under ignored folders"),
    ));
    assert_eq!(committed["files"].as_u64(), Some(2), "{committed}");
    assert_eq!(
        recorded(&world, "--no-renames"),
        ["M\t.aikit/profile.toml", "M\tql/target/k8/basis.json"]
    );
    assert!(world.standing().is_empty(), "{:?}", world.standing());
}

#[test]
fn a_new_file_in_a_folder_the_index_holds_in_another_case_is_committed_as_git_staged_it() {
    // On a checkout that ignores case (every macOS seat), a folder can stand on disk in another case than the index holds:
    // `Docs/` on disk, `docs/y.md` in the index. git status names a new file in it in the disk's case, and git add stages it
    // in the index's. The commit took only the entries named exactly as the lane's paths, so it left the file out and still
    // counted it (F2). It takes the entry git staged for each path.
    let world = World::new("folder-case", false);
    world.hold_lane("lane-1");
    write(&world, "docs/y.md", "a page\n");
    git(&world.checkout, &world.home, &["add", "docs/y.md"]);
    git(
        &world.checkout,
        &world.home,
        &["commit", "--quiet", "-m", "A page"],
    );
    fs::write(world.checkout.join("CASE-PROBE"), "").unwrap();
    let folds = world.checkout.join("case-probe").exists();
    fs::remove_file(world.checkout.join("CASE-PROBE")).unwrap();
    if folds {
        // The folder renamed on disk in case only, as Finder, or a switch across a rename of it, leaves it.
        fs::rename(
            world.checkout.join("docs"),
            world.checkout.join("docs-moving"),
        )
        .unwrap();
        fs::rename(
            world.checkout.join("docs-moving"),
            world.checkout.join("Docs"),
        )
        .unwrap();
    } else {
        // A filesystem that keeps case: git reads the checkout as a macOS seat's, the folder in the disk's case beside.
        git(
            &world.checkout,
            &world.home,
            &["config", "core.ignorecase", "true"],
        );
    }
    write(&world, "Docs/x.md", "a new page\n");
    write(&world, "README.md", "notes, with a new page\n");
    assert_eq!(world.standing(), [" M README.md", "?? Docs/x.md"]);
    let committed = answer(world.act("workcell.lane.commit", world.with("message", "A new page")));
    assert_eq!(committed["files"].as_u64(), Some(2), "{committed}");
    assert_eq!(
        recorded(&world, "--no-renames"),
        ["M\tREADME.md", "A\tdocs/x.md"]
    );
    assert_eq!(
        git(&world.checkout, &world.home, &["show", "HEAD:docs/x.md"]),
        "a new page"
    );
    assert!(
        git_output(
            &world.checkout,
            &world.home,
            &["diff", "--cached", "--quiet"]
        )
        .status
        .success(),
        "nothing the lane staged is left out of its commit"
    );
}

/// The git the tests run, by its full path: a stand-in `git` first on PATH hands each call on to it.
fn real_git() -> PathBuf {
    std::env::split_paths(&std::env::var_os("PATH").unwrap_or_default())
        .map(|folder| folder.join("git"))
        .find(|git| git.is_file())
        .expect("git on PATH")
}

/// A stand-in `git`, in a folder of its own to put first on PATH: once, as the lane's `git commit` begins (on its own index),
/// it runs `act` in the checkout, as another process sharing the seat (the checkout's index, not the commit's), and then
/// hands the call on to git. `act` reaches git as `$GIT`. The folder, the mark that `act` ran, and what `act` said.
fn stand_in_git(world: &World, act: &str) -> (PathBuf, PathBuf, PathBuf) {
    use std::os::unix::fs::PermissionsExt;
    let bin = world.scratch.0.join("bin");
    fs::create_dir_all(&bin).unwrap();
    let ran = world.scratch.0.join("stand-in-ran");
    let said = world.scratch.0.join("stand-in.log");
    let stand_in = bin.join("git");
    fs::write(
        &stand_in,
        format!(
            "#!/bin/sh\nGIT='{git}'\nif [ \"$1\" = commit ] && [ -n \"$GIT_INDEX_FILE\" ] && [ ! -e '{ran}' ]; then\n  : > '{ran}'\n  (unset GIT_INDEX_FILE; cd '{checkout}' && {act}) > '{said}' 2>&1\nfi\nexec \"$GIT\" \"$@\"\n",
            git = real_git().display(),
            ran = ran.display(),
            checkout = world.checkout.display(),
            said = said.display(),
        ),
    )
    .unwrap();
    fs::set_permissions(&stand_in, fs::Permissions::from_mode(0o755)).unwrap();
    (bin, ran, said)
}

/// A seat whose lane works in `lane` beside a page every worker shares, `d.md`, committed: the lane's page written.
fn shared_page(label: &str) -> World {
    let world = World::new(label, false);
    world.hold_lane("lane-1");
    world.zone("agent:landing-test", &["lane"]);
    write(&world, "d.md", "d0\n");
    git(&world.checkout, &world.home, &["add", "d.md"]);
    git(
        &world.checkout,
        &world.home,
        &["commit", "--quiet", "-m", "The shared page"],
    );
    write(&world, "lane/a.md", "the lane's page\n");
    world
}

#[test]
fn a_co_workers_commit_is_never_undone_by_the_lanes() {
    // A shared seat: a co-worker commits its own file while the lane's commit is made. The commit's tree was read from HEAD
    // before `git commit` read HEAD again as its parent, so a co-worker's commit landing between the two was taken back by
    // the lane's (F3). A stand-in git lands the co-worker's commit just as the lane's begins.
    let world = shared_page("co-worker");
    write(&world, "d.md", "d1, the co-worker's\n");
    let (bin, ran, said) = stand_in_git(
        &world,
        "\"$GIT\" commit --quiet -m \"The co-worker's page\" -- d.md",
    );
    let committed = answer(world.act_on_path(
        "workcell.lane.commit",
        world.with("message", "The lane's page"),
        &bin,
    ));
    assert!(ran.exists(), "the co-worker's commit was tried");
    assert_eq!(
        recorded(&world, "--no-renames"),
        ["A\tlane/a.md"],
        "the lane's commit holds the lane's page alone (the co-worker's commit: {})",
        fs::read_to_string(&said).unwrap_or_default()
    );
    let subjects = git(&world.checkout, &world.home, &["log", "--format=%s"]);
    if subjects
        .lines()
        .any(|subject| subject == "The co-worker's page")
    {
        assert_eq!(
            git(&world.checkout, &world.home, &["show", "HEAD:d.md"]),
            "d1, the co-worker's"
        );
    } else {
        assert_eq!(committed["left_outside"], json!(["d.md"]), "{committed}");
        assert_eq!(
            world.standing(),
            [" M d.md"],
            "the co-worker's page waits for its own commit"
        );
    }
}

#[test]
fn a_head_set_by_hand_as_the_lane_commits_is_refused_and_left_where_it_was_set() {
    // A ref set by hand takes no index, so nothing that holds the index keeps it out of the moment between the head the
    // lane's commit is cut from and the head git finds as the commit begins. That commit took back what the hand set (F3).
    // The lane's commit is one on the head it was cut from, or none.
    let world = shared_page("set-by-hand");
    write(&world, "d.md", "d1, set by hand\n");
    git(&world.checkout, &world.home, &["add", "d.md"]);
    let tree = git(&world.checkout, &world.home, &["write-tree"]);
    let set = git(
        &world.checkout,
        &world.home,
        &["commit-tree", &tree, "-p", "HEAD", "-m", "Set by hand"],
    );
    git(
        &world.checkout,
        &world.home,
        &["reset", "--quiet", "--", "d.md"],
    );
    write(&world, "d.md", "d0\n");
    let (bin, ran, _) = stand_in_git(&world, &format!("\"$GIT\" update-ref HEAD {set}"));
    let refused = refusal(world.act_on_path(
        "workcell.lane.commit",
        world.with("message", "The lane's page"),
        &bin,
    ));
    assert!(ran.exists(), "the hand set the head");
    assert!(
        refused.contains("moved") && refused.contains("taken back"),
        "{refused}"
    );
    assert_eq!(
        git(&world.checkout, &world.home, &["rev-parse", "HEAD"]),
        set,
        "the head stands where the hand set it"
    );
    assert_eq!(
        git(&world.checkout, &world.home, &["show", "HEAD:d.md"]),
        "d1, set by hand"
    );
    assert!(
        world.standing().contains(&"A  lane/a.md".to_owned()),
        "the lane's page stays staged: {:?}",
        world.standing()
    );
}

#[test]
fn a_zone_kept_for_a_displaced_lane_is_never_taken_by_a_lane_that_holds_the_whole_seat() {
    // A lane that joins without --region holds the whole seat, and the seat instrument keeps the zone of the lane it
    // displaced. Reading the whole seat as the lane's swept the displaced lane's unfinished work and its secret into the new
    // lane's commit (F4). A zone kept for a displaced lane is never the lane's, whatever the lane holds.
    let world = World::new("displaced-whole", false);
    world.hold_lane("lane-2");
    world.zone("agent:displaced", &["crates/b"]);
    write(&world, "crates/n/lib.rs", "the new lane's crate\n");
    write(
        &world,
        "crates/b/lib.rs",
        "the displaced lane's unfinished crate\n",
    );
    write(&world, "crates/b/.env.local", "TOKEN=the-displaced-lanes\n");
    let committed = answer(world.act(
        "workcell.lane.commit",
        world.with("message", "The new lane's crate"),
    ));
    assert_eq!(committed["regions"], json!([]), "{committed}");
    assert_eq!(recorded(&world, "--no-renames"), ["A\tcrates/n/lib.rs"]);
    assert_eq!(
        committed["left_outside"],
        json!(["crates/b/.env.local", "crates/b/lib.rs"]),
        "{committed}"
    );
    assert_eq!(
        world.standing(),
        ["?? crates/b/.env.local", "?? crates/b/lib.rs"],
        "the displaced lane's work stays as it was"
    );
    let only_theirs = refusal(world.act(
        "workcell.lane.commit",
        world.with("message", "The displaced lane's work"),
    ));
    assert!(
        only_theirs.contains("nothing to commit") && only_theirs.contains("crates/b"),
        "{only_theirs}"
    );
}
