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
/// on, the main its lanes are cut from and land in, the actors on its lane
/// (its `workers`, else the one who `claimed_by` it, as the seat instrument
/// reads them) and the zones its claim records (the regions the lane commits
/// inside; lane_regions reads them).
struct LaneSeat {
    world: PathBuf,
    workcell: String,
    product: String,
    checkout: PathBuf,
    branch: String,
    main_ref: String,
    actors: Vec<String>,
    zones: Value,
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
        actors: match seat["workers"].as_array().filter(|workers| !workers.is_empty()) {
            Some(workers) => workers
                .iter()
                .filter_map(Value::as_str)
                .map(str::to_owned)
                .collect(),
            None => seat["claimed_by"]
                .as_str()
                .map(str::to_owned)
                .into_iter()
                .collect(),
        },
        zones: seat["zones"].clone(),
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

/// A remote's URL as an answer or a refusal shows it: without its userinfo,
/// where a credential rides (`https://user:secret@host/path` is shown
/// `https://host/path`, `user@host:path` is shown `host:path`), as git's own
/// messages show a remote. The userinfo ends at the last `@` before the first
/// `/` after the scheme. git pushes to the URL as it is.
fn shown_remote(url: &str) -> String {
    if let Some(scheme) = url.find("://") {
        let (head, rest) = url.split_at(scheme + 3);
        let authority = rest.find('/').unwrap_or(rest.len());
        return match rest[..authority].rfind('@') {
            Some(at) => format!("{head}{}", &rest[at + 1..]),
            None => url.to_owned(),
        };
    }
    // scp-like `[user@]host:path`, a `:` before any `/` (a local path has none).
    match url.find(':') {
        Some(colon) if !url[..colon].contains('/') => match url[..colon].rfind('@') {
            Some(at) => url[at + 1..].to_owned(),
            None => url.to_owned(),
        },
        _ => url.to_owned(),
    }
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
/// not the lane's work. Where git does not ignore the link (a .gitignore that
/// names `target/` matches directories only), `git status` lists it and a
/// commit leaves it out by name, as the instrument's own reading of a seat's
/// files does (drop_pool_link). Where git ignores it (`/target`, `target`,
/// info/exclude: every env seat here), git lists it nowhere, and the commit
/// names it nowhere: a pathspec naming an ignored path makes `git add` refuse.
const SEAT_POOL_LINK: &str = "target";

/// One change in a seat's checkout as `git status` reads it: its two-letter
/// code and its path, as git names it.
struct LaneChange {
    code: [u8; 2],
    path: Vec<u8>,
}

impl LaneChange {
    fn shown(&self) -> String {
        String::from_utf8_lossy(&self.path).into_owned()
    }

    /// A path git holds in conflict (`git status`'s unmerged codes).
    fn unmerged(&self) -> bool {
        matches!(
            &self.code,
            b"DD" | b"AU" | b"UD" | b"UA" | b"DU" | b"AA" | b"UU"
        )
    }

    /// A path no longer among the working files as git knew it: deleted, or
    /// replaced by a folder (its deletion staged or not). It leaves the index
    /// by name, for git adds only what stands.
    fn gone(&self) -> bool {
        self.code[1] == b'D' || &self.code == b"D "
    }

    /// The path as the index names it: an untracked repository inside the
    /// checkout is listed as its folder (`nested/`), and the index holds it as
    /// `nested`.
    fn indexed(&self) -> &[u8] {
        self.path.strip_suffix(b"/").unwrap_or(&self.path)
    }
}

/// An index file of a commit's own, removed when the commit is done, however
/// it ends.
struct LaneIndex(PathBuf);

impl Drop for LaneIndex {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.0);
    }
}

/// Paths, each ended by a NUL, as git reads them on its stdin with `-z`.
fn nul_ended<'a>(paths: impl IntoIterator<Item = &'a [u8]>) -> Vec<u8> {
    paths
        .into_iter()
        .flat_map(|path| path.iter().copied().chain([0]))
        .collect()
}

/// Paths in words: the first dozen, and how many more.
fn listed(paths: &[String]) -> String {
    const SHOWN: usize = 12;
    let shown = paths
        .iter()
        .take(SHOWN)
        .map(String::as_str)
        .collect::<Vec<_>>()
        .join(", ");
    match paths.len().saturating_sub(SHOWN) {
        0 => shown,
        more => format!("{shown} and {more} more"),
    }
}

