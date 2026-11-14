// A World's workcell, declared (O-I #1006). Included by main.rs, beside its
// dispatch.
//
// `workcell declare --world <World> --project <name>` gives a World that has
// none its first workcell: the persistent track `env-1` under the World's
// `worktrees/`, holding one linked checkout (`git worktree`) of the project's
// primary (`Work/<name>`, standing on `main`), the shape every seat of the
// machine has, and the World's workcell register
// (`Control/machines/workcells.register.json`, central.workcell-register/v1)
// naming it, its seat free and detached at the main it can see: the primary's
// `origin/main` where the primary knows one, else the primary's own `main`.
// From then on the seats are the seat instrument's: it joins and releases them
// (`workcell.seat.join`, `workcell.seat.release`) in the World its
// CENTRAL_ROOT names, cutting a lane from that main. A World that already
// declares its workcells is refused, its register untouched: a declaration is
// made once, and changed by the instrument that keeps it.

const WORKCELL_REGISTER_SCHEMA: &str = "central.workcell-register/v1";
const WORKCELL_DECLARATION: &str = "workcell.declaration/v1";
const WORKCELL_REGISTER_REL: &str = "Control/machines/workcells.register.json";
/// The persistent track a fresh World's sessions work in (the seat instrument
/// joins env-1, env-2 and env-3 only).
const FIRST_TRACK: &str = "env-1";
/// The admission floors a World's first workcell declares: below the soft
/// floor the instrument tends its caches before it admits, and below the
/// hard floor it refuses a lane.
const SOFT_FLOOR_GB: u64 = 5;
const HARD_FLOOR_GB: u64 = 2;

fn declare_refused(words: impl Into<String>) -> WorkcellError {
    WorkcellError::InvalidDemand(words.into())
}

fn declare_git(cwd: &Path, args: &[&str]) -> Result<String, WorkcellError> {
    let output = std::process::Command::new("git")
        .args(args)
        .current_dir(cwd)
        .env_remove("GIT_DIR")
        .env_remove("GIT_WORK_TREE")
        .env_remove("GIT_INDEX_FILE")
        .stdin(std::process::Stdio::null())
        .output()
        .map_err(|error| {
            WorkcellError::OperationFailed(format!("git could not start: {error}"))
        })?;
    if !output.status.success() {
        return Err(WorkcellError::OperationFailed(format!(
            "git {} refused in {}: {}",
            args.join(" "),
            cwd.display(),
            String::from_utf8_lossy(&output.stderr).trim()
        )));
    }
    Ok(String::from_utf8_lossy(&output.stdout).trim().to_owned())
}

/// The register a World's first workcell writes: the shape the seat instrument
/// reads (`find_workcell`, `find_seat`, `product_dir`, the admission floors),
/// its seat's lanes cut from `main_ref`.
fn first_register(env: &Path, project: &str, head: &str, main_ref: &str, now: u64) -> Value {
    json!({
        "schema": WORKCELL_REGISTER_SCHEMA,
        "scope_ref": "control:root",
        "machine_ref": "workcell:local",
        "recorded_at_unix_seconds": now,
        "instrument": {
            "name": "seat",
            "source": "Control/user/workcell/seat",
            "note": "Declared by `workcell declare`; its seats are joined and released only through the seat instrument, in the World CENTRAL_ROOT names."
        },
        "workcells": [{
            "name": FIRST_TRACK,
            "path": env,
            "domains": [],
            "notes": [format!("Declared with the World by `workcell declare`: one linked checkout of {project}'s primary.")],
            "products": {
                project: {"path": project, "main_ref": main_ref, "primary": format!("Work/{project}")}
            },
            "seats": [{
                "product": project,
                "branch": null,
                "head": head,
                "state": "free",
                "claimed_by": null,
                "now_ref": null,
                "claimed_at_unix_seconds": null,
                "note": format!("declared free, detached at {main_ref}")
            }],
            "admission": {"min_free_gb": SOFT_FLOOR_GB, "hard_free_gb": HARD_FLOOR_GB}
        }],
        "retired_seats": []
    })
}

