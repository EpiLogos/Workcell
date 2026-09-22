//! Material object identity only; never a Central source or Agent identity.
use epilogos_workcell_core::{Result, WorkcellError};
use serde_json::{json, Value};
use std::{
    fs,
    path::{Component, Path, PathBuf},
};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum MaterialKind {
    Directory,
    RegularFile,
}

#[derive(Clone, Debug, Eq, PartialEq)]
enum MaterialState {
    Existing {
        identity: String,
        kind: MaterialKind,
    },
    Missing {
        anchor_supplied: PathBuf,
        anchor_canonical: PathBuf,
        anchor_identity: String,
        suffix: PathBuf,
    },
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct MaterialPath {
    pub supplied: PathBuf,
    pub canonical: PathBuf,
    /// Existing material identity retained for directory-only internal callers.
    /// Missing protections have no object identity and leave this empty.
    pub identity: String,
    state: MaterialState,
}

impl MaterialPath {
    /// Writable/storage bindings remain directory-only.
    pub fn directory(path: &Path) -> Result<Self> {
        Self::resolve(path, Some(MaterialKind::Directory), false)
    }

    /// A supplied protection can name a regular source file, not just a directory.
    /// A missing protection retains its existing ancestor identity and unresolved
    /// suffix. This grants no access and does not allow holes under a writable
    /// ancestor.
    pub fn protected_object(path: &Path) -> Result<Self> {
        Self::resolve(path, None, true)
    }

    fn resolve(
        path: &Path,
        expected_kind: Option<MaterialKind>,
        allow_missing: bool,
    ) -> Result<Self> {
        if !path.is_absolute() || path.components().any(|c| matches!(c, Component::ParentDir)) {
            return Err(WorkcellError::InvalidDemand(
                "material path must be absolute and contain no parent traversal".into(),
            ));
        }
        let canonical = match fs::canonicalize(path) {
            Ok(canonical) => canonical,
            Err(error) if allow_missing && error.kind() == std::io::ErrorKind::NotFound => {
                return Self::resolve_missing(path)
            }
            Err(error) => {
                return Err(WorkcellError::Unavailable(format!(
                    "resolve material object: {error}"
                )))
            }
        };
        let metadata = fs::metadata(&canonical)
            .map_err(|e| WorkcellError::Unavailable(format!("inspect material object: {e}")))?;
        let kind = if metadata.is_dir() {
            MaterialKind::Directory
        } else if metadata.is_file() {
            MaterialKind::RegularFile
        } else {
            return Err(WorkcellError::InvalidDemand(
                "material protection requires an existing regular file or directory".into(),
            ));
        };
        if expected_kind.is_some_and(|expected| expected != kind) {
            return Err(WorkcellError::InvalidDemand(
                "material object type does not match its directory/file binding".into(),
            ));
        }
        let identity = object_identity(&metadata)?;
        Ok(Self {
            supplied: path.to_path_buf(),
            canonical,
            identity: identity.clone(),
            state: MaterialState::Existing { identity, kind },
        })
    }

    fn resolve_missing(path: &Path) -> Result<Self> {
        let mut cursor = PathBuf::new();
        let mut anchor_supplied = None;
        let mut suffix = PathBuf::new();
        let mut missing = false;
        for component in path.components() {
            cursor.push(component.as_os_str());
            if missing {
                suffix.push(component.as_os_str());
                continue;
            }
            match fs::symlink_metadata(&cursor) {
                Ok(metadata) => {
                    if metadata.file_type().is_symlink() {
                        return Err(WorkcellError::InvalidDemand(
                            "missing protected path cannot traverse a symbolic link".into(),
                        ));
                    }
                    anchor_supplied = Some(cursor.clone());
                }
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                    missing = true;
                    suffix.push(component.as_os_str());
                }
                Err(error) => {
                    return Err(WorkcellError::Unavailable(format!(
                        "inspect missing protected path: {error}"
                    )))
                }
            }
        }
        if !missing || suffix.as_os_str().is_empty() {
            return Err(WorkcellError::OperationFailed(
                "protected object appeared while resolving; resolve policy and placement again"
                    .into(),
            ));
        }
        let anchor_supplied = anchor_supplied.ok_or_else(|| {
            WorkcellError::InvalidDemand(
                "missing protected path has no existing directory ancestor".into(),
            )
        })?;
        let anchor_canonical = fs::canonicalize(&anchor_supplied).map_err(|error| {
            WorkcellError::Unavailable(format!("resolve protected path ancestor: {error}"))
        })?;
        let metadata = fs::metadata(&anchor_canonical).map_err(|error| {
            WorkcellError::Unavailable(format!("inspect protected path ancestor: {error}"))
        })?;
        if !metadata.is_dir() {
            return Err(WorkcellError::InvalidDemand(
                "missing protected path requires an existing directory ancestor".into(),
            ));
        }
        let canonical = anchor_canonical.join(&suffix);
        Ok(Self {
            supplied: path.to_path_buf(),
            canonical,
            identity: String::new(),
            state: MaterialState::Missing {
                anchor_supplied,
                anchor_canonical,
                anchor_identity: object_identity(&metadata)?,
                suffix,
            },
        })
    }

    pub fn validate(&self) -> Result<()> {
        let (expected_kind, allow_missing) = match &self.state {
            MaterialState::Existing { kind, .. } => (Some(*kind), false),
            MaterialState::Missing { .. } => (None, true),
        };
        if Self::resolve(&self.supplied, expected_kind, allow_missing)? != *self {
            return Err(WorkcellError::OperationFailed(
                "material object changed since resolution; re-resolve policy and placement".into(),
            ));
        }
        Ok(())
    }

