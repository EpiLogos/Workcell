use std::{
    fs,
    io::{self, Read},
    path::{Path, PathBuf},
};

use epilogos_workcell_core::{Result, WorkcellError};

pub(crate) fn stable_key(parts: &[&str]) -> String {
    let mut hash = 0xcbf29ce484222325_u64;
    for part in parts {
        for byte in part.as_bytes() {
            hash ^= u64::from(*byte);
            hash = hash.wrapping_mul(0x100000001b3);
        }
        hash ^= 0xff;
        hash = hash.wrapping_mul(0x100000001b3);
    }
    format!("{hash:016x}")
}

pub(crate) fn require_directory(path: &Path) -> Result<()> {
    if !path.exists() {
        return Err(WorkcellError::Unavailable(format!(
            "workspace material source `{}` does not exist",
            path.display()
        )));
    }
    if !path.is_dir() {
        return Err(WorkcellError::InvalidDemand(format!(
            "workspace material source `{}` is not a directory",
            path.display()
        )));
    }
    Ok(())
}

pub(crate) fn copy_tree(source: &Path, target: &Path) -> Result<()> {
    fs::create_dir_all(target).map_err(io_error("create workspace target"))?;
    let mut entries = fs::read_dir(source)
        .map_err(io_error("read workspace source"))?
        .collect::<io::Result<Vec<_>>>()
        .map_err(io_error("read workspace source entry"))?;
    entries.sort_by_key(|entry| entry.file_name());

    for entry in entries {
        let source_path = entry.path();
        let target_path = target.join(entry.file_name());
        let file_type = entry
            .file_type()
            .map_err(io_error("inspect workspace source entry"))?;
        if file_type.is_symlink() {
            return Err(WorkcellError::Unsupported(format!(
                "directory workspace source contains unsupported symlink `{}`",
                source_path.display()
            )));
        }
        if file_type.is_dir() {
            copy_tree(&source_path, &target_path)?;
        } else if file_type.is_file() {
            fs::copy(&source_path, &target_path).map_err(io_error("copy workspace file"))?;
        }
    }
    Ok(())
}

pub(crate) fn fingerprint_tree(root: &Path) -> Result<u64> {
    require_directory(root)?;
    let mut hash = 0xcbf29ce484222325_u64;
    fingerprint_dir(root, root, &mut hash)?;
    Ok(hash)
}

fn fingerprint_dir(root: &Path, current: &Path, hash: &mut u64) -> Result<()> {
    let mut entries = fs::read_dir(current)
        .map_err(io_error("read workspace for fingerprint"))?
        .collect::<io::Result<Vec<_>>>()
        .map_err(io_error("read workspace fingerprint entry"))?;
    entries.sort_by_key(|entry| entry.file_name());

    for entry in entries {
        let path = entry.path();
        let relative = path.strip_prefix(root).map_err(|error| {
            WorkcellError::OperationFailed(format!("workspace path escaped root: {error}"))
        })?;
        hash_bytes(hash, relative.to_string_lossy().as_bytes());
        let file_type = entry
            .file_type()
            .map_err(io_error("inspect workspace fingerprint entry"))?;
        if file_type.is_symlink() {
            hash_bytes(hash, b"symlink");
        } else if file_type.is_dir() {
            hash_bytes(hash, b"directory");
            fingerprint_dir(root, &path, hash)?;
        } else if file_type.is_file() {
            hash_bytes(hash, b"file");
            let mut file = fs::File::open(&path).map_err(io_error("open workspace file"))?;
            let mut buffer = [0_u8; 8192];
            loop {
                let read = file
                    .read(&mut buffer)
                    .map_err(io_error("read workspace file"))?;
                if read == 0 {
                    break;
                }
                hash_bytes(hash, &buffer[..read]);
            }
        }
    }
    Ok(())
}

pub(crate) fn set_tree_readonly(root: &Path) -> Result<()> {
    let mut paths = walk(root)?;
    paths.push(root.to_path_buf());
    for path in paths {
        let mut permissions = fs::metadata(&path)
            .map_err(io_error("inspect workspace permissions"))?
            .permissions();
        permissions.set_readonly(true);
        fs::set_permissions(&path, permissions)
            .map_err(io_error("set workspace read-only"))?;
    }
    Ok(())
}

pub(crate) fn make_directories_writable(root: &Path) -> Result<()> {
    if !root.exists() {
        return Ok(());
    }
    let mut paths = walk(root)?;
    paths.push(root.to_path_buf());
    for path in paths {
        if !path.is_dir() {
            continue;
        }
        let mut permissions = fs::metadata(&path)
            .map_err(io_error("inspect workspace directory permissions"))?
            .permissions();
        make_owner_writable(&mut permissions);
        fs::set_permissions(&path, permissions)
            .map_err(io_error("make workspace directory writable for cleanup"))?;
    }
    Ok(())
}

#[cfg(unix)]
fn make_owner_writable(permissions: &mut fs::Permissions) {
    use std::os::unix::fs::PermissionsExt;

    permissions.set_mode(permissions.mode() | 0o700);
}

#[cfg(not(unix))]
fn make_owner_writable(permissions: &mut fs::Permissions) {
    permissions.set_readonly(false);
}

fn walk(root: &Path) -> Result<Vec<PathBuf>> {
    let mut output = Vec::new();
    let mut stack = vec![root.to_path_buf()];
    while let Some(current) = stack.pop() {
        let entries = fs::read_dir(&current)
            .map_err(io_error("walk workspace"))?
            .collect::<io::Result<Vec<_>>>()
            .map_err(io_error("walk workspace entry"))?;
        for entry in entries {
            let path = entry.path();
            if path.is_dir() {
                stack.push(path.clone());
            }
            output.push(path);
        }
    }
    Ok(output)
}

fn hash_bytes(hash: &mut u64, bytes: &[u8]) {
    for byte in bytes {
        *hash ^= u64::from(*byte);
        *hash = hash.wrapping_mul(0x100000001b3);
    }
}

fn io_error(context: &'static str) -> impl Fn(io::Error) -> WorkcellError {
    move |error| WorkcellError::OperationFailed(format!("{context}: {error}"))
}