/// What stops a commit before anything is staged: an operation under way that
/// a commit would conclude (a merge, cherry-pick, revert or rebase, read where
/// git keeps it for this checkout, so a linked checkout reads its own), and a
/// path in conflict, which staging would record with its conflict markers.
/// The operation is left as it stands, to be resolved and concluded or
/// aborted.
fn lane_settled(seat: &LaneSeat, changes: &[LaneChange]) -> Result<(), WorkcellError> {
    const UNDER_WAY: [(&str, &str); 5] = [
        ("MERGE_HEAD", "a merge"),
        ("CHERRY_PICK_HEAD", "a cherry-pick"),
        ("REVERT_HEAD", "a revert"),
        ("rebase-merge", "a rebase"),
        ("rebase-apply", "a rebase or git am"),
    ];
    let mut args = vec!["rev-parse", "--path-format=absolute"];
    for (name, _) in UNDER_WAY {
        args.extend(["--git-path", name]);
    }
    let kept = lane_git(seat, &args)?;
    let under_way = UNDER_WAY
        .iter()
        .zip(kept.lines())
        .filter(|(_, path)| Path::new(path).exists())
        .map(|((name, operation), _)| format!("{operation} ({name})"))
        .collect::<Vec<_>>();
    let unmerged = changes
        .iter()
        .filter(|change| change.unmerged())
        .map(LaneChange::shown)
        .collect::<Vec<_>>();
    let state = match (under_way.is_empty(), unmerged.is_empty()) {
        (true, true) => return Ok(()),
        (false, true) => format!("is in the middle of {}", under_way.join(" and ")),
        (false, false) => format!(
            "is in the middle of {}, with unmerged {}",
            under_way.join(" and "),
            listed(&unmerged)
        ),
        (true, false) => format!("holds unmerged {}", listed(&unmerged)),
    };
    Err(landing_refused(format!(
        "{} {state}: a lane's commit never concludes what is under way or records conflict markers; resolve it, and conclude or abort it, first. Nothing was staged or committed",
        seat.checkout.display()
    )))
}

/// The checkout's changes: every change and every untracked file git does not
/// ignore, a rename read as its two sides.
fn lane_changes(seat: &LaneSeat) -> Result<Vec<LaneChange>, WorkcellError> {
    let status = git_bytes(
        &seat.checkout,
        &[
            "status",
            "--porcelain=v1",
            "-z",
            "--untracked-files=all",
            "--no-renames",
        ],
        None,
    )?;
    Ok(status
        .split(|byte| *byte == 0)
        .filter(|record| record.len() > 3 && record[2] == b' ')
        .map(|record| LaneChange {
            code: [record[0], record[1]],
            path: record[3..].to_vec(),
        })
        .collect())
}

/// The regions a lane's commit is cut by: the lane's own, and those of the
/// zones the seat keeps for lanes no longer on it.
#[derive(Default)]
struct LaneRegions {
    /// The regions of the zones of the actors on the lane; none where the
    /// lane holds the seat whole.
    held: Vec<String>,
    /// The regions of the zones of actors no longer on the lane (a
    /// `--new-lane` claim displaced them, and the instrument keeps their
    /// zones): never the lane's, whatever the lane holds.
    displaced: Vec<String>,
}

impl LaneRegions {
    /// Whether a change is the lane's: inside its regions (all of the
    /// checkout, where it holds the seat whole), and inside no zone kept for
    /// a displaced lane.
    fn take(&self, path: &[u8]) -> bool {
        (self.held.is_empty() || self.held.iter().any(|region| in_region(path, region)))
            && !self.displaced.iter().any(|region| in_region(path, region))
    }
}

