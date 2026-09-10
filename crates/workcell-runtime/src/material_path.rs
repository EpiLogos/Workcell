//! Material object identity only; never a Central source or Agent identity.
use epilogos_workcell_core::{Result, WorkcellError};
use std::{
    fs,
    path::{Component, Path, PathBuf},
};

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct MaterialPath {
    pub supplied: PathBuf,
    pub canonical: PathBuf,
    pub identity: String,
}

impl MaterialPath {
    pub fn directory(path: &Path) -> Result<Self> {
        if !path.is_absolute() || path.components().any(|c| matches!(c, Component::ParentDir)) {
            return Err(WorkcellError::InvalidDemand(
                "material directory must be absolute and contain no parent traversal".into(),
            ));
        }
        let canonical = fs::canonicalize(path)
            .map_err(|e| WorkcellError::Unavailable(format!("resolve material directory: {e}")))?;
        let metadata = fs::metadata(&canonical)
            .map_err(|e| WorkcellError::Unavailable(format!("inspect material directory: {e}")))?;
        if !metadata.is_dir() {
            return Err(WorkcellError::InvalidDemand(
                "material binding requires an existing directory".into(),
            ));
        }
        Ok(Self {
            supplied: path.to_path_buf(),
            canonical,
            identity: object_identity(&metadata)?,
        })
    }

    pub fn validate(&self) -> Result<()> {
        if Self::directory(&self.supplied)? != *self {
            return Err(WorkcellError::OperationFailed(
                "material directory changed since resolution; re-resolve policy and placement"
                    .into(),
            ));
        }
        Ok(())
    }
}

/// A denied path is not a writable directory binding. It may be a regular file
/// or not yet exist. Retain the exact denied path and pin its nearest existing
/// object; never replace a denied file with its parent or silently omit it.
/// The boundary still refuses any writable ancestor of this canonical path.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct ProtectedPath {
    pub supplied: PathBuf,
    pub canonical: PathBuf,
    pub existing_ancestor: PathBuf,
    pub identity: String,
    pub exists: bool,
}

impl ProtectedPath {
    pub fn resolve(path: &Path) -> Result<Self> {
        if !path.is_absolute() || path.components().any(|c| matches!(c, Component::ParentDir)) {
            return Err(WorkcellError::InvalidDemand(
                "protected path must be absolute and contain no parent traversal".into(),
            ));
        }
        let mut existing = path;
        let mut tail = Vec::new();
        loop {
            match fs::symlink_metadata(existing) {
                Ok(_) => break,
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                    let name = existing.file_name().ok_or_else(|| {
                        WorkcellError::InvalidDemand("protected path has no existing anchor".into())
                    })?;
                    tail.push(name.to_os_string());
                    existing = existing.parent().ok_or_else(|| {
                        WorkcellError::InvalidDemand("protected path has no parent".into())
                    })?;
                }
                Err(error) => {
                    return Err(WorkcellError::Unavailable(format!(
                        "inspect protected path: {error}"
                    )));
                }
            }
        }
        let existing_ancestor = fs::canonicalize(existing).map_err(|error| {
            WorkcellError::Unavailable(format!("resolve protected path: {error}"))
        })?;
        let metadata = fs::metadata(&existing_ancestor).map_err(|error| {
            WorkcellError::Unavailable(format!("inspect protected object: {error}"))
        })?;
        if !(metadata.is_dir() || metadata.is_file()) || (!tail.is_empty() && !metadata.is_dir()) {
            return Err(WorkcellError::InvalidDemand(
                "protected object must be a regular file, directory or absent directory member"
                    .into(),
            ));
        }
        let exists = tail.is_empty();
        let mut canonical = existing_ancestor.clone();
        for component in tail.into_iter().rev() {
            canonical.push(component);
        }
        Ok(Self {
            supplied: path.to_path_buf(),
            canonical,
            existing_ancestor,
            identity: object_identity(&metadata)?,
            exists,
        })
    }

    pub fn validate(&self) -> Result<()> {
        if Self::resolve(&self.supplied)? != *self {
            return Err(WorkcellError::OperationFailed(
                "protected object or absence changed; re-resolve policy and placement".into(),
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

    #[test]
    fn protected_files_and_absence_keep_exact_identity() {
        let root = std::env::temp_dir().join(format!(
            "workcell-protected-path-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir(&root).unwrap();
        let file = root.join("human.json");
        fs::write(&file, "human source").unwrap();
        let protected = ProtectedPath::resolve(&file).unwrap();
        assert!(protected.exists);
        assert_eq!(protected.canonical, fs::canonicalize(&file).unwrap());
        protected.validate().unwrap();
        fs::rename(&file, root.join("old-human.json")).unwrap();
        fs::write(&file, "replacement").unwrap();
        assert!(protected.validate().is_err());
        let absent = ProtectedPath::resolve(&root.join("future/source.json")).unwrap();
        assert!(!absent.exists);
        assert_eq!(absent.existing_ancestor, fs::canonicalize(&root).unwrap());
        absent.validate().unwrap();
        fs::create_dir(root.join("future")).unwrap();
        assert!(absent.validate().is_err());
        assert!(ProtectedPath::resolve(Path::new("relative")).is_err());
        assert!(ProtectedPath::resolve(&root.join("../escape")).is_err());
        fs::remove_dir_all(root).unwrap();
    }
}
