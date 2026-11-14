// A seat's landing (O-I #1006; manual §19.16 Git / landing). Included by
// main.rs, beside its dispatch.
//
// `workcell lane commit|push|land --workcell <name> --product <name> …` take a
// seat's lane out to where it lands: its work committed on the lane's branch,
// the branch pushed to the project's remote, and the branch landed as a draft
// merge request, a merge request or a merge into main. Each acts on a seat the
// World's workcell register names (Control/machines/workcells.register.json)
// while a lane holds it; the seat instrument claims and releases seats
// (`workcell.seat.join`, `workcell.seat.release`), and these acts never write
// the register. The World is `--world`, else CENTRAL_ROOT, as the seat
// instrument reads it (the kernel's `act` names the window's World there);
// with neither the act is refused, so it never lands in a register nobody
// named. Every refusal says which state stops it, and no act touches a file it
// did not come for: a landing never tidies the working files.

const LANDING_SCHEMA: &str = "workcell.landing/v1";

fn landing_refused(words: impl Into<String>) -> WorkcellError {
    WorkcellError::InvalidDemand(words.into())
}

/// A seat a lane holds: where its checkout stands, the branch the lane works
/// on, and the main its lanes are cut from and land in.
struct LaneSeat {
    world: PathBuf,
    workcell: String,
    product: String,
    checkout: PathBuf,
    branch: String,
    main_ref: String,
}

/// The flags of a landing act.
#[derive(Default)]
struct LaneArgs {
    world: Option<String>,
    workcell: Option<String>,
    product: Option<String>,
    message: Option<String>,
    land_as: Option<String>,
}

fn lane_args(act: &str, args: &[String]) -> Result<LaneArgs, WorkcellError> {
    let mut parsed = LaneArgs::default();
    let mut index = 0;
    while index < args.len() {
        let flag = args[index].as_str();
        let value = args.get(index + 1).cloned();
        let slot = match (act, flag) {
            (_, "--world") => &mut parsed.world,
            (_, "--workcell") => &mut parsed.workcell,
            (_, "--product") => &mut parsed.product,
            ("commit", "--message") => &mut parsed.message,
            ("land", "--as") => &mut parsed.land_as,
            _ => {
                return Err(landing_refused(format!(
                    "lane {act} takes no flag `{flag}`"
                )))
            }
        };
        if value.is_none() {
            return Err(landing_refused(format!(
                "lane {act}'s `{flag}` needs a value"
            )));
        }
        *slot = value;
        index += 2;
    }
    Ok(parsed)
}

/// The World a landing acts in: `--world`, else CENTRAL_ROOT; an absolute
/// folder that keeps Control/.
fn lane_world(flag: Option<&str>) -> Result<PathBuf, WorkcellError> {
    let named = flag
        .map(str::to_owned)
        .or_else(|| env::var("CENTRAL_ROOT").ok().filter(|root| !root.trim().is_empty()))
        .ok_or_else(|| {
            landing_refused(
                "no World is named: a landing acts in the World `--world` or CENTRAL_ROOT names, never in a register nobody named",
            )
        })?;
    let world = PathBuf::from(&named);
    if !world.is_absolute() || !world.join("Control").is_dir() {
        return Err(landing_refused(format!(
            "{named} is no Central World: a World is an absolute folder that keeps Control/"
        )));
    }
    Ok(world)
}