/// The lane's regions, as the seat instrument records and reads them: the
/// regions of the zones of the actors on the lane (`zones`, each `{actor,
/// regions, since_unix_seconds}` as `seat join --region` writes it), with a
/// trailing `/` dropped and `.` the whole checkout, as its `region_of` reads a
/// region. None: the lane holds the seat whole, as where an actor on it has no
/// zone, for the instrument reads no region as `.`. A zone of an actor no
/// longer on the lane (one a `--new-lane` claim displaced, which the
/// instrument keeps, with that lane's unfinished work in the shared tree) is
/// not the lane's, and what lies inside it stays out of the lane's commit even
/// where the lane holds the seat whole. A region that cannot be read as a path
/// (free words, several paths in one string) is refused by name, for a commit
/// is never cut to a guess.
fn lane_regions(seat: &LaneSeat) -> Result<LaneRegions, WorkcellError> {
    let unreadable = || {
        landing_refused(format!(
            "{}'s {} seat records zones that are no list of {{actor, regions}}: {}",
            seat.workcell, seat.product, seat.zones
        ))
    };
    let zones = match &seat.zones {
        Value::Null => return Ok(LaneRegions::default()),
        Value::Array(zones) => zones,
        _ => return Err(unreadable()),
    };
    // A zone's regions as paths: each written region read as one, or refused naming whose zone it is.
    let regions_of = |zone: &Value, whose: &str| -> Result<Vec<String>, WorkcellError> {
        let mut read = Vec::new();
        for written in zone["regions"].as_array().ok_or_else(unreadable)? {
            let written = written.as_str().ok_or_else(unreadable)?;
            let region = match written.trim_end_matches('/') {
                "" => ".",
                region => region,
            };
            if !lane_reads_as_a_path(seat, region) {
                return Err(landing_refused(format!(
                    "the region `{written}` ({whose}) names no path in {}: a commit is cut by the seat's zones, and this region cannot be read as a path. Nothing was staged or committed",
                    seat.checkout.display()
                )));
            }
            read.push(region.to_owned());
        }
        Ok(read)
    };
    let add = |regions: &mut Vec<String>, region: String| {
        if !regions.contains(&region) {
            regions.push(region);
        }
    };
    let mut regions = LaneRegions::default();
    let mut whole = false;
    for actor in &seat.actors {
        let mut zoned = false;
        for zone in zones.iter().filter(|zone| zone["actor"] == actor.as_str()) {
            for region in regions_of(zone, &format!("{actor}'s zone, on the lane"))? {
                zoned = true;
                add(&mut regions.held, region);
            }
        }
        whole |= !zoned;
    }
    for zone in zones
        .iter()
        .filter(|zone| !seat.actors.iter().any(|actor| zone["actor"] == actor.as_str()))
    {
        let whose = match zone["actor"].as_str() {
            Some(actor) => format!("the zone kept for {actor}, no longer on the lane"),
            None => "a zone kept for an actor not on the lane".to_owned(),
        };
        for region in regions_of(zone, &whose)? {
            add(&mut regions.displaced, region);
        }
    }
    if whole {
        regions.held.clear();
    }
    Ok(regions)
}

/// Whether a region can be read as a path in the checkout: `.` (the whole of
/// it), or a relative path in plain form (no empty, `.` or `..` step). One
/// that names nothing there yet, or nothing any longer, holds no change, as
/// the seat instrument's `region_of` reads it. Words with spaces between them
/// are read as a path only where one stands among the working files or what
/// git tracks.
fn lane_reads_as_a_path(seat: &LaneSeat, region: &str) -> bool {
    if region == "." {
        return true;
    }
    if region
        .split('/')
        .any(|step| matches!(step, "" | "." | ".."))
    {
        return false;
    }
    !region.chars().any(char::is_whitespace)
        || seat.checkout.join(region).symlink_metadata().is_ok()
        || lane_git(
            seat,
            &[
                "--literal-pathspecs",
                "ls-files",
                "--error-unmatch",
                "--with-tree=HEAD",
                "--",
                region,
            ],
        )
        .is_ok()
}

/// Whether a path lies inside a region, as the seat instrument's `region_of`
/// reads it: a region holds itself and what lies under it, and `.` holds
/// everything.
fn in_region(path: &[u8], region: &str) -> bool {
    region == "."
        || path
            .strip_prefix(region.as_bytes())
            .is_some_and(|rest| rest.is_empty() || rest[0] == b'/')
}

/// git in a seat's checkout on exactly `paths`, each as it is written (no
/// pathspec magic), handed to git on its stdin.
fn lane_git_paths(seat: &LaneSeat, args: &[&str], paths: &[&[u8]]) -> Result<(), WorkcellError> {
    let mut words = vec!["--literal-pathspecs"];
    words.extend_from_slice(args);
    words.extend(["--pathspec-from-file=-", "--pathspec-file-nul"]);
    git_bytes(&seat.checkout, &words, Some(&nul_ended(paths.iter().copied()))).map(|_| ())
}

/// The checkout's index held as git holds it to write it (its `index.lock`),
/// from before the lane's commit reads the branch's head until the commit
/// stands, and released however the commit ends. git's own commits in the
/// checkout hold that lock from before they read the head until theirs is
/// recorded, so none lands between the head the lane's commit is cut from and
/// the commit, to be taken back by it.
struct HeldIndex(PathBuf);

