//! workcell-secret-scan: exposure discovery for credential material that
//! lives outside a secret provider — environment variables, shell rc files,
//! and well-known auth files.
//!
//! The scanner *detects*; it never collects. Findings carry location and
//! key-name only — the matched value is deliberately not representable in
//! any finding type, so no rendering of a report can leak material.
//! Detection failure is declared: a file that exists but cannot be read, or
//! an auth file that cannot be parsed, becomes a `ScanRefusal` in the
//! report — never a silent skip, and never an empty success.
//!
//! `reference_only` marks values that look like an indirection (a command
//! substitution, or a `keychain://` / `op://` / `linux-secret-service://`
//! ref) rather than raw material: they are still reported, because the
//! scan's job is to make exposure visible, but the human sees that the line
//! already points at a vault.

use std::fmt;

pub const SECRET_SCAN_SCHEMA: &str = "workcell.secret-exposure-scan/v1";

/// What kind of surface the finding came from. Plain facts, no ontology.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub enum ExposureKind {
    EnvironmentVariable,
    ShellRcAssignment,
    JsonAuthFile,
    KeyValueAuthFile,
    Netrc,
}

impl ExposureKind {
    pub fn label(&self) -> &'static str {
        match self {
            Self::EnvironmentVariable => "environment variable",
            Self::ShellRcAssignment => "shell rc assignment",
            Self::JsonAuthFile => "json auth file",
            Self::KeyValueAuthFile => "key/value auth file",
            Self::Netrc => "netrc",
        }
    }
}

/// One exposure candidate. There is no field here that can carry the
/// matched value — that is the boundary, expressed as a type.
#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct ExposureFinding {
    pub kind: ExposureKind,
    /// `env:NAME` for environment variables, the file path otherwise.
    pub location: String,
    /// The variable or key name, or a dotted JSON path. Never a value.
    pub key: String,
    pub line: Option<usize>,
    /// The value looks like a reference into a vault (command substitution
    /// or a provider ref), not raw material.
    pub reference_only: bool,
}

/// A declared detection failure: the scan saw the target and could not run
/// detection on it. Never silent, never an empty result.
#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct ScanRefusal {
    pub target: String,
    pub reason: String,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct ScanReport {
    pub findings: Vec<ExposureFinding>,
    pub refusals: Vec<ScanRefusal>,
    /// Targets detection actually ran on.
    pub scanned: Vec<String>,
}

impl ScanReport {
    pub fn merge(&mut self, other: ScanReport) {
        self.findings.extend(other.findings);
        self.refusals.extend(other.refusals);
        self.scanned.extend(other.scanned);
    }

    /// Structured report. Contains locations and key names only.
    pub fn to_json(&self) -> serde_json::Value {
        serde_json::json!({
            "schema": SECRET_SCAN_SCHEMA,
            "findings": self.findings.iter().map(|f| serde_json::json!({
                "kind": f.kind.label(),
                "location": f.location,
                "key": f.key,
                "line": f.line,
                "reference_only": f.reference_only,
            })).collect::<Vec<_>>(),
            "refusals": self.refusals.iter().map(|r| serde_json::json!({
                "target": r.target,
                "reason": r.reason,
            })).collect::<Vec<_>>(),
            "scanned": self.scanned.len(),
        })
    }

    /// Human report. Locations and key names only, by construction.
    pub fn render_plain(&self) -> String {
        let mut out = String::new();
        if self.findings.is_empty() {
            out.push_str("exposure scan: no candidate credential material found\n");
        } else {
            out.push_str(&format!(
                "exposure scan: {} candidate{} outside a secret provider\n",
                self.findings.len(),
                if self.findings.len() == 1 { "" } else { "s" }
            ));
            for finding in &self.findings {
                let line = finding
                    .line
                    .map(|n| format!(":{n}"))
                    .unwrap_or_default();
                let reference = if finding.reference_only {
                    " [reference into a vault, not raw material]"
                } else {
                    ""
                };
                out.push_str(&format!(
                    "  [{}] {}{}  key `{}` ({}){}\n",
                    match finding.kind {
                        ExposureKind::EnvironmentVariable => "env",
                        ExposureKind::ShellRcAssignment => "rc",
                        ExposureKind::JsonAuthFile => "auth",
                        ExposureKind::KeyValueAuthFile => "auth",
                        ExposureKind::Netrc => "netrc",
                    },
                    finding.location,
                    line,
                    finding.key,
                    finding.kind.label(),
                    reference,
                ));
            }
        }
        for refusal in &self.refusals {
            out.push_str(&format!(
                "  refusal: {} — {} (detection did not run; this is declared, not a silent skip)\n",
                refusal.target, refusal.reason
            ));
        }
        out.push_str(&format!(
            "targets scanned: {}\n",
            self.scanned.len()
        ));
        out
    }
}