fn command_declare(_global: &GlobalArgs, args: &[String]) -> Result<(), WorkcellError> {
    let mut world: Option<&str> = None;
    let mut project: Option<&str> = None;
    let mut index = 0;
    while index < args.len() {
        match args[index].as_str() {
            "--world" => {
                index += 1;
                world = args.get(index).map(String::as_str);
            }
            "--project" => {
                index += 1;
                project = args.get(index).map(String::as_str);
            }
            other => return Err(declare_refused(format!("unknown declare flag `{other}`"))),
        }
        index += 1;
    }
    let (Some(world), Some(project)) = (world, project) else {
        return Err(declare_refused(
            "usage: workcell declare --world <the World's folder> --project <one folder of its Work>",
        ));
    };
    let world = PathBuf::from(world);
    if !world.is_absolute() || !world.join("Control").is_dir() {
        return Err(declare_refused(format!(
            "{} is no Central World: a World is an absolute folder that keeps Control/",
            world.display()
        )));
    }
    // The World by its real path, never by a link to it (on macOS /tmp is /private/tmp): git records a seat's primary by its
    // real path in the seat's `.git`, and whoever finds the seat's project reads that path under the World's Work (AIKit's
    // project rule), so the register, the seat and the checkout name the World as git does.
    let world = world.canonicalize().map_err(|error| {
        WorkcellError::OperationFailed(format!("{}: {error}", world.display()))
    })?;
    if project.is_empty()
        || project.contains('/')
        || project.starts_with('.')
        || project.chars().any(|c| c.is_whitespace() || c.is_control())
    {
        return Err(declare_refused(
            "a project is named by one folder of the World's Work (its name, never a path)",
        ));
    }
    let register = world.join(WORKCELL_REGISTER_REL);
    if register.exists() {
        return Err(declare_refused(format!(
            "{} declares this World's workcells already; its seats are the seat instrument's to join and release",
            register.display()
        )));
    }
    let primary = world.join("Work").join(project);
    if !primary.join(".git").exists() {
        return Err(declare_refused(format!(
            "{} is no repository: a workcell holds a checkout of a project's primary",
            primary.display()
        )));
    }
    let branch = declare_git(&primary, &["rev-parse", "--abbrev-ref", "HEAD"])?;
    if branch != "main" {
        return Err(declare_refused(format!(
            "{}'s primary stands on {branch}, not main: a seat is cut from its main",
            primary.display()
        )));
    }
    let env = world.join("worktrees").join(FIRST_TRACK);
    let checkout = env.join(project);
    if checkout.exists() {
        return Err(declare_refused(format!(
            "{} exists already, outside any register: it is left as it is",
            checkout.display()
        )));
    }
    fs::create_dir_all(&env)
        .map_err(|error| WorkcellError::OperationFailed(format!("{}: {error}", env.display())))?;
    // The checkout by its real path too, where the World's `worktrees` is itself a link.
    let env = env
        .canonicalize()
        .map_err(|error| WorkcellError::OperationFailed(format!("{}: {error}", env.display())))?;
    let checkout = env.join(project);
    // The seat is a linked checkout of the primary, as every seat of the machine is: it shares the primary's refs, and its
    // `.git` names the worktree the primary keeps for it, so whoever reads a seat as a worktree of `Work/<name>` (AIKit's
    // project rule) finds its project. It stands detached at the main it can see, where its lanes are cut: the primary's
    // origin/main where the primary knows one, else the primary's own main (which the primary stands on, so detached).
    let main_ref = if declare_git(
        &primary,
        &["rev-parse", "--verify", "--quiet", "refs/remotes/origin/main"],
    )
    .is_ok()
    {
        "origin/main"
    } else {
        "main"
    };
    let checkout_text = checkout.to_string_lossy().into_owned();
    declare_git(
        &primary,
        &["worktree", "add", "--quiet", "--detach", &checkout_text, main_ref],
    )?;
    let head = declare_git(&checkout, &["rev-parse", "--short", "HEAD"])?;
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|elapsed| elapsed.as_secs())
        .unwrap_or_default();
    let document = first_register(&env, project, &head, main_ref, now);
    let machines = register.parent().unwrap_or(&world).to_path_buf();
    fs::create_dir_all(&machines).map_err(|error| {
        WorkcellError::OperationFailed(format!("{}: {error}", machines.display()))
    })?;
    let bytes = serde_json::to_vec_pretty(&document)
        .map_err(|error| WorkcellError::OperationFailed(error.to_string()))?;
    {
        use std::io::Write;
        #[cfg(unix)]
        use std::os::unix::fs::OpenOptionsExt;
        let mut options = fs::OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        options.mode(0o600);
        let mut file = options.open(&register).map_err(|error| {
            WorkcellError::OperationFailed(format!("{}: {error}", register.display()))
        })?;
        file.write_all(&bytes)
            .and_then(|()| file.write_all(b"\n"))
            .and_then(|()| file.sync_all())
            .map_err(|error| {
                WorkcellError::OperationFailed(format!("{}: {error}", register.display()))
            })?;
    }
    emit_json(json!({
        "schema": WORKCELL_DECLARATION,
        "world": world,
        "register": register,
        "workcell": FIRST_TRACK,
        "path": env,
        "product": project,
        "checkout": checkout,
        "head": head,
        "main_ref": main_ref,
        "seat": "free",
        "standing": "declared; its seat is joined and released by the seat instrument (workcell.seat.join, workcell.seat.release)",
    }));
    Ok(())
}