    pub fn inspection(&self) -> Value {
        match &self.state {
            MaterialState::Existing { identity, .. } => {
                json!({"path": self.canonical, "identity": identity})
            }
            MaterialState::Missing {
                anchor_supplied,
                anchor_canonical,
                anchor_identity,
                suffix,
            } => json!({
                "path": self.canonical,
                "presence": "missing",
                "identity": Value::Null,
                "existing_ancestor": {
                    "supplied_path": anchor_supplied,
                    "path": anchor_canonical,
                    "identity": anchor_identity
                },
                "missing_suffix": suffix
            }),
        }
    }
}

#[cfg(unix)]
pub(crate) fn object_identity(metadata: &fs::Metadata) -> Result<String> {
    use std::os::unix::fs::MetadataExt;
    Ok(format!("{}:{}", metadata.dev(), metadata.ino()))
}
#[cfg(not(unix))]
pub(crate) fn object_identity(_metadata: &fs::Metadata) -> Result<String> {
    Err(WorkcellError::Unsupported(
        "stable directory identity is currently implemented on Unix only".into(),
    ))
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use std::time::{SystemTime, UNIX_EPOCH};

    struct Temporary(PathBuf);
    impl Temporary {
        fn new() -> Self {
            let path = std::env::temp_dir().join(format!(
                "workcell-protected-object-{}-{}",
                std::process::id(),
                SystemTime::now()
                    .duration_since(UNIX_EPOCH)
                    .unwrap()
                    .as_nanos()
            ));
            fs::create_dir(&path).unwrap();
            Self(path)
        }
    }
    impl Drop for Temporary {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn protected_regular_file_is_not_a_writable_directory() {
        let root = Temporary::new();
        let source = root.0.join("now.json");
        fs::write(&source, b"retained native source").unwrap();
        let pinned = MaterialPath::protected_object(&source).unwrap();
        pinned.validate().unwrap();
        assert!(MaterialPath::directory(&source).is_err());
        assert_eq!(fs::read(&source).unwrap(), b"retained native source");
    }

    #[test]
    fn same_path_replacement_and_type_drift_require_new_resolution() {
        let root = Temporary::new();
        let source = root.0.join("source-relations.json");
        fs::write(&source, b"original").unwrap();
        let pinned = MaterialPath::protected_object(&source).unwrap();
        // Preserve the previous inode so the replacement cannot reuse it.
        fs::rename(&source, root.0.join("retained-original")).unwrap();
        fs::write(&source, b"original").unwrap();
        assert!(pinned.validate().is_err());
        let replacement = MaterialPath::protected_object(&source).unwrap();
        fs::remove_file(&source).unwrap();
        fs::create_dir(&source).unwrap();
        assert!(replacement.validate().is_err());
    }

    #[test]
    fn redirected_protected_alias_is_not_the_pinned_object() {
        let root = Temporary::new();
        let first = root.0.join("first");
        let second = root.0.join("second");
        let alias = root.0.join("source");
        fs::write(&first, b"one").unwrap();
        fs::write(&second, b"two").unwrap();
        std::os::unix::fs::symlink(&first, &alias).unwrap();
        let pinned = MaterialPath::protected_object(&alias).unwrap();
        fs::remove_file(&alias).unwrap();
        std::os::unix::fs::symlink(&second, &alias).unwrap();
        assert!(pinned.validate().is_err());
        // Absent protection is represented and denied by the anchored cases
        // below; absence is no longer an unsupported-only refusal.
    }

    #[test]
    fn missing_protection_pins_ancestor_and_requires_fresh_resolution_on_appearance() {
        let root = Temporary::new();
        let root = fs::canonicalize(&root.0).unwrap();
        let protected = root.join("ProjectCentral/future-source.json");
        let pinned = MaterialPath::protected_object(&protected).unwrap();
        let inspected = pinned.inspection();
        assert_eq!(inspected["presence"], "missing");
        assert_eq!(inspected["identity"], Value::Null);
        assert_eq!(
            inspected["existing_ancestor"]["path"].as_str(),
            root.to_str()
        );
        assert_eq!(
            inspected["missing_suffix"],
            "ProjectCentral/future-source.json"
        );
        pinned.validate().unwrap();
        fs::create_dir(root.join("ProjectCentral")).unwrap();
        assert!(pinned.validate().is_err());
    }

    #[test]
    fn missing_protection_rejects_symbolic_link_ancestors() {
        let root = Temporary::new();
        let root = fs::canonicalize(&root.0).unwrap();
        fs::create_dir(root.join("actual")).unwrap();
        std::os::unix::fs::symlink(root.join("actual"), root.join("alias")).unwrap();
        let error = MaterialPath::protected_object(&root.join("alias/absent"))
            .unwrap_err()
            .to_string();
        assert!(error.contains("symbolic link"), "{error}");
    }

    #[test]
    fn missing_protection_pins_existing_ancestor_identity() {
        let root = Temporary::new();
        let root = fs::canonicalize(&root.0).unwrap();
        let project = root.join("ProjectCentral");
        let protected = project.join("future-source.json");
        fs::create_dir(&project).unwrap();
        let pinned = MaterialPath::protected_object(&protected).unwrap();
        pinned.validate().unwrap();
        fs::rename(&project, root.join("retained-ProjectCentral")).unwrap();
        fs::create_dir(&project).unwrap();
        assert!(pinned.validate().is_err());
    }
}