impl fmt::Display for ScanReport {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.render_plain())
    }
}

/// Key-name heuristic: does this name look like it names credential
/// material? Matches on token presence in the normalised name, so
/// `GITHUB_TOKEN`, `my_service_api_key`, `oauth_token`, `AWS_SECRET_ACCESS_KEY`
/// and `client_secret` all match. The heuristic errs toward reporting: a
/// finding is a location to look at, not an accusation.
pub fn secretish_key(key: &str) -> bool {
    let normalised: String = key
        .chars()
        .filter(|c| c.is_ascii_alphanumeric())
        .map(|c| c.to_ascii_uppercase())
        .collect();
    if normalised.is_empty() {
        return false;
    }
    const TOKENS: [&str; 10] = [
        "APIKEY",
        "TOKEN",
        "SECRET",
        "PASSWORD",
        "PASSWD",
        "CREDENTIAL",
        "PASSPHRASE",
        "PRIVATEKEY",
        "ACCESSKEY",
        "AUTHORIZATION",
    ];
    if TOKENS.iter().any(|token| normalised.contains(token)) {
        return true;
    }
    // Bare names that credential files use for the material itself — an
    // auth file holding a field literally called `key` or `pass` is naming
    // the value, not describing a concept.
    if matches!(normalised.as_str(), "KEY" | "PASS" | "AUTH") {
        return true;
    }
    // A bare AUTH stem appears inside ordinary words (AUTHOR) as well as
    // credential names, so require a credential-shaped neighbour.
    normalised.contains("AUTH") && (normalised.contains("KEY") || normalised.contains("TOKEN"))
}

/// Values that are indirections into a vault rather than raw material.
pub fn reference_only_value(value: &str) -> bool {
    let trimmed = value.trim().trim_matches('"').trim_matches('\'').trim();
    trimmed.starts_with("$(")
        || trimmed.starts_with('`')
        || trimmed.starts_with("op://")
        || trimmed.starts_with("keychain://")
        || trimmed.starts_with("linux-secret-service://")
        || trimmed.starts_with("secret-provider:")
        || trimmed.contains("op read ")
        || trimmed.contains("secret-tool lookup")
        || trimmed.contains("pass show ")
}

fn is_assignment_key(key: &str) -> bool {
    let mut chars = key.chars();
    match chars.next() {
        Some(c) if c.is_ascii_alphabetic() || c == '_' => {}
        _ => return false,
    }
    chars.all(|c| c.is_ascii_alphanumeric() || c == '_')
}

/// Environment variables that name credential-adjacent things but carry no
/// material: socket paths, TTYs, helper paths.
const ENV_NON_MATERIAL: [&str; 4] = ["SSH_AUTH_SOCK", "XAUTHORITY", "GPG_TTY", "SUDO_ASKPASS"];

pub fn scan_environment<'a, I: IntoIterator<Item = (&'a str, &'a str)>>(
    vars: I,
) -> Vec<ExposureFinding> {
    let mut findings = Vec::new();
    for (name, value) in vars {
        if value.trim().is_empty() || ENV_NON_MATERIAL.contains(&name) {
            continue;
        }
        if secretish_key(name) {
            findings.push(ExposureFinding {
                kind: ExposureKind::EnvironmentVariable,
                location: format!("env:{name}"),
                key: name.to_owned(),
                line: None,
                reference_only: reference_only_value(value),
            });
        }
    }
    findings
}

/// Scan shell rc text (`export KEY=value`, `KEY=value`). Comment lines are
/// skipped; every other line's assignment shape is checked.
pub fn scan_shell_text(location: &str, text: &str) -> Vec<ExposureFinding> {
    let mut findings = Vec::new();
    for (index, raw_line) in text.lines().enumerate() {
        let line = raw_line.trim();
        if line.starts_with('#') || line.is_empty() {
            continue;
        }
        let candidate = line
            .strip_prefix("export ")
            .map(|rest| rest.trim_start())
            .unwrap_or(line);
        let Some((key, value)) = candidate.split_once('=') else {
            continue;
        };
        let key = key.trim();
        let value = value.trim();
        if !is_assignment_key(key) || value.is_empty() {
            continue;
        }
        if secretish_key(key) {
            findings.push(ExposureFinding {
                kind: ExposureKind::ShellRcAssignment,
                location: location.to_owned(),
                key: key.to_owned(),
                line: Some(index + 1),
                reference_only: reference_only_value(value),
            });
        }
    }
    findings
}