#[cfg(test)]
mod declare_tests {
    use super::*;

    /// A scratch folder of the test's own, removed when it ends.
    struct Scratch(PathBuf);
    impl Scratch {
        fn new(name: &str) -> Self {
            let path = std::env::temp_dir().join(format!(
                "workcell-declare-{name}-{}-{}",
                std::process::id(),
                SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_nanos()
            ));
            fs::create_dir_all(&path).unwrap();
            Scratch(path.canonicalize().unwrap())
        }
        fn path(&self) -> &Path {
            &self.0
        }
    }
    impl Drop for Scratch {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    fn git_in(cwd: &Path, args: &[&str]) {
        declare_git(cwd, args).unwrap();
    }

    fn world_with_project(root: &Path) -> PathBuf {
        let world = root.join("Central");
        fs::create_dir_all(world.join("Control")).unwrap();
        let project = world.join("Work/notes");
        fs::create_dir_all(&project).unwrap();
        git_in(&project, &["init", "--quiet", "--initial-branch", "main"]);
        fs::write(project.join("README.md"), "notes\n").unwrap();
        git_in(&project, &["add", "README.md"]);
        git_in(
            &project,
            &["-c", "user.name=t", "-c", "user.email=t@t", "commit", "--quiet", "-m", "first"],
        );
        world
    }

    fn declare(world: &Path, project: &str) -> Result<(), WorkcellError> {
        let global = parse_global(vec!["--json".into()]).unwrap();
        command_declare(
            &global,
            &[
                "--world".into(),
                world.to_string_lossy().into_owned(),
                "--project".into(),
                project.into(),
            ],
        )
    }

    /// The seat is a linked checkout of the primary, the shape every seat of the machine has: its `.git` is a file naming
    /// the worktree the primary keeps for it (`Work/<name>/.git/worktrees/`), and its HEAD stands detached.
    fn assert_linked_to_its_primary(checkout: &Path, primary: &Path) {
        let link = fs::read_to_string(checkout.join(".git")).unwrap_or_else(|error| {
            panic!("{}/.git is no file naming the primary's worktree: {error}", checkout.display())
        });
        let gitdir = link.trim().strip_prefix("gitdir: ").unwrap_or_default();
        assert!(
            Path::new(gitdir).starts_with(primary.join(".git/worktrees")),
            "{} links to {gitdir}, not into the primary's worktrees",
            checkout.display()
        );
        let listed = declare_git(primary, &["worktree", "list", "--porcelain"]).unwrap();
        assert!(
            listed.lines().any(|line| line == format!("worktree {}", checkout.display())),
            "the primary keeps no worktree at {}: {listed}",
            checkout.display()
        );
        assert!(declare_git(checkout, &["symbolic-ref", "--quiet", "HEAD"]).is_err());
    }