/// The seat of `product` in `workcell`, as the World's register names it, with
/// the lane that holds it. Refused where the register names no such seat, where
/// the seat is free or another's held lane, and where its checkout does not
/// stand on the lane's branch: a commit from a place that is not a lane's seat
/// is refused.
fn lane_seat(world: &Path, workcell: &str, product: &str) -> Result<LaneSeat, WorkcellError> {
    let register_path = world.join(WORKCELL_REGISTER_REL);
    let register: Value = fs::read(&register_path)
        .map_err(|error| {
            landing_refused(format!(
                "{} declares no workcells ({error}): a World's first workcell is declared with workcell.declare",
                register_path.display()
            ))
        })
        .and_then(|bytes| {
            serde_json::from_slice(&bytes).map_err(|error| {
                landing_refused(format!("{} is no workcell register: {error}", register_path.display()))
            })
        })?;
    if register["schema"] != WORKCELL_REGISTER_SCHEMA {
        return Err(landing_refused(format!(
            "{} is no {WORKCELL_REGISTER_SCHEMA} document",
            register_path.display()
        )));
    }
    let cell = register["workcells"]
        .as_array()
        .and_then(|cells| cells.iter().find(|cell| cell["name"] == workcell))
        .ok_or_else(|| landing_refused(format!("the register names no workcell {workcell}")))?;
    let declared = &cell["products"][product];
    let seat = cell["seats"]
        .as_array()
        .and_then(|seats| seats.iter().find(|seat| seat["product"] == product))
        .filter(|_| declared.is_object())
        .ok_or_else(|| landing_refused(format!("{workcell} holds no seat of {product}")))?;
    match seat["state"].as_str() {
        Some("claimed") => {}
        Some("free") => {
            return Err(landing_refused(format!(
                "{workcell}'s {product} seat is free: it holds no lane to land (a lane joins it with workcell.seat.join)"
            )))
        }
        other => {
            return Err(landing_refused(format!(
                "{workcell}'s {product} seat is {}: only the lane that holds a seat lands from it",
                other.unwrap_or("of no state")
            )))
        }
    }
    let checkout = PathBuf::from(cell["path"].as_str().unwrap_or_default())
        .join(declared["path"].as_str().unwrap_or(product));
    if !checkout.is_dir() {
        return Err(landing_refused(format!(
            "{workcell}'s {product} seat names {}, which is not there",
            checkout.display()
        )));
    }
    let branch =
        declare_git(&checkout, &["symbolic-ref", "--quiet", "--short", "HEAD"]).map_err(|_| {
            landing_refused(format!(
                "{} stands on no branch: a lane's work is committed on its branch",
                checkout.display()
            ))
        })?;
    if let Some(lane) = seat["branch"].as_str().filter(|lane| *lane != branch) {
        return Err(landing_refused(format!(
            "{} stands on {branch}, but the register's lane is {lane}: the seat is read again before it lands",
            checkout.display()
        )));
    }
    Ok(LaneSeat {
        world: world.to_path_buf(),
        workcell: workcell.to_owned(),
        product: product.to_owned(),
        checkout,
        branch,
        main_ref: declared["main_ref"]
            .as_str()
            .unwrap_or("origin/main")
            .to_owned(),
    })
}

/// git in a seat's checkout, its words kept on refusal; nothing is asked of a
/// terminal.
fn lane_git(seat: &LaneSeat, args: &[&str]) -> Result<String, WorkcellError> {
    declare_git(&seat.checkout, args)
}

fn lane_remote(seat: &LaneSeat) -> Result<String, WorkcellError> {
    declare_git(&seat.checkout, &["remote", "get-url", "origin"]).map_err(|_| {
        landing_refused(format!(
            "{}'s primary has no remote (origin): its work stays on this machine",
            seat.product
        ))
    })
}

fn gh_on_path() -> bool {
    env::var_os("PATH")
        .is_some_and(|path| env::split_paths(&path).any(|dir| dir.join("gh").is_file()))
}

/// A push to the project's remote. One to GitHub over https goes through GitHub's
/// CLI's credential helper where `gh` is installed, as this machine's seats push.
fn lane_push_to(seat: &LaneSeat, remote: &str, refspec: &str) -> Result<(), WorkcellError> {
    let mut args = Vec::new();
    if remote.starts_with("https://github.com/") && gh_on_path() {
        args.extend([
            "-c",
            "credential.helper=",
            "-c",
            "credential.helper=!gh auth git-credential",
        ]);
    }
    args.extend(["push", "--quiet", "origin", refspec]);
    lane_git(seat, &args).map(|_| ())
}

fn landing(seat: &LaneSeat, act: &str, head: &str) -> Value {
    json!({
        "schema": LANDING_SCHEMA,
        "act": act,
        "world": seat.world,
        "workcell": seat.workcell,
        "product": seat.product,
        "checkout": seat.checkout,
        "branch": seat.branch,
        "head": head,
    })
}

/// An env seat's `target`: the seat instrument's link into the primary's
/// shared build tree (its ensure_target_pool), the instrument's artifact and
/// not the lane's work. A repository whose .gitignore names `target/`
/// (directories only) does not ignore the link, so a commit leaves it out by
/// name, as the instrument's own reading of a seat's files does
/// (drop_pool_link).
const SEAT_POOL_LINK: &str = "target";

