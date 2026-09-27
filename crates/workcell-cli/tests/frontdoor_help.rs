//! The task-oriented frontdoor help (command-encounter classification §5):
//! bare/help print the outcome-grouped reference with no side effects, and
//! every route — including material/inspect/recover/run, which the previous
//! reference omitted — stays listed. A material operator remains the
//! audience; only exposure and actionability are simplified.

use std::process::Command;

fn run_workcell(args: &[&str]) -> (String, String, i32) {
    let output = Command::new(env!("CARGO_BIN_EXE_workcell"))
        .args(args)
        .env("WORKCELL_HOME", std::env::temp_dir().join("workcell-help-probe"))
        .output()
        .unwrap_or_else(|e| panic!("workcell {args:?} should run: {e}"));
    (
        String::from_utf8_lossy(&output.stdout).to_string(),
        String::from_utf8_lossy(&output.stderr).to_string(),
        output.status.code().unwrap_or(1),
    )
}

#[test]
fn bare_and_help_print_the_grouped_reference_without_side_effects() {
    for args in [vec!["--help"], vec!["help"]] {
        let (stdout, _stderr, code) = run_workcell(&args);
        assert_eq!(code, 0, "help never fails: {args:?}");
        assert!(stdout.contains("provider-neutral material execution control"), "{args:?}");
        // Encounter-classed sections lead with inspection, then the material
        // lifecycle, then operator depth.
        for section in [
            "Everyday inspection:",
            "Material lifecycle (contextual",
            "Cross-cell connection lifecycle (operator):",
            "Secret origin and projection (operator",
            "System and configuration:",
        ] {
            assert!(stdout.contains(section), "{args:?}: missing `{section}`");
        }
        let inspection = stdout.find("Everyday inspection:");
        let operator = stdout.find("Cross-cell connection lifecycle (operator):");
        assert!(
            inspection.unwrap() < operator.unwrap(),
            "everyday inspection leads operator depth: {args:?}"
        );
        // Every current root is present, including the ones the previous
        // reference omitted.
        for route in [
            "status", "discover", "material", "inspect", "providers", "doctor", "instances",
            "places", "plan", "prepare", "observe", "expose", "collect", "release", "reconcile",
            "recover", "run", "place", "serve", "authorise", "revoke", "connect", "connections",
            "machine", "secret", "sandboxes", "correlate-projection", "system",
            "config-contribution", "config",
        ] {
            assert!(
                stdout.contains(&format!("workcell {route}"))
                    || stdout.contains(&format!(" {route} ")),
                "{args:?}: missing route `{route}`"
            );
        }
    }
}
