//! The state root's own Workcell identity.
//!
//! A machine names its cell once, in its state root
//! (`<state-root>/identity.json`, `workcell.identity/v1`), and every local
//! operation that is not handed an explicit `--workcell-ref` answers as that
//! cell. Without a declaration the cell is `workcell:local`, the single-machine
//! default. The registries this state root holds (connection grants, harness
//! instances) are each bound to one `workcell_ref`; a caller that resolves a
//! different identity is refused by them, so the declaration is what lets a
//! renamed machine's own callers — which pass no flag — read its own stores.
//!
//! Same law as the registries: a missing declaration is the default identity;
//! an unreadable or invalid declaration is named unavailability, never a
//! silent fallback to `workcell:local`.

use std::{
    fs,
    path::{Path, PathBuf},
};

use epilogos_workcell_core::{Result, WorkcellError, WorkcellRef};
use serde_json::{json, Value};

pub const IDENTITY_FILE: &str = "identity.json";
pub const IDENTITY_SCHEMA: &str = "workcell.identity/v1";
/// The identity of a state root that declares none.
pub const DEFAULT_WORKCELL_REF: &str = "workcell:local";

/// Where a resolved identity came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IdentitySource {
    /// `--workcell-ref` on this invocation.
    Flag,
    /// The state root's `identity.json`.
    Declared,
    /// No flag and no declaration: `workcell:local`.
    Default,
}

impl IdentitySource {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Flag => "flag",
            Self::Declared => "declared",
            Self::Default => "default",
        }
    }
}

pub fn identity_path(state_root: impl AsRef<Path>) -> PathBuf {
    state_root.as_ref().join(IDENTITY_FILE)
}

/// The identity this state root declares, if it declares one.
pub fn read_declared(state_root: impl AsRef<Path>) -> Result<Option<WorkcellRef>> {
    let path = identity_path(state_root);
    let raw = match fs::read_to_string(&path) {
        Ok(raw) => raw,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => {
            return Err(WorkcellError::OperationFailed(format!(
                "read Workcell identity {}: {error}",
                path.display()
            )))
        }
    };
    let parsed: Value = serde_json::from_str(&raw).map_err(|error| {
        WorkcellError::OperationFailed(format!(
            "parse Workcell identity {}: {error}",
            path.display()
        ))
    })?;
    if parsed.get("schema").and_then(Value::as_str) != Some(IDENTITY_SCHEMA) {
        return Err(WorkcellError::OperationFailed(format!(
            "Workcell identity {} does not carry schema {IDENTITY_SCHEMA}",
            path.display()
        )));
    }
    let reference = parsed
        .get("workcell_ref")
        .and_then(Value::as_str)
        .ok_or_else(|| {
            WorkcellError::OperationFailed(format!(
                "Workcell identity {} carries no string `workcell_ref`",
                path.display()
            ))
        })?;
    WorkcellRef::new(reference).map(Some).map_err(|message| {
        WorkcellError::OperationFailed(format!(
            "Workcell identity {} has an invalid `workcell_ref`: {message}",
            path.display()
        ))
    })
}

/// The identity a local operation answers as: the explicit flag, else the
/// state root's declaration, else `workcell:local`.
pub fn resolve(
    state_root: impl AsRef<Path>,
    flag: Option<&str>,
) -> Result<(WorkcellRef, IdentitySource)> {
    if let Some(flag) = flag {
        return WorkcellRef::new(flag)
            .map(|reference| (reference, IdentitySource::Flag))
            .map_err(WorkcellError::from);
    }
    match read_declared(state_root)? {
        Some(reference) => Ok((reference, IdentitySource::Declared)),
        None => Ok((
            WorkcellRef::new(DEFAULT_WORKCELL_REF).expect("default Workcell ref is non-empty"),
            IdentitySource::Default,
        )),
    }
}

/// Write the declaration atomically. `provenance` is carried verbatim (what
/// was rebound on the way, and when).
pub fn declare(
    state_root: impl AsRef<Path>,
    workcell_ref: &WorkcellRef,
    provenance: Value,
) -> Result<Value> {
    let state_root = state_root.as_ref();
    fs::create_dir_all(state_root).map_err(|error| {
        WorkcellError::OperationFailed(format!(
            "create Workcell state root {}: {error}",
            state_root.display()
        ))
    })?;
    let path = identity_path(state_root);
    let document = json!({
        "schema": IDENTITY_SCHEMA,
        "workcell_ref": workcell_ref.as_str(),
        "provenance": provenance,
    });
    let temp = path.with_extension(format!("json.pending-{}", std::process::id()));
    fs::write(
        &temp,
        serde_json::to_vec_pretty(&document).map_err(|error| {
            WorkcellError::OperationFailed(format!("encode Workcell identity: {error}"))
        })?,
    )
    .map_err(|error| {
        WorkcellError::OperationFailed(format!(
            "write Workcell identity {}: {error}",
            temp.display()
        ))
    })?;
    fs::rename(&temp, &path).map_err(|error| {
        WorkcellError::OperationFailed(format!(
            "publish Workcell identity {}: {error}",
            path.display()
        ))
    })?;
    Ok(document)
}