impl HeldIndex {
    fn hold(seat: &LaneSeat) -> Result<Self, WorkcellError> {
        let index = lane_git(
            seat,
            &["rev-parse", "--path-format=absolute", "--git-path", "index"],
        )?;
        let lock = PathBuf::from(format!("{index}.lock"));
        match fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&lock)
        {
            Ok(_) => Ok(HeldIndex(lock)),
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
                Err(landing_refused(format!(
                    "another git process is writing {}'s index ({} stands): the lane's paths are staged, and its commit is made once that process is done. Nothing was committed",
                    seat.checkout.display(),
                    lock.display()
                )))
            }
            Err(error) => Err(WorkcellError::OperationFailed(format!(
                "{} could not be held: {error}",
                lock.display()
            ))),
        }
    }
}

impl Drop for HeldIndex {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.0);
    }
}

/// The lane's changes staged, and committed as exactly those: each path is
/// staged in the checkout's index as it stands (one gone from the working
/// files leaves it by name, since git adds only what stands, and one under a
/// folder that is now a link lies beyond it), and the commit is made from an
/// index of its own, built from the branch's head with the entries git staged
/// for those paths taken over from the checkout's. git never reads the paths
/// from disk again at the commit, so a rename in case only, or a link swapped
/// for a folder or the other way, is committed as staged; what else the
/// checkout's index holds stays staged, and out of the commit. The head is
/// read once, with the checkout's index held (HeldIndex), and the commit is
/// one on that head or none: a head moved by what takes no index (a ref set by
/// hand) is refused, and the commit made on it taken back. The commit made.
fn lane_stage_and_commit(
    seat: &LaneSeat,
    changes: &[LaneChange],
    message: &str,
) -> Result<String, WorkcellError> {
    let (gone, standing): (Vec<&LaneChange>, Vec<&LaneChange>) =
        changes.iter().partition(|change| change.gone());
    if !gone.is_empty() {
        git_bytes(
            &seat.checkout,
            &["update-index", "-z", "--force-remove", "--stdin"],
            Some(&nul_ended(gone.iter().map(|change| change.indexed()))),
        )?;
    }
    if !standing.is_empty() {
        let paths = standing
            .iter()
            .map(|change| change.path.as_slice())
            .collect::<Vec<_>>();
        // `--force`: git add exits 1 on a path an ignore rule covers as a folder, tracked or not, though it stages it (a file
        // ai-kit tracks under its `/.aikit/` rule, JSON O-I tracks under a folder its `target` rule covers). git status lists no
        // new file the rules ignore, so none is among the lane's paths.
        lane_git_paths(seat, &["add", "--all", "--force"], &paths)?;
    }
    // Where git reads the checkout as ignoring case (every macOS seat), it stages a new file in a folder the index holds in
    // another case under the index's case (`?? Docs/x.md` staged as `docs/x.md`): a path and its entry are matched as git
    // matches them there.
    let folds = lane_git(
        seat,
        &["config", "--type=bool", "--default=false", "core.ignorecase"],
    )? == "true";
    let key = |path: &[u8]| {
        if folds {
            path.to_ascii_lowercase()
        } else {
            path.to_vec()
        }
    };
    let lane = changes
        .iter()
        .map(|change| key(change.indexed()))
        .collect::<BTreeSet<_>>();
    let held = HeldIndex::hold(seat)?;
    let head = lane_git(seat, &["rev-parse", "--verify", "HEAD^{commit}"])?;
    let index = LaneIndex(PathBuf::from(lane_git(
        seat,
        &[
            "rev-parse",
            "--path-format=absolute",
            "--git-path",
            &format!("workcell-lane-commit-{}.index", std::process::id()),
        ],
    )?));
    let own = Some(index.0.as_path());
    git_bytes_in(&seat.checkout, &["read-tree", &head], None, own)?;
    // The checkout's entries for the lane's paths (`<mode> <object> <stage>\t<path>`), as --index-info reads them.
    let staged = git_bytes(&seat.checkout, &["ls-files", "--stage", "-z"], None)?;
    let mut entries = Vec::new();
    let mut named = BTreeSet::new();
    let mut leaving = changes
        .iter()
        .map(|change| change.indexed().to_vec())
        .collect::<BTreeSet<_>>();
    for entry in staged.split(|byte| *byte == 0) {
        let Some(tab) = entry.iter().position(|byte| *byte == b'\t') else {
            continue;
        };
        let name = &entry[tab + 1..];
        if lane.contains(&key(name)) {
            named.insert(key(name));
            leaving.insert(name.to_vec());
            entries.extend(entry.iter().copied().chain([0]));
        }
    }
    let unstaged = standing
        .iter()
        .filter(|change| !named.contains(&key(change.indexed())))
        .map(|change| change.shown())
        .collect::<Vec<_>>();
    if !unstaged.is_empty() {
        return Err(landing_refused(format!(
            "git staged no entry for {} in {}'s index, so the commit would leave them out: nothing was committed",
            listed(&unstaged),
            seat.checkout.display()
        )));
    }
    git_bytes_in(
        &seat.checkout,
        &["update-index", "-z", "--force-remove", "--stdin"],
        Some(&nul_ended(leaving.iter().map(Vec::as_slice))),
        own,
    )?;
    if !entries.is_empty() {
        git_bytes_in(
            &seat.checkout,
            &["update-index", "-z", "--index-info"],
            Some(&entries),
            own,
        )?;
    }
    let tree = String::from_utf8_lossy(&git_bytes_in(
        &seat.checkout,
        &["write-tree"],
        None,
        own,
    )?)
    .trim()
    .to_owned();
    git_bytes_in(
        &seat.checkout,
        &["commit", "--quiet", "--message", message],
        None,
        own,
    )?;
    let made = lane_git(seat, &["rev-parse", "HEAD"])?;
    let read = lane_git(seat, &["rev-parse", &format!("{made}^{{tree}}"), &format!("{made}^")])?;
    let (made_tree, parent) = read.split_once('\n').unwrap_or((read.as_str(), ""));
    if made_tree != tree {
        return Err(landing_refused(format!(
            "{}'s head moved while the lane's commit was made, by what takes no index: it stands at {made}, which is not the lane's commit. Look at the branch before committing again",
            seat.checkout.display()
        )));
    }
    if parent != head {
        // The commit's tree was read from `head`, its parent is the head git found when it began: it would take back what moved it.
        let taken_back = lane_git(
            seat,
            &[
                "update-ref",
                "-m",
                "workcell: a lane's commit made on a head moved by hand, taken back",
                "HEAD",
                parent,
                &made,
            ],
        );
        drop(held);
        return Err(landing_refused(match taken_back {
            Ok(_) => format!(
                "{}'s head moved from {head} to {parent} while the lane's commit was made, by what takes no index (a ref set by hand): the commit would take back what moved it, so it is taken back and the head stands at {parent}. The lane's paths are staged; nothing was committed",
                seat.checkout.display()
            ),
            Err(error) => format!(
                "{}'s head moved from {head} to {parent} while the lane's commit {made} was made, and moved again before it could be taken back ({error}): {made} takes back what moved it. Look at the branch before committing again",
                seat.checkout.display()
            ),
        }));
    }
    drop(held);
    Ok(made)
}

