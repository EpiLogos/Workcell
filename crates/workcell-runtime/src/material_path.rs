//! Material object identity only; never a Central source or Agent identity.
use epilogos_workcell_core::{Result, WorkcellError};
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
pub(crate) struct MaterialPath {
    pub supplied: PathBuf,
    pub canonical: PathBuf,
    pub identity: String,
    kind: MaterialKind,
}

impl MaterialPath {
    /// Writable/storage bindings remain directory-only.
    pub fn directory(path: &Path) -> Result<Self> {
        Self::resolve(path, Some(MaterialKind::Directory))
    }

    /// A supplied protection can name a regular source file, not just a directory.
    /// This grants no access and does not allow holes under a writable ancestor.
    pub fn protected_object(path: &Path) -> Result<Self> {
        Self::resolve(path, None)
    }

    fn resolve(path: &Path, expected_kind: Option<MaterialKind>) -> Result<Self> {
        if !path.is_absolute() || path.components().any(|c| matches!(c, Component::ParentDir)) {
            return Err(WorkcellError::InvalidDemand(
                "material path must be absolute and contain no parent traversal".into(),
            ));
        }
        let canonical = fs::canonicalize(path)
            .map_err(|e| WorkcellError::Unavailable(format!("resolve material object: {e}")))?;
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
        Ok(Self {
            supplied: path.to_path_buf(),
            canonical,
            identity: object_identity(&metadata)?,
            kind,
        })
    }

    pub fn validate(&self) -> Result<()> {
        if Self::resolve(&self.supplied, Some(self.kind))? != *self {
            return Err(WorkcellError::OperationFailed(
                "material object changed since resolution; re-resolve policy and placement".into(),
            ));
        }
        Ok(())
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
        assert!(MaterialPath::protected_object(&root.0.join("absent")).is_err());
    }
}
