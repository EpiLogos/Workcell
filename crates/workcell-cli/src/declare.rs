// A World's workcell, declared (O-I #1006). Included by main.rs, beside its
// dispatch.
//
// `workcell declare --world <World> --project <name>` gives a World that has
// none its first workcell: the persistent track `env-1` under the World's
// `worktrees/`, holding one checkout of the project cloned from the project's
// primary (`Work/<name>`, standing on `main`), and the World's workcell
// register (`Control/machines/workcells.register.json`,
// central.workcell-register/v1) naming it, its seat free and detached at
// `origin/main`. From then on the seats are the seat instrument's: it joins and
// releases them (`workcell.seat.join`, `workcell.seat.release`) in the World
// its CENTRAL_ROOT names. A World that already declares its workcells is
// refused, its register untouched: a declaration is made once, and changed by
// the instrument that keeps it.

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
/// reads (`find_workcell`, `find_seat`, `product_dir`, the admission floors).
fn first_register(env: &Path, project: &str, head: &str, now: u64) -> Value {
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
            "notes": [format!("Declared with the World by `workcell declare`: one checkout of {project}, cloned from its primary.")],
            "products": {
                project: {"path": project, "main_ref": "origin/main", "primary": format!("Work/{project}")}
            },
            "seats": [{
                "product": project,
                "branch": null,
                "head": head,
                "state": "free",
                "claimed_by": null,
                "now_ref": null,
                "claimed_at_unix_seconds": null,
                "note": "declared free, detached at origin/main"
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
            "{}'s primary stands on {branch}, not main: a seat is cut from origin/main",
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
    let primary_text = primary.to_string_lossy().into_owned();
    let checkout_text = checkout.to_string_lossy().into_owned();
    declare_git(&env, &["clone", "--quiet", &primary_text, &checkout_text])?;
    declare_git(&checkout, &["switch", "--quiet", "--detach", "origin/main"])?;
    let head = declare_git(&checkout, &["rev-parse", "--short", "HEAD"])?;
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|elapsed| elapsed.as_secs())
        .unwrap_or_default();
    let document = first_register(&env, project, &head, now);
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
        assert_eq!(cell["products"]["notes"]["main_ref"], "origin/main");
        let checkout = world.join("worktrees/env-1/notes");
        assert!(checkout.join("README.md").is_file());
        assert_eq!(
            declare_git(&checkout, &["rev-parse", "HEAD"]).unwrap(),
            declare_git(&world.join("Work/notes"), &["rev-parse", "HEAD"]).unwrap()
        );
        let before = fs::read(world.join(WORKCELL_REGISTER_REL)).unwrap();
        let again = declare(&world, "notes").unwrap_err().to_string();
        assert!(again.contains("declares this World's workcells already"), "{again}");
        assert_eq!(fs::read(world.join(WORKCELL_REGISTER_REL)).unwrap(), before);
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