/// Walk a JSON auth file: any key on a secretish name holding a non-empty
/// string becomes a finding at its dotted path. Unparseable content is a
/// declared refusal.
pub fn scan_json_text(location: &str, text: &str) -> Result<Vec<ExposureFinding>, ScanRefusal> {
    let value: serde_json::Value = serde_json::from_str(text).map_err(|error| ScanRefusal {
        target: location.to_owned(),
        reason: format!("not valid JSON ({error}); detection cannot run on this file"),
    })?;
    let mut findings = Vec::new();
    walk_json(location, &value, String::new(), &mut findings);
    Ok(findings)
}

fn walk_json(location: &str, value: &serde_json::Value, path: String, findings: &mut Vec<ExposureFinding>) {
    match value {
        serde_json::Value::Object(map) => {
            for (key, child) in map {
                let child_path = if path.is_empty() {
                    key.clone()
                } else {
                    format!("{path}.{key}")
                };
                if let serde_json::Value::String(text) = child {
                    if !text.trim().is_empty() && secretish_key(key) {
                        findings.push(ExposureFinding {
                            kind: ExposureKind::JsonAuthFile,
                            location: location.to_owned(),
                            key: child_path,
                            line: None,
                            reference_only: reference_only_value(text),
                        });
                    }
                } else {
                    walk_json(location, child, child_path, findings);
                }
            }
        }
        serde_json::Value::Array(items) => {
            for (index, child) in items.iter().enumerate() {
                walk_json(location, child, format!("{path}[{index}]"), findings);
            }
        }
        _ => {}
    }
}

/// Scan `key = value`, `key: value` and netrc-style `keyword value` text
/// (INI/YAML-ish auth files, `~/.netrc`).
pub fn scan_keyvalue_text(location: &str, text: &str, kind: ExposureKind) -> Vec<ExposureFinding> {
    let mut findings = Vec::new();
    for (index, raw_line) in text.lines().enumerate() {
        let line = raw_line.trim();
        if line.starts_with('#') || line.is_empty() {
            continue;
        }
        if kind == ExposureKind::Netrc {
            // netrc lines are keyword sequences (`machine h login u password
            // p`): report every secretish keyword and the value after it.
            let tokens: Vec<&str> = line.split_whitespace().collect();
            let mut position = 0;
            while position + 1 < tokens.len() {
                if secretish_key(tokens[position]) {
                    findings.push(ExposureFinding {
                        kind,
                        location: location.to_owned(),
                        key: tokens[position].to_owned(),
                        line: Some(index + 1),
                        reference_only: reference_only_value(tokens[position + 1]),
                    });
                    position += 2;
                } else {
                    position += 1;
                }
            }
            continue;
        }
        let (key, value) = if let Some((key, value)) = line.split_once('=') {
            (key.trim(), value.trim())
        } else if let Some((key, value)) = line.split_once(':') {
            (key.trim(), value.trim())
        } else {
            continue;
        };
        if key.is_empty() || value.is_empty() {
            continue;
        }
        if secretish_key(key) {
            findings.push(ExposureFinding {
                kind,
                location: location.to_owned(),
                key: key.to_owned(),
                line: Some(index + 1),
                reference_only: reference_only_value(value),
            });
        }
    }
    findings
}

/// Which detection runs for a well-known auth surface, decided by the file's
/// name — the same decision the CLI makes when it prints vaulting hints.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AuthFileShape {
    Json,
    Netrc,
    KeyValue,
}

pub fn auth_file_shape(file_name: &str) -> AuthFileShape {
    if file_name.ends_with(".json") {
        AuthFileShape::Json
    } else if file_name.contains("netrc") {
        AuthFileShape::Netrc
    } else {
        AuthFileShape::KeyValue
    }
}