/// Commit: the seat's working files, every change and every untracked file
/// git does not ignore, but the seat instrument's link, as one commit on the
/// lane's branch, under the identity the repository's git names.
fn lane_commit(seat: &LaneSeat, message: &str) -> Result<Value, WorkcellError> {
    if message.trim().is_empty() {
        return Err(landing_refused(
            "a commit says what it holds: its message is empty",
        ));
    }
    let pool_link = seat.checkout.join(SEAT_POOL_LINK).is_symlink();
    let changes = lane_git(seat, &["status", "--porcelain=v1", "--untracked-files=all"])?;
    let files = changes
        .lines()
        .filter(|line| !line.trim().is_empty())
        .filter(|line| !(pool_link && line.get(3..) == Some(SEAT_POOL_LINK)))
        .count();
    if files == 0 {
        return Err(landing_refused(format!(
            "nothing to commit: {} holds no change on {}",
            seat.checkout.display(),
            seat.branch
        )));
    }
    let leave_out = format!(":(exclude){SEAT_POOL_LINK}");
    let mut add = vec!["add", "--all"];
    if pool_link {
        add.extend(["--", ".", leave_out.as_str()]);
    }
    lane_git(seat, &add)?;
    lane_git(seat, &["commit", "--quiet", "--message", message])?;
    let head = lane_git(seat, &["rev-parse", "HEAD"])?;
    let mut answer = landing(seat, "commit", &head);
    answer["files"] = files.into();
    answer["subject"] = message.lines().next().unwrap_or_default().into();
    Ok(answer)
}

/// Push: the lane's branch to the project's remote, as the branch it is.
fn lane_push(seat: &LaneSeat) -> Result<Value, WorkcellError> {
    let remote = lane_remote(seat)?;
    let head = lane_git(seat, &["rev-parse", "HEAD"])?;
    lane_push_to(seat, &remote, &format!("HEAD:refs/heads/{}", seat.branch))?;
    lane_git(
        seat,
        &[
            "branch",
            "--quiet",
            "--set-upstream-to",
            &format!("origin/{}", seat.branch),
        ],
    )?;
    let pushed = lane_git(
        seat,
        &["rev-parse", &format!("refs/remotes/origin/{}", seat.branch)],
    )?;
    if pushed != head {
        return Err(WorkcellError::OperationFailed(format!(
            "origin/{} reads {pushed} after the push, not the lane's head {head}",
            seat.branch
        )));
    }
    let mut answer = landing(seat, "push", &head);
    answer["remote"] = remote.into();
    answer["upstream"] = format!("origin/{}", seat.branch).into();
    Ok(answer)
}

