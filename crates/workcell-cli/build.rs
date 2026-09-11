fn main() {
    // The suite's build identity: the exact source revision compiled in, so
    // any installed binary answers what it is without its build tree. No
    // rerun-if directives: cargo reruns this script when any file in the
    // package changes, so the stamp stays true of the compiled code — a
    // workflow- or docs-only commit keeps the older stamp, which remains true.
    // SUITE_BUILD_REVISION overrides the probe for reproducible out-of-tree
    // builds where no .git exists.
    println!("cargo:rustc-env=SUITE_BUILD_REVISION={}", build_revision());
}

fn build_revision() -> String {
    if let Ok(revision) = std::env::var("SUITE_BUILD_REVISION") {
        if !revision.is_empty() {
            return revision;
        }
    }
    std::process::Command::new("git")
        .args(["rev-parse", "--short=12", "HEAD"])
        .output()
        .ok()
        .filter(|output| output.status.success())
        .map(|output| String::from_utf8_lossy(&output.stdout).trim().to_owned())
        .filter(|revision| !revision.is_empty())
        .unwrap_or_else(|| "unknown".to_owned())
}