/// Commit: the lane's work, as one commit on its branch, under the identity
/// the repository's git names. The lane's work is every change and every
/// untracked file git does not ignore, but the seat instrument's link; where
/// the lane's claim names regions (a shared checkout holds other lanes' and
/// workers' work beside the lane's), only the changes inside them, and the
/// answer names what it left outside. What lies inside a zone the seat keeps
/// for a lane no longer on it is never the lane's, and is left outside too.
/// The commit takes exactly those paths as they stand in the working files
/// (lane_stage_and_commit), leaving whatever else the index holds as it was.
/// Refused, with nothing staged, in the middle of a merge, cherry-pick, revert
/// or rebase, with a path in conflict, and with a region that cannot be read
/// as a path.
fn lane_commit(seat: &LaneSeat, message: &str) -> Result<Value, WorkcellError> {
    if message.trim().is_empty() {
        return Err(landing_refused(
            "a commit says what it holds: its message is empty",
        ));
    }
    let changes = lane_changes(seat)?;
    lane_settled(seat, &changes)?;
    let regions = lane_regions(seat)?;
    let pool_link = seat.checkout.join(SEAT_POOL_LINK).is_symlink();
    let (changes, outside): (Vec<_>, Vec<_>) = changes
        .into_iter()
        .filter(|change| !(pool_link && change.path == SEAT_POOL_LINK.as_bytes()))
        .partition(|change| regions.take(&change.path));
    let left_outside = outside
        .iter()
        .map(LaneChange::shown)
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect::<Vec<_>>();
    if changes.is_empty() {
        let mut within = Vec::new();
        if !regions.held.is_empty() {
            within.push(format!(
                "inside the lane's regions ({})",
                regions.held.join(", ")
            ));
        }
        if !regions.displaced.is_empty() {
            within.push(format!(
                "outside the zones the seat keeps for lanes no longer on it ({})",
                regions.displaced.join(", ")
            ));
        }
        let within = match within.is_empty() {
            true => String::new(),
            false => format!(" {}", within.join(" and ")),
        };
        let left = if left_outside.is_empty() {
            String::new()
        } else {
            format!("; left as they are: {}", listed(&left_outside))
        };
        return Err(landing_refused(format!(
            "nothing to commit: {} holds no change{within} on {}{left}",
            seat.checkout.display(),
            seat.branch
        )));
    }
    let head = lane_stage_and_commit(seat, &changes, message)?;
    let paths = changes
        .iter()
        .map(|change| change.path.as_slice())
        .collect::<BTreeSet<_>>();
    let mut answer = landing(seat, "commit", &head);
    answer["files"] = paths.len().into();
    answer["subject"] = message.lines().next().unwrap_or_default().into();
    answer["regions"] = regions.held.into();
    answer["left_outside"] = left_outside.into();
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
    answer["remote"] = shown_remote(&remote).into();
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
        answer["remote"] = shown_remote(&remote).into();
        return Ok(answer);
    }
    if !remote.contains("github.com") {
        return Err(landing_refused(format!(
            "{} is not on GitHub: Workcell opens merge requests with GitHub's CLI, on GitHub",
            shown_remote(&remote)
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
    answer["remote"] = shown_remote(&remote).into();
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

#[cfg(test)]
mod landing_tests {
    use super::*;

    #[test]
    fn a_path_lies_inside_the_lanes_regions_as_the_seat_instrument_reads_them() {
        let regions = LaneRegions {
            held: vec!["crates/a".to_owned(), "docs".to_owned()],
            displaced: vec!["docs/kept".to_owned()],
        };
        for (path, inside) in [
            ("crates/a", true),
            ("crates/a/lib.rs", true),
            ("crates/ab/lib.rs", false),
            ("crates", false),
            ("docs/guide/one.md", true),
            ("docs/kept", false),
            ("docs/kept/one.md", false),
            ("docs/keepsake.md", true),
            ("README.md", false),
        ] {
            assert_eq!(regions.take(path.as_bytes()), inside, "{path}");
        }
        assert!(
            LaneRegions::default().take(b"README.md"),
            "no region: the whole checkout"
        );
        let dot = LaneRegions {
            held: vec![".".to_owned()],
            displaced: Vec::new(),
        };
        assert!(dot.take(b"README.md"));
        let whole = LaneRegions {
            held: Vec::new(),
            displaced: vec!["crates/b".to_owned()],
        };
        assert!(whole.take(b"crates/n/lib.rs"), "the whole seat");
        assert!(
            !whole.take(b"crates/b/.env.local"),
            "but never a zone kept for a displaced lane"
        );
    }

    #[test]
    fn a_remote_is_shown_without_its_userinfo() {
        for (url, shown) in [
            (
                "https://x-access-token:tok3n@github.com/EpiLogos/X.git",
                "https://github.com/EpiLogos/X.git",
            ),
            ("https://oauth2:tok3n@gitlab.com/g/x.git", "https://gitlab.com/g/x.git"),
            ("https://user:p@ss@host.invalid/x.git", "https://host.invalid/x.git"),
            ("https://org@dev.azure.com/org/p/_git/r", "https://dev.azure.com/org/p/_git/r"),
            ("ssh://git@host.invalid:22/x.git", "ssh://host.invalid:22/x.git"),
            ("file://landing:tok3n@/srv/x.git", "file:///srv/x.git"),
            ("git@github.com:EpiLogos/X.git", "github.com:EpiLogos/X.git"),
            ("https://github.com/EpiLogos/X.git", "https://github.com/EpiLogos/X.git"),
            ("https://host.invalid/a@b/x.git", "https://host.invalid/a@b/x.git"),
            ("/srv/repos/x@y.git", "/srv/repos/x@y.git"),
            ("../x.git", "../x.git"),
        ] {
            assert_eq!(shown_remote(url), shown, "{url}");
        }
    }
}