/// The `workcell_ref` a state-root file is bound to, read without the
/// owning store's refusal: `None` when the file is absent.
pub fn bound_identity(path: impl AsRef<Path>) -> Result<Option<String>> {
    let path = path.as_ref();
    let raw = match fs::read_to_string(path) {
        Ok(raw) => raw,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => {
            return Err(WorkcellError::OperationFailed(format!(
                "read {}: {error}",
                path.display()
            )))
        }
    };
    let parsed: Value = serde_json::from_str(&raw).map_err(|error| {
        WorkcellError::OperationFailed(format!("parse {}: {error}", path.display()))
    })?;
    parsed
        .get("workcell_ref")
        .and_then(Value::as_str)
        .map(|reference| Some(reference.to_owned()))
        .ok_or_else(|| {
            WorkcellError::OperationFailed(format!(
                "{} carries no string `workcell_ref`",
                path.display()
            ))
        })
}

/// Keep the exact prior bytes of a store before it is rebound. The name says
/// which identity the bytes were bound to; an existing backup is never
/// overwritten.
pub fn backup_before_rebind(path: &Path, from: &WorkcellRef, at_unix_ms: u64) -> Result<PathBuf> {
    let safe: String = from
        .as_str()
        .chars()
        .map(|character| {
            if character.is_ascii_alphanumeric() || matches!(character, '-' | '_' | '.') {
                character
            } else {
                '-'
            }
        })
        .collect();
    let file_name = path
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_default();
    let backup = path.with_file_name(format!("{file_name}.rebind-{safe}-{at_unix_ms}.bak"));
    let bytes = fs::read(path).map_err(|error| {
        WorkcellError::OperationFailed(format!("read {} for backup: {error}", path.display()))
    })?;
    let mut file = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&backup)
        .map_err(|error| {
            WorkcellError::OperationFailed(format!(
                "create rebind backup {}: {error}",
                backup.display()
            ))
        })?;
    std::io::Write::write_all(&mut file, &bytes).map_err(|error| {
        WorkcellError::OperationFailed(format!("write rebind backup {}: {error}", backup.display()))
    })?;
    file.sync_all().map_err(|error| {
        WorkcellError::OperationFailed(format!("sync rebind backup {}: {error}", backup.display()))
    })?;
    Ok(backup)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_root(label: &str) -> PathBuf {
        let nonce = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let root = std::env::temp_dir().join(format!(
            "workcell-identity-{label}-{}-{nonce}",
            std::process::id()
        ));
        fs::create_dir_all(&root).unwrap();
        root
    }

    #[test]
    fn an_undeclared_root_is_local_a_declared_root_is_its_declaration_and_a_flag_wins() {
        let root = temp_root("resolve");
        let (reference, source) = resolve(&root, None).unwrap();
        assert_eq!(
            (reference.as_str(), source),
            ("workcell:local", IdentitySource::Default)
        );
        declare(&root, &WorkcellRef::new("workcell:mac").unwrap(), json!({})).unwrap();
        let (reference, source) = resolve(&root, None).unwrap();
        assert_eq!(
            (reference.as_str(), source),
            ("workcell:mac", IdentitySource::Declared)
        );
        let (reference, source) = resolve(&root, Some("workcell:other")).unwrap();
        assert_eq!(
            (reference.as_str(), source),
            ("workcell:other", IdentitySource::Flag)
        );
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn an_unreadable_declaration_is_named_never_a_fallback_to_local() {
        let root = temp_root("invalid");
        fs::write(identity_path(&root), "{not json").unwrap();
        let error = resolve(&root, None).unwrap_err().to_string();
        assert!(error.contains("parse Workcell identity"), "{error}");
        fs::write(
            identity_path(&root),
            r#"{"schema":"workcell.identity/v0","workcell_ref":"workcell:mac"}"#,
        )
        .unwrap();
        assert!(resolve(&root, None).is_err());
        fs::write(
            identity_path(&root),
            r#"{"schema":"workcell.identity/v1","workcell_ref":" "}"#,
        )
        .unwrap();
        assert!(resolve(&root, None).is_err());
        // An explicit flag never needs the declaration.
        assert!(resolve(&root, Some("workcell:flagged")).is_ok());
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn a_rebind_backup_keeps_the_exact_prior_bytes_and_never_overwrites() {
        let root = temp_root("backup");
        let store = root.join("grants.json");
        fs::write(&store, b"exact prior bytes").unwrap();
        let from = WorkcellRef::new("workcell:local").unwrap();
        let backup = backup_before_rebind(&store, &from, 7).unwrap();
        assert_eq!(
            backup.file_name().unwrap().to_string_lossy(),
            "grants.json.rebind-workcell-local-7.bak"
        );
        assert_eq!(fs::read(&backup).unwrap(), b"exact prior bytes");
        assert!(backup_before_rebind(&store, &from, 7).is_err());
        fs::remove_dir_all(root).unwrap();
    }
}