    #[test]
    fn a_world_declares_its_first_workcell_once_with_its_seat_free_at_main() {
        let root = Scratch::new("once");
        let world = world_with_project(root.path());
        declare(&world, "notes").unwrap();
        let register: Value =
            serde_json::from_slice(&fs::read(world.join(WORKCELL_REGISTER_REL)).unwrap()).unwrap();
        assert_eq!(register["schema"], WORKCELL_REGISTER_SCHEMA);
        let cell = &register["workcells"][0];
        assert_eq!(cell["name"], "env-1");
        assert_eq!(cell["seats"][0]["state"], "free");
        // The primary has no origin: its seat sees the primary's own main, and is cut from it.
        assert_eq!(cell["products"]["notes"]["main_ref"], "main");
        let checkout = world.join("worktrees/env-1/notes");
        let primary = world.join("Work/notes");
        assert!(checkout.join("README.md").is_file());
        assert_linked_to_its_primary(&checkout, &primary);
        assert_eq!(
            declare_git(&checkout, &["rev-parse", "HEAD"]).unwrap(),
            declare_git(&primary, &["rev-parse", "main"]).unwrap()
        );
        let before = fs::read(world.join(WORKCELL_REGISTER_REL)).unwrap();
        let again = declare(&world, "notes").unwrap_err().to_string();
        assert!(again.contains("declares this World's workcells already"), "{again}");
        assert_eq!(fs::read(world.join(WORKCELL_REGISTER_REL)).unwrap(), before);
    }

    #[test]
    fn a_primary_with_an_origin_declares_its_seat_at_origin_main() {
        let root = Scratch::new("origin");
        let world = world_with_project(root.path());
        let primary = world.join("Work/notes");
        let origin = root.path().join("notes.git");
        git_in(
            root.path(),
            &["init", "--quiet", "--bare", "--initial-branch", "main", &origin.to_string_lossy()],
        );
        git_in(&primary, &["remote", "add", "origin", &origin.to_string_lossy()]);
        git_in(&primary, &["push", "--quiet", "origin", "main"]);
        // The primary's main moves on past what its origin holds: the seat is cut from origin/main, not from it.
        fs::write(primary.join("NEXT.md"), "next\n").unwrap();
        git_in(&primary, &["add", "NEXT.md"]);
        git_in(
            &primary,
            &["-c", "user.name=t", "-c", "user.email=t@t", "commit", "--quiet", "-m", "next"],
        );
        declare(&world, "notes").unwrap();
        let register: Value =
            serde_json::from_slice(&fs::read(world.join(WORKCELL_REGISTER_REL)).unwrap()).unwrap();
        assert_eq!(register["workcells"][0]["products"]["notes"]["main_ref"], "origin/main");
        let checkout = world.join("worktrees/env-1/notes");
        assert_linked_to_its_primary(&checkout, &primary);
        assert_eq!(
            declare_git(&checkout, &["rev-parse", "HEAD"]).unwrap(),
            declare_git(&primary, &["rev-parse", "origin/main"]).unwrap()
        );
        assert!(!checkout.join("NEXT.md").exists());
    }

    #[cfg(unix)]
    #[test]
    fn a_world_named_through_a_link_is_declared_by_its_real_path() {
        // A World reached through a link (on macOS /tmp is /private/tmp, and a mktemp World stands under /var, which is
        // /private/var): git records the seat's primary by its real path, so the register and the checkout name the World by
        // that path too, and the seat's primary stands under the register's World's Work, where AIKit's project rule finds it.
        let root = Scratch::new("linked");
        let real = world_with_project(root.path());
        let link = root.path().join("linked-world");
        std::os::unix::fs::symlink(&real, &link).unwrap();
        declare(&link, "notes").unwrap();
        let register: Value =
            serde_json::from_slice(&fs::read(real.join(WORKCELL_REGISTER_REL)).unwrap()).unwrap();
        let env = PathBuf::from(register["workcells"][0]["path"].as_str().unwrap());
        assert_eq!(env, real.join("worktrees/env-1"), "the register names the World by its real path");
        let seat = fs::read_to_string(env.join("notes/.git")).unwrap();
        let gitdir = PathBuf::from(seat.trim().strip_prefix("gitdir: ").unwrap());
        let world = env.parent().and_then(Path::parent).unwrap();
        assert!(
            gitdir.starts_with(world.join("Work/notes/.git")),
            "the seat's primary ({}) stands under the register's World's Work ({})",
            gitdir.display(),
            world.join("Work").display()
        );
    }

    #[test]
    fn a_declaration_refuses_what_is_no_world_or_no_project() {
        let root = Scratch::new("refused");
        let world = world_with_project(root.path());
        for (world, project, words) in [
            (root.path().join("nowhere"), "notes", "no Central World"),
            (world.clone(), "../notes", "one folder of the World's Work"),
            (world.clone(), "absent", "is no repository"),
        ] {
            let refused = declare(&world, project).unwrap_err().to_string();
            assert!(refused.contains(words), "{refused}");
        }
        assert!(!world.join(WORKCELL_REGISTER_REL).exists());
    }
}
