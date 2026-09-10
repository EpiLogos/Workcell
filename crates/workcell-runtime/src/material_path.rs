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