/// The well-known surfaces the standard scan covers: shell rc files, and
/// known auth files of tools that persist credential material in the clear.
/// Each entry is `(location, shape)`; absence is not a refusal — a file that
/// does not exist is simply not an exposure.
pub fn standard_targets(home: &std::path::Path) -> Vec<(std::path::PathBuf, AuthFileShape)> {
    let rc_names = [
        ".bashrc",
        ".bash_profile",
        ".profile",
        ".zshrc",
        ".zprofile",
    ];
    let mut targets: Vec<(std::path::PathBuf, AuthFileShape)> = rc_names
        .iter()
        .map(|name| (home.join(name), AuthFileShape::KeyValue))
        .collect();
    for name in [".netrc"] {
        targets.push((home.join(name), AuthFileShape::Netrc));
    }
    let auth_files = [
        ".pi/agent/auth.json",
        ".openai/auth.json",
        ".claude/.credentials.json",
        ".aws/credentials",
        ".config/gh/hosts.yml",
    ];
    for name in auth_files {
        let path = home.join(name);
        targets.push((path, auth_file_shape(name)));
    }
    targets
}

/// Run the standard scan: shell rc files, known auth files, and the given
/// environment pairs. Unreadable-but-present targets are declared refusals.
pub fn run_standard_scan<'a, I: IntoIterator<Item = (&'a str, &'a str)>>(
    home: &std::path::Path,
    env_pairs: I,
) -> ScanReport {
    let mut report = ScanReport {
        findings: scan_environment(env_pairs),
        ..ScanReport::default()
    };
    for (path, shape) in standard_targets(home) {
        let location = path.display().to_string();
        let text = match std::fs::read_to_string(&path) {
            Ok(text) => text,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                continue;
            }
            Err(error) => {
                report.refusals.push(ScanRefusal {
                    target: location,
                    reason: format!("exists but could not be read ({error}); detection did not run"),
                });
                continue;
            }
        };
        report.scanned.push(location.clone());
        match shape {
            AuthFileShape::Json => match scan_json_text(&location, &text) {
                Ok(findings) => report.findings.extend(findings),
                Err(refusal) => report.refusals.push(refusal),
            },
            AuthFileShape::Netrc => report
                .findings
                .extend(scan_keyvalue_text(&location, &text, ExposureKind::Netrc)),
            AuthFileShape::KeyValue => report.findings.extend(scan_keyvalue_text(
                &location,
                &text,
                ExposureKind::KeyValueAuthFile,
            )),
        }
    }
    report
}

#[cfg(test)]
mod tests {
    use super::*;

    const RAW_DUMMY: &str = "RAW_DUMMY_FIXTURE_VALUE_DO_NOT_PRINT";