/// Land, as Land as says: `merge` brings the lane into the project's main
/// (its remote's where the project's main is its remote's, else the primary's
/// own, fast-forward only); `draft` and `mr` open a merge request on GitHub
/// from the pushed branch, and merge nothing.
fn lane_land(seat: &LaneSeat, land_as: &str) -> Result<Value, WorkcellError> {
    if !matches!(land_as, "draft" | "mr" | "merge") {
        return Err(landing_refused(format!(
            "Land as is draft, mr or merge, not `{land_as}`"
        )));
    }
    let head = lane_git(seat, &["rev-parse", "HEAD"])?;
    let ahead = lane_git(
        seat,
        &["rev-list", "--count", &format!("{}..HEAD", seat.main_ref)],
    )?;
    if ahead == "0" {
        return Err(landing_refused(format!(
            "the lane holds no commit {} does not: nothing to land",
            seat.main_ref
        )));
    }
    let on_remote = seat.main_ref.starts_with("origin/");
    let pushed = || -> Result<String, WorkcellError> {
        let remote = lane_remote(seat)?;
        match lane_git(seat, &["rev-parse", "--verify", "--quiet", &format!("refs/remotes/origin/{}", seat.branch)]) {
            Ok(tip) if tip == head => Ok(remote),
            _ => Err(landing_refused(format!(
                "the lane's head is not on origin: push it first (Push: On land), then land it as {land_as}"
            ))),
        }
    };
    let mut answer = landing(seat, "land", &head);
    answer["as"] = land_as.into();
    answer["ahead"] = ahead.parse::<u64>().unwrap_or_default().into();
    if land_as == "merge" && !on_remote {
        // The project's main is the primary's own: it moves to the lane's head, fast-forward only. Git refuses where main has
        // moved on, or where the primary's working files would be overwritten; nothing is tidied to let it.
        let primary = PathBuf::from(lane_git(
            seat,
            &["rev-parse", "--path-format=absolute", "--git-common-dir"],
        )?)
        .parent()
        .map(Path::to_path_buf)
        .ok_or_else(|| {
            WorkcellError::OperationFailed("the seat's repository has no primary".into())
        })?;
        let standing = declare_git(&primary, &["symbolic-ref", "--quiet", "--short", "HEAD"])
            .unwrap_or_default();
        if standing != "main" {
            return Err(landing_refused(format!(
                "{} stands on {}, not main: a merge lands in the main the primary stands on",
                primary.display(),
                if standing.is_empty() {
                    "no branch"
                } else {
                    &standing
                }
            )));
        }
        declare_git(&primary, &["merge", "--ff-only", "--quiet", &head])?;
        answer["merged_into"] = "main".into();
        answer["primary"] = primary.to_string_lossy().into_owned().into();
        return Ok(answer);
    }
    let remote = pushed()?;
    if land_as == "merge" {
        // The project's main is its remote's: the remote's main moves to the lane's head, fast-forward only, as the remote's
        // own rules allow (a protected main refuses it in the remote's words).
        lane_push_to(seat, &remote, &format!("{head}:refs/heads/main"))?;
        lane_git(seat, &["fetch", "--quiet", "origin", "main"])?;
        answer["merged_into"] = "origin/main".into();
        answer["remote"] = remote.into();
        return Ok(answer);
    }
    if !remote.contains("github.com") {
        return Err(landing_refused(format!(
            "{remote} is not on GitHub: Workcell opens merge requests with GitHub's CLI, on GitHub"
        )));
    }
    if !gh_on_path() {
        return Err(landing_refused(
            "gh is not on PATH: Workcell opens merge requests with GitHub's CLI",
        ));
    }
    let subject = lane_git(seat, &["log", "-1", "--format=%s", "HEAD"])?;
    let mut request = std::process::Command::new("gh");
    request
        .args([
            "pr",
            "create",
            "--head",
            &seat.branch,
            "--base",
            "main",
            "--title",
            &subject,
        ])
        .args([
            "--body",
            &format!(
                "Landed from {}'s {} seat by Workcell.",
                seat.workcell, seat.product
            ),
        ])
        .current_dir(&seat.checkout)
        .env("GH_PROMPT_DISABLED", "1")
        .stdin(std::process::Stdio::null());
    if land_as == "draft" {
        request.arg("--draft");
    }
    let output = request
        .output()
        .map_err(|error| WorkcellError::OperationFailed(format!("gh could not start: {error}")))?;
    if !output.status.success() {
        return Err(WorkcellError::OperationFailed(format!(
            "gh pr create refused: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        )));
    }
    answer["merge_request"] = String::from_utf8_lossy(&output.stdout)
        .trim()
        .to_owned()
        .into();
    answer["remote"] = remote.into();
    Ok(answer)
}

fn command_lane(_global: &GlobalArgs, args: &[String]) -> Result<(), WorkcellError> {
    let usage = "usage: workcell lane commit|push|land --workcell <name> --product <name> [--world <World>] \
                 [--message <words> (commit)] [--as draft|mr|merge (land)]";
    let act = args.first().map(String::as_str).unwrap_or_default();
    if !matches!(act, "commit" | "push" | "land") {
        return Err(landing_refused(usage));
    }
    let parsed = lane_args(act, &args[1..])?;
    let (Some(workcell), Some(product)) = (parsed.workcell.as_deref(), parsed.product.as_deref())
    else {
        return Err(landing_refused(usage));
    };
    let world = lane_world(parsed.world.as_deref())?;
    let seat = lane_seat(&world, workcell, product)?;
    let answer = match act {
        "commit" => lane_commit(&seat, parsed.message.as_deref().unwrap_or_default())?,
        "push" => lane_push(&seat)?,
        _ => lane_land(
            &seat,
            parsed.land_as.as_deref().ok_or_else(|| {
                landing_refused("lane land needs `--as draft|mr|merge`: how the work lands")
            })?,
        )?,
    };
    emit_json(answer);
    Ok(())
}