    #[test]
    fn seeded_rc_key_is_found_and_never_printed() {
        let dir = std::env::temp_dir().join(format!("wck-scan-rc-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join(".zshrc-seeded");
        std::fs::write(
            &path,
            format!(
                "# export COMMENTED_TOKEN=no\nexport WCK_FIXTURE_API_KEY={RAW_DUMMY}\nPATH=/usr/bin\n"
            ),
        )
        .unwrap();
        let text = std::fs::read_to_string(&path).unwrap();
        let findings = scan_shell_text(&path.display().to_string(), &text);

        assert_eq!(findings.len(), 1);
        assert_eq!(findings[0].key, "WCK_FIXTURE_API_KEY");
        assert_eq!(findings[0].line, Some(2));
        assert!(!findings[0].reference_only);

        let report = ScanReport {
            findings,
            ..ScanReport::default()
        };
        let rendered = format!("{}\n{:?}", report.render_plain(), report);
        assert!(!rendered.contains(RAW_DUMMY), "a report printed material");
        assert!(!rendered.contains("COMMENTED_TOKEN"));

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn vault_reference_value_is_marked_reference_only() {
        let findings = scan_shell_text(
            "/tmp/rc",
            "export GITHUB_TOKEN=\"$(op read op://v/i/f)\"\nexport REAL_TOKEN=abc123\n",
        );
        assert_eq!(findings.len(), 2);
        assert!(findings[0].reference_only);
        assert!(!findings[1].reference_only);
    }

    #[test]
    fn json_auth_file_findings_carry_dotted_paths_and_never_values() {
        let text = format!(
            r#"{{"type":"acme","auth":{{"api_key":"{RAW_DUMMY}"}},"model":"x","nested":[{{"client_secret":"inner"}}]}}"#
        );
        let findings = scan_json_text("/home/u/.pi/agent/auth.json", &text).unwrap();
        let keys: Vec<&str> = findings.iter().map(|f| f.key.as_str()).collect();
        assert_eq!(keys, vec!["auth.api_key", "nested[0].client_secret"]);
        let rendered = format!("{:?}", findings);
        assert!(!rendered.contains(RAW_DUMMY));
    }

    #[test]
    fn invalid_json_is_a_declared_refusal_not_silence() {
        let refusal = scan_json_text("/home/u/auth.json", "{not json").unwrap_err();
        assert_eq!(refusal.target, "/home/u/auth.json");
        assert!(refusal.reason.contains("not valid JSON"));
    }

    #[test]
    fn environment_scan_skips_empty_and_non_material_names() {
        let pairs = [
            ("WCK_FIXTURE_TOKEN", RAW_DUMMY),
            ("SSH_AUTH_SOCK", "/run/user/1000/keyring/ssh"),
            ("GPG_TTY", "/dev/pts/0"),
            ("EMPTY_TOKEN", ""),
            ("PATH", "/usr/bin"),
        ];
        let findings = scan_environment(pairs);
        assert_eq!(findings.len(), 1);
        assert_eq!(findings[0].key, "WCK_FIXTURE_TOKEN");
        assert_eq!(findings[0].location, "env:WCK_FIXTURE_TOKEN");
    }

    #[test]
    fn keyvalue_and_netrc_shapes_are_detected() {
        let ini = format!("aws_secret_access_key = {RAW_DUMMY}\nregion = eu-west-1\n");
        let ini_findings = scan_keyvalue_text("/home/u/.aws/credentials", &ini, ExposureKind::KeyValueAuthFile);
        assert_eq!(ini_findings.len(), 1);
        assert_eq!(ini_findings[0].key, "aws_secret_access_key");

        let netrc = format!("machine example.com login frank password {RAW_DUMMY}\n");
        let netrc_findings =
            scan_keyvalue_text("/home/u/.netrc", &netrc, ExposureKind::Netrc);
        assert_eq!(netrc_findings.len(), 1);
        assert_eq!(netrc_findings[0].key, "password");
    }

    #[test]
    fn unreadable_target_is_a_declared_refusal_in_the_standard_scan() {
        let dir = std::env::temp_dir().join(format!("wck-scan-refusal-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let locked = dir.join(".aws");
        std::fs::create_dir_all(&locked).unwrap();
        let file = locked.join("credentials");
        std::fs::write(&file, "x = y\n").unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&file, std::fs::Permissions::from_mode(0o000)).unwrap();
        }

        let report = run_standard_scan(&dir, std::iter::empty());
        assert_eq!(report.refusals.len(), 1, "expected one declared refusal");
        assert!(report.refusals[0].target.ends_with(".aws/credentials"));
        assert!(report.refusals[0].reason.contains("could not be read"));

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&file, std::fs::Permissions::from_mode(0o644)).unwrap();
        }
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn standard_scan_reports_json_auth_file_present_in_home() {
        let dir = std::env::temp_dir().join(format!("wck-scan-home-{}", std::process::id()));
        std::fs::create_dir_all(dir.join(".pi/agent")).unwrap();
        std::fs::write(
            dir.join(".pi/agent/auth.json"),
            format!(r#"{{"access_token":"{RAW_DUMMY}"}}"#),
        )
        .unwrap();

        let report = run_standard_scan(&dir, std::iter::empty::<(&str, &str)>());
        let pi_finding = report
            .findings
            .iter()
            .find(|f| f.location.ends_with(".pi/agent/auth.json"))
            .expect("the seeded auth file must be scanned");
        assert_eq!(pi_finding.key, "access_token");
        assert_eq!(pi_finding.kind, ExposureKind::JsonAuthFile);
        let rendered = report.render_plain();
        assert!(!rendered.contains(RAW_DUMMY));

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn secretish_covers_common_shapes_and_stays_quiet_on_ordinals() {
        for key in [
            "API_KEY",
            "api_key",
            "GITHUB_TOKEN",
            "oauth_token",
            "AWS_SECRET_ACCESS_KEY",
            "client_secret",
            "my_password",
            "refresh_token",
            "AUTH_TOKEN",
        ] {
            assert!(secretish_key(key), "{key} should be detected");
        }
        for key in ["PATH", "HOME", "EDITOR", "MODEL_NAME", "REGION"] {
            assert!(!secretish_key(key), "{key} should not be detected");
        }
    }
}
