//! Physical publication for the native Workcell instance registry. Semantic
//! validation and revisions remain the caller's responsibility. The permanent
//! sibling lock is `.<filename>.publication.lock` (flock, five-second deadline).
//! Source bytes are read and compared while that lock is held. Staging is unique,
//! metadata-preserving and durable; initial creation never clobbers a winner.
//! Legacy registry staging is neither consumed nor removed.
use std::ffi::{CString, OsStr, OsString};
use std::fs::{self, File, Metadata};
use std::io::{self, Read, Seek, SeekFrom, Write};
use std::os::unix::fs::{MetadataExt, PermissionsExt};
use std::os::unix::io::{AsRawFd, FromRawFd};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

const LOCK_TIMEOUT: Duration = Duration::from_secs(5);
// Refuse oversized state without altering retained data or allocating unbounded memory.
const MAX_SOURCE_BYTES: u64 = 64 * 1024 * 1024;
static NEXT_STAGE: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

fn denied(message: &str) -> io::Error {
    io::Error::new(io::ErrorKind::PermissionDenied, message)
}
fn conflict(message: &str) -> io::Error {
    io::Error::new(io::ErrorKind::AlreadyExists, message)
}
fn native_name(name: &OsStr) -> io::Result<CString> {
    CString::new(name.as_encoded_bytes()).map_err(io::Error::other)
}
fn open_in(parent: &File, name: &OsStr, flags: libc::c_int) -> io::Result<File> {
    let name = native_name(name)?;
    // The parent descriptor and O_NOFOLLOW retain the physical identity even
    // if an external process redirects a pathname while the owner is working.
    let fd = unsafe {
        libc::openat(
            parent.as_raw_fd(),
            name.as_ptr(),
            flags | libc::O_NOFOLLOW | libc::O_CLOEXEC,
            0o600,
        )
    };
    if fd < 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(unsafe { File::from_raw_fd(fd) })
}
fn ordinary(metadata: &Metadata) -> io::Result<()> {
    if !metadata.is_file() || metadata.nlink() != 1 {
        return Err(denied(
            "Instance registry publication requires an ordinary source with one physical link",
        ));
    }
    Ok(())
}
fn same_file(left: &Metadata, right: &Metadata) -> bool {
    left.dev() == right.dev() && left.ino() == right.ino()
}

struct Snapshot {
    file: File,
    metadata: Metadata,
    bytes: Vec<u8>,
}
impl Snapshot {
    fn read(parent: &File, name: &OsStr) -> io::Result<Option<Self>> {
        let mut file = match open_in(parent, name, libc::O_RDONLY | libc::O_NONBLOCK) {
            Ok(file) => file,
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
            Err(error) => return Err(error),
        };
        let metadata = file.metadata()?;
        ordinary(&metadata)?;
        let mut bytes = Vec::new();
        (&mut file)
            .take(MAX_SOURCE_BYTES + 1)
            .read_to_end(&mut bytes)?;
        if bytes.len() as u64 > MAX_SOURCE_BYTES {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "Instance registry publication source exceeds 64 MiB",
            ));
        }
        Ok(Some(Self {
            file,
            metadata,
            bytes,
        }))
    }
    fn matches(&self, current: &Self) -> bool {
        same_file(&self.metadata, &current.metadata)
            && self.bytes == current.bytes
            && self.metadata.mode() == current.metadata.mode()
            && self.metadata.uid() == current.metadata.uid()
            && self.metadata.gid() == current.metadata.gid()
            && self.metadata.ctime() == current.metadata.ctime()
            && self.metadata.ctime_nsec() == current.metadata.ctime_nsec()
    }
}

pub(crate) struct Publication {
    parent_path: PathBuf,
    parent: File,
    name: OsString,
    lock_name: OsString,
    lock: File,
    current: Option<Snapshot>,
}
impl Drop for Publication {
    fn drop(&mut self) {
        unsafe {
            libc::flock(self.lock.as_raw_fd(), libc::LOCK_UN);
        }
    }
}
impl Publication {
    pub(crate) fn read(path: &Path) -> io::Result<Option<Vec<u8>>> {
        let parent_path = match fs::canonicalize(path.parent().ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::InvalidInput,
                "Instance registry has no parent",
            )
        })?) {
            Ok(parent) => parent,
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
            Err(error) => return Err(error),
        };
        let parent = File::open(parent_path)?;
        let name = path.file_name().ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::InvalidInput,
                "Instance registry has no filename",
            )
        })?;
        Snapshot::read(&parent, name).map(|source| source.map(|source| source.bytes))
    }

    pub(crate) fn acquire(path: &Path) -> io::Result<Self> {
        let name = path
            .file_name()
            .ok_or_else(|| {
                io::Error::new(
                    io::ErrorKind::InvalidInput,
                    "Instance registry publication has no filename",
                )
            })?
            .to_os_string();
        let parent_path = fs::canonicalize(path.parent().ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::InvalidInput,
                "Instance registry publication has no parent",
            )
        })?)?;
        use std::os::unix::fs::OpenOptionsExt;
        let parent = fs::OpenOptions::new()
            .read(true)
            .custom_flags(libc::O_DIRECTORY | libc::O_NOFOLLOW)
            .open(&parent_path)?;
        let mut lock_name = OsString::from(".");
        lock_name.push(&name);
        lock_name.push(".publication.lock");
        let lock = open_in(
            &parent,
            &lock_name,
            libc::O_RDWR | libc::O_CREAT | libc::O_NONBLOCK,
        )?;
        ordinary(&lock.metadata()?)?;
        let started = Instant::now();
        loop {
            if unsafe { libc::flock(lock.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } == 0 {
                break;
            }
            let error = io::Error::last_os_error();
            if error.kind() != io::ErrorKind::WouldBlock
                && error.kind() != io::ErrorKind::Interrupted
            {
                return Err(error);
            }
            let remaining = LOCK_TIMEOUT.saturating_sub(started.elapsed());
            if remaining.is_zero() {
                return Err(io::Error::new(
                    io::ErrorKind::TimedOut,
                    "Instance registry publication lock was not acquired within five seconds; source unchanged",
                ));
            }
            std::thread::sleep(remaining.min(Duration::from_millis(20)));
        }
        let current = Snapshot::read(&parent, &name)?;
        let publication = Self {
            parent_path,
            parent,
            name,
            lock_name,
            lock,
            current,
        };
        publication.check_basis()?;
        Ok(publication)
    }
    pub(crate) fn bytes(&self) -> Option<&[u8]> {
        self.current
            .as_ref()
            .map(|current| current.bytes.as_slice())
    }
    fn check_basis(&self) -> io::Result<()> {
        if !same_file(&self.parent.metadata()?, &fs::metadata(&self.parent_path)?) {
            return Err(conflict(
                "Instance registry publication parent changed; source unchanged",
            ));
        }
        let lock = open_in(
            &self.parent,
            &self.lock_name,
            libc::O_RDONLY | libc::O_NONBLOCK,
        )?;
        ordinary(&lock.metadata()?)?;
        if !same_file(&lock.metadata()?, &self.lock.metadata()?) {
            return Err(conflict(
                "Instance registry publication lock identity changed; source unchanged",
            ));
        }
        match (&self.current, Snapshot::read(&self.parent, &self.name)?) {
            (None, None) => Ok(()),
            (Some(basis), Some(current)) if basis.matches(&current) => Ok(()),
            _ => Err(conflict(
                "Instance registry publication source changed outside the owner lock; source not replaced",
            )),
        }
    }
    pub(crate) fn replace(&self, bytes: &[u8]) -> io::Result<bool> {
        if self.bytes() == Some(bytes) {
            self.check_basis()?;
            return Ok(false);
        }
        if let Some(current) = &self.current {
            if current.metadata.mode() & 0o222 == 0 {
                return Err(denied(
                    "Instance registry publication refuses to replace a readonly source",
                ));
            }
            // Atomic replacement must not bypass the source ACL merely
            // because its parent directory permits a rename.
            let writable = open_in(&self.parent, &self.name, libc::O_WRONLY | libc::O_NONBLOCK)?;
            if !same_file(&current.metadata, &writable.metadata()?) {
                return Err(conflict(
                    "Instance registry source changed during write admission",
                ));
            }
        }
        self.publish(bytes, self.current.as_ref())?;
        Ok(true)
    }
    fn publish(&self, bytes: &[u8], metadata_source: Option<&Snapshot>) -> io::Result<()> {
        if bytes.len() as u64 > MAX_SOURCE_BYTES {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "Instance registry publication exceeds 64 MiB",
            ));
        }
        let stage_name = OsString::from(format!(
            ".workcell-publication-{}-{}",
            std::process::id(),
            format!(
                "{}-{}",
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map_err(io::Error::other)?
                    .as_nanos(),
                NEXT_STAGE.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
            )
        ));
        let mut stage = open_in(
            &self.parent,
            &stage_name,
            libc::O_RDWR | libc::O_CREAT | libc::O_EXCL,
        )?;
        #[cfg(test)]
        tests::stage_created(&self.parent_path.join(&stage_name));
        let mut committed = false;
        let result = (|| {
            if let Some(source) = metadata_source {
                preserve_metadata(source, &stage)?;
            }
            stage.set_len(0)?;
            stage.seek(SeekFrom::Start(0))?;
            stage.write_all(bytes)?;
            stage.sync_all()?;
            if let Some(source) = metadata_source {
                verify_retained_metadata(source, &stage)?;
            }
            self.check_basis()?;
            // Publish only the create-new inode held by this operation.
            // A substituted named stage must not become native source.
            let named_stage =
                open_in(&self.parent, &stage_name, libc::O_RDONLY | libc::O_NONBLOCK)?;
            ordinary(&named_stage.metadata()?)?;
            if !same_file(&stage.metadata()?, &named_stage.metadata()?) {
                return Err(conflict(
                    "Publication staging identity changed; source unchanged",
                ));
            }
            let stage_c = native_name(&stage_name)?;
            let target_c = native_name(&self.name)?;
            if self.current.is_some() {
                if unsafe {
                    libc::renameat(
                        self.parent.as_raw_fd(),
                        stage_c.as_ptr(),
                        self.parent.as_raw_fd(),
                        target_c.as_ptr(),
                    )
                } != 0
                {
                    return Err(io::Error::last_os_error());
                }
            } else {
                rename_new(&self.parent, &stage_c, &target_c)?;
            }
            committed = true;
            self.parent.sync_all()?;
            if !same_file(&self.parent.metadata()?, &fs::metadata(&self.parent_path)?) {
                return Err(io::Error::other(
                    "Published Instance registry parent identity changed",
                ));
            }
            let reading = Snapshot::read(&self.parent, &self.name)?
                .ok_or_else(|| io::Error::other("Published Instance registry disappeared"))?;
            if reading.bytes != bytes || !same_file(&reading.metadata, &stage.metadata()?) {
                return Err(io::Error::other(
                    "Published Instance registry readback differs",
                ));
            }
            Ok(())
        })();
        // Failed staging is retained as bounded evidence. A separate inode
        // comparison followed by pathname unlink cannot atomically prove
        // ownership, and could delete a concurrent foreign replacement.
        // Successful descriptor-relative rename already removes our stage.
        if committed {
            result.map_err(|error: io::Error| io::Error::other(format!(
                "Instance registry publication may have committed, but durability/readback failed: {error}; inspect the original operation and source before retrying")))
        } else {
            result.map_err(|error: io::Error| {
                let identity = stage.metadata().map(|metadata| format!(
                    "{}:{}", metadata.dev(), metadata.ino()))
                    .unwrap_or_else(|_| "unavailable".to_string());
                io::Error::new(error.kind(), format!(
                    "{error}; uncommitted stage inode {identity} was not deleted; initial staging name {} (a substituted name is not owned)",
                    self.parent_path.join(&stage_name).display()))
            })
        }
    }
}

fn verify_ownership_mode(source: &Metadata, stage: &Metadata) -> io::Result<()> {
    if (source.uid(), source.gid(), source.mode() & 0o7777)
        != (stage.uid(), stage.gid(), stage.mode() & 0o7777)
    {
        return Err(denied(
            "Source ownership or permissions could not be retained; source unchanged",
        ));
    }
    Ok(())
}
fn preserve_metadata(source: &Snapshot, stage: &File) -> io::Result<()> {
    #[cfg(target_os = "macos")]
    {
        // Bound retained metadata before the native copy, as well as checking
        // parity after writing. Failed staging must remain bounded evidence.
        extended_attributes(&source.file)?;
        native_acl(&source.file)?;
        unsafe extern "C" {
            fn fcopyfile(
                from: libc::c_int,
                to: libc::c_int,
                state: *mut libc::c_void,
                flags: libc::c_uint,
            ) -> libc::c_int;
        }
        // Native COPYFILE_METADATA = ACL | STAT | XATTR. As in Central's
        // ordinary-file owner, a null state lets copyfile manage its state.
        // Copy before writing so the mutation receives a fresh mtime.
        if unsafe {
            fcopyfile(
                source.file.as_raw_fd(),
                stage.as_raw_fd(),
                std::ptr::null_mut(),
                7,
            )
        } != 0
        {
            return Err(io::Error::last_os_error());
        }
    }
    #[cfg(target_os = "linux")]
    {
        preserve_linux_metadata(source, stage)?;
    }
    #[cfg(not(any(target_os = "macos", target_os = "linux")))]
    {
        return Err(io::Error::new(io::ErrorKind::Unsupported,
            "Metadata-preserving Instance registry replacement is unsupported on this platform; source unchanged"));
    }
    stage.set_permissions(fs::Permissions::from_mode(source.metadata.mode() & 0o7777))?;
    verify_ownership_mode(&source.metadata, &stage.metadata()?)
}

// Metadata is checked after the staged data write: kernels can clear a
// security xattr while writing bytes even after a successful metadata copy.
// A failed readback refuses publication rather than silently reducing source
// permissions or provenance. Linux POSIX ACLs are included in these xattrs.
fn verify_retained_metadata(source: &Snapshot, stage: &File) -> io::Result<()> {
    verify_ownership_mode(&source.metadata, &stage.metadata()?)?;
    if extended_attributes(&source.file)? != extended_attributes(stage)? {
        return Err(denied(
            "Source extended attributes could not be retained; source unchanged",
        ));
    }
    #[cfg(target_os = "macos")]
    if native_acl(&source.file)? != native_acl(stage)? {
        return Err(denied("Source ACL could not be retained; source unchanged"));
    }
    Ok(())
}

const MAX_METADATA_BYTES: usize = 8 * 1024 * 1024;

#[cfg(any(target_os = "macos", target_os = "linux"))]
fn extended_attributes(file: &File) -> io::Result<std::collections::BTreeMap<Vec<u8>, Vec<u8>>> {
    fn names(file: &File, buffer: *mut libc::c_char, size: usize) -> libc::ssize_t {
        #[cfg(target_os = "macos")]
        unsafe {
            libc::flistxattr(file.as_raw_fd(), buffer, size, 0)
        }
        #[cfg(target_os = "linux")]
        unsafe {
            libc::flistxattr(file.as_raw_fd(), buffer, size)
        }
    }
    fn value(file: &File, name: &CString, buffer: *mut libc::c_void, size: usize) -> libc::ssize_t {
        #[cfg(target_os = "macos")]
        unsafe {
            libc::fgetxattr(file.as_raw_fd(), name.as_ptr(), buffer, size, 0, 0)
        }
        #[cfg(target_os = "linux")]
        unsafe {
            libc::fgetxattr(file.as_raw_fd(), name.as_ptr(), buffer, size)
        }
    }
    fn bounded_size(size: libc::ssize_t) -> io::Result<usize> {
        if size < 0 {
            return Err(io::Error::last_os_error());
        }
        let size = size as usize;
        if size > MAX_METADATA_BYTES {
            return Err(io::Error::other(
                "Source metadata exceeds publication bound",
            ));
        }
        Ok(size)
    }
    let size = bounded_size(names(file, std::ptr::null_mut(), 0))?;
    let mut names_buffer = vec![0u8; size];
    let read = bounded_size(names(
        file,
        names_buffer.as_mut_ptr().cast(),
        names_buffer.len(),
    ))?;
    if read > names_buffer.len() {
        return Err(conflict("Source metadata changed during readback"));
    }
    names_buffer.truncate(read);
    if !names_buffer.is_empty() && names_buffer.last() != Some(&0) {
        return Err(io::Error::other("Source metadata names are not terminated"));
    }
    let mut total = names_buffer.len();
    let mut attributes = std::collections::BTreeMap::new();
    for name_bytes in names_buffer
        .split(|byte| *byte == 0)
        .filter(|name| !name.is_empty())
    {
        let name = CString::new(name_bytes).map_err(io::Error::other)?;
        let size = bounded_size(value(file, &name, std::ptr::null_mut(), 0))?;
        total = total
            .checked_add(size)
            .filter(|total| *total <= MAX_METADATA_BYTES)
            .ok_or_else(|| io::Error::other("Source metadata exceeds publication bound"))?;
        let mut bytes = vec![0u8; size];
        let read = bounded_size(value(file, &name, bytes.as_mut_ptr().cast(), bytes.len()))?;
        if read > bytes.len() {
            return Err(conflict("Source metadata changed during readback"));
        }
        bytes.truncate(read);
        attributes.insert(name_bytes.to_vec(), bytes);
    }
    Ok(attributes)
}

#[cfg(not(any(target_os = "macos", target_os = "linux")))]
fn extended_attributes(_file: &File) -> io::Result<std::collections::BTreeMap<Vec<u8>, Vec<u8>>> {
    Err(io::Error::new(
        io::ErrorKind::Unsupported,
        "Metadata readback is unsupported; source unchanged",
    ))
}

#[cfg(target_os = "macos")]
fn native_acl(file: &File) -> io::Result<Option<Vec<u8>>> {
    unsafe extern "C" {
        fn acl_get_fd_np(fd: libc::c_int, acl_type: libc::c_int) -> *mut libc::c_void;
        fn acl_size(acl: *mut libc::c_void) -> libc::ssize_t;
        fn acl_copy_ext(
            buffer: *mut libc::c_void,
            acl: *mut libc::c_void,
            size: libc::ssize_t,
        ) -> libc::ssize_t;
        fn acl_free(acl: *mut libc::c_void) -> libc::c_int;
    }
    // ACL_TYPE_EXTENDED from the native sys/acl.h contract. Descriptor APIs
    // avoid pathname redirection and serialise identities without name lookup.
    let acl = unsafe { acl_get_fd_np(file.as_raw_fd(), 0x100) };
    if acl.is_null() {
        let error = io::Error::last_os_error();
        return if error.raw_os_error() == Some(libc::ENOENT) {
            Ok(None)
        } else {
            Err(error)
        };
    }
    let result = (|| {
        let size = unsafe { acl_size(acl) };
        if size < 0 {
            return Err(io::Error::last_os_error());
        }
        if size as usize > MAX_METADATA_BYTES {
            return Err(io::Error::other("Source ACL exceeds publication bound"));
        }
        let mut bytes = vec![0u8; size as usize];
        let read = unsafe { acl_copy_ext(bytes.as_mut_ptr().cast(), acl, size) };
        if read < 0 {
            return Err(io::Error::last_os_error());
        }
        if read as usize > bytes.len() {
            return Err(io::Error::other("Source ACL size changed during readback"));
        }
        bytes.truncate(read as usize);
        Ok(Some(bytes))
    })();
    unsafe {
        acl_free(acl);
    }
    result
}

#[cfg(target_os = "linux")]
fn preserve_linux_metadata(source: &Snapshot, stage: &File) -> io::Result<()> {
    let stage_meta = stage.metadata()?;
    if (source.metadata.uid(), source.metadata.gid()) != (stage_meta.uid(), stage_meta.gid())
        && unsafe {
            libc::fchown(
                stage.as_raw_fd(),
                source.metadata.uid(),
                source.metadata.gid(),
            )
        } != 0
    {
        return Err(io::Error::last_os_error());
    }
    stage.set_permissions(fs::Permissions::from_mode(source.metadata.mode() & 0o7777))?;
    let attributes = extended_attributes(&source.file)?;
    for name_bytes in extended_attributes(stage)?.keys() {
        if !attributes.contains_key(name_bytes) {
            let name = CString::new(name_bytes.clone()).map_err(io::Error::other)?;
            if unsafe { libc::fremovexattr(stage.as_raw_fd(), name.as_ptr()) } != 0 {
                return Err(io::Error::last_os_error());
            }
        }
    }
    for (name_bytes, bytes) in attributes {
        let name = CString::new(name_bytes).map_err(io::Error::other)?;
        if unsafe {
            libc::fsetxattr(
                stage.as_raw_fd(),
                name.as_ptr(),
                bytes.as_ptr().cast(),
                bytes.len(),
                0,
            )
        } != 0
        {
            return Err(io::Error::last_os_error());
        }
    }
    Ok(())
}

// Exclusive rename has one publication effect. A link+unlink implementation
// would expose two links and strand a committed source if the process died
// before unlink, contradicting the single-link restart contract.
fn rename_new(parent: &File, stage: &CString, target: &CString) -> io::Result<()> {
    #[cfg(target_os = "macos")]
    let result = {
        unsafe extern "C" {
            fn renameatx_np(
                from_dir: libc::c_int,
                from: *const libc::c_char,
                to_dir: libc::c_int,
                to: *const libc::c_char,
                flags: libc::c_uint,
            ) -> libc::c_int;
        }
        // RENAME_EXCL, as defined by the native sys/stdio.h contract.
        unsafe {
            renameatx_np(
                parent.as_raw_fd(),
                stage.as_ptr(),
                parent.as_raw_fd(),
                target.as_ptr(),
                0x4,
            )
        }
    };
    #[cfg(target_os = "linux")]
    let result = unsafe {
        libc::syscall(
            libc::SYS_renameat2,
            parent.as_raw_fd(),
            stage.as_ptr(),
            parent.as_raw_fd(),
            target.as_ptr(),
            libc::RENAME_NOREPLACE,
        ) as libc::c_int
    };
    #[cfg(not(any(target_os = "macos", target_os = "linux")))]
    return Err(io::Error::new(
        io::ErrorKind::Unsupported,
        "Atomic exclusive native source creation is unsupported; source unchanged",
    ));
    #[cfg(any(target_os = "macos", target_os = "linux"))]
    if result != 0 {
        return Err(io::Error::last_os_error());
    }
    #[cfg(any(target_os = "macos", target_os = "linux"))]
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::process::CommandExt;
    use std::process::{Child, Command};

    std::thread_local! {
        static ON_STAGE_CREATED: std::cell::RefCell<Option<Box<dyn FnOnce(&Path)>>> =
            const { std::cell::RefCell::new(None) };
    }

    pub(super) fn stage_created(path: &Path) {
        ON_STAGE_CREATED.with(|observer| {
            if let Some(observer) = observer.borrow_mut().take() {
                observer(path);
            }
        });
    }

    const EXEC_OBSERVATIONS: &str = "NATIVE_PUBLICATION_EXEC_OBSERVATIONS";
    const EXEC_CONTROL: &str = "NATIVE_PUBLICATION_EXEC_CONTROL";

    fn receipt(file: &File) -> String {
        let metadata = file.metadata().unwrap();
        format!("{},{},{}", file.as_raw_fd(), metadata.dev(), metadata.ino())
    }

    fn inherited(receipt: &str) -> bool {
        let parts: Vec<_> = receipt.split(',').collect();
        let fd: libc::c_int = parts[0].parse().unwrap();
        let dev: u64 = parts[1].parse().unwrap();
        let ino: u64 = parts[2].parse().unwrap();
        let mut metadata = std::mem::MaybeUninit::<libc::stat>::uninit();
        if unsafe { libc::fstat(fd, metadata.as_mut_ptr()) } != 0 {
            return false;
        }
        let metadata = unsafe { metadata.assume_init() };
        metadata.st_dev as u64 == dev && metadata.st_ino as u64 == ino
    }

    #[test]
    #[ignore = "actual exec child entry; invoked by owner descriptor regression"]
    fn exec_inheritance_child() {
        assert!(
            inherited(&std::env::var(EXEC_CONTROL).unwrap()),
            "the deliberate non-CLOEXEC control must cross exec, making the regression sensitive"
        );
        for observation in std::env::var(EXEC_OBSERVATIONS).unwrap().split(';') {
            assert!(
                !inherited(observation),
                "native owner descriptor crossed exec: {observation}"
            );
        }
    }

    #[test]
    fn actual_owner_refusal_and_drop_retain_substituted_stage_and_source() {
        struct Fixture(PathBuf);
        impl Drop for Fixture {
            fn drop(&mut self) {
                fs::remove_dir_all(&self.0).unwrap();
            }
        }
        let scratch = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../../ProjectCentral/now/tmp")
            .join(format!(
                "native-publication-substitution-{}-{}",
                std::process::id(),
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap()
                    .as_nanos()
            ));
        fs::create_dir_all(&scratch).unwrap();
        let fixture = Fixture(scratch);
        let path = fixture.0.join("source.json");
        fs::write(&path, b"retained physical source").unwrap();
        let publication = Publication::acquire(&path).unwrap();
        let retained_owned = fixture.0.join("retained-owned-stage");
        let observed = std::sync::Arc::new(std::sync::Mutex::new(None::<PathBuf>));
        let observed_stage = observed.clone();
        let retained_stage = retained_owned.clone();
        // The actual owner is paused only by this test observer. Real native
        // rename/write operations substitute its named staging inode while
        // Publication still owns the original descriptor and kernel lock.
        ON_STAGE_CREATED.with(|observer| {
            *observer.borrow_mut() = Some(Box::new(move |stage| {
                fs::rename(stage, &retained_stage).unwrap();
                fs::write(stage, b"foreign replacement retained").unwrap();
                *observed_stage.lock().unwrap() = Some(stage.to_path_buf());
            }))
        });
        let error = publication.replace(b"uncommitted candidate").unwrap_err();
        assert!(error.to_string().contains("was not deleted"), "{error}");
        drop(publication);
        let substituted = observed.lock().unwrap().clone().unwrap();
        assert_eq!(fs::read(&path).unwrap(), b"retained physical source");
        assert_eq!(
            fs::read(&substituted).unwrap(),
            b"foreign replacement retained"
        );
        assert_eq!(fs::read(&retained_owned).unwrap(), b"uncommitted candidate");
        let next = Publication::acquire(&path).unwrap();
        next.replace(b"subsequent acknowledged source").unwrap();
        drop(next);
        assert_eq!(fs::read(&path).unwrap(), b"subsequent acknowledged source");
        assert_eq!(
            fs::read(&substituted).unwrap(),
            b"foreign replacement retained"
        );
        assert_eq!(fs::read(&retained_owned).unwrap(), b"uncommitted candidate");
    }

    #[test]
    fn native_source_lock_stage_and_directory_do_not_cross_actual_exec() {
        struct Fixture(PathBuf);
        impl Drop for Fixture {
            fn drop(&mut self) {
                fs::remove_dir_all(&self.0).unwrap();
            }
        }
        struct Reap(Child);
        impl Drop for Reap {
            fn drop(&mut self) {
                let _ = self.0.kill();
                let _ = self.0.wait();
            }
        }
        let scratch = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../../ProjectCentral/now/tmp")
            .join(format!(
                "native-publication-exec-{}-{}",
                std::process::id(),
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap()
                    .as_nanos()
            ));
        fs::create_dir_all(&scratch).unwrap();
        let fixture = Fixture(scratch);
        let path = fixture.0.join("source.json");
        fs::write(&path, b"retained physical source").unwrap();
        let publication = Publication::acquire(&path).unwrap();
        let stage = open_in(
            &publication.parent,
            OsStr::new("owned-stage.tmp"),
            libc::O_RDWR | libc::O_CREAT | libc::O_EXCL,
        )
        .unwrap();
        let control_path = fixture.0.join("deliberately-inheritable-control");
        fs::write(&control_path, b"private regression control").unwrap();
        let control = File::open(&control_path).unwrap();
        let flags = unsafe { libc::fcntl(control.as_raw_fd(), libc::F_GETFD) };
        assert!(flags >= 0);
        assert_eq!(
            unsafe {
                libc::fcntl(
                    control.as_raw_fd(),
                    libc::F_SETFD,
                    flags & !libc::FD_CLOEXEC,
                )
            },
            0
        );
        let observations = [
            receipt(&publication.parent),
            receipt(&publication.lock),
            receipt(&publication.current.as_ref().unwrap().file),
            receipt(&stage),
        ]
        .join(";");
        let mut command = Command::new(std::env::current_exe().unwrap());
        command
            .args([
                "--ignored",
                "--exact",
                "instance_publication::tests::exec_inheritance_child",
                "--nocapture",
            ])
            .env(EXEC_OBSERVATIONS, observations)
            .env(EXEC_CONTROL, receipt(&control));
        // A pre_exec hook selects the fork/exec route on macOS, rather than a
        // spawn mode that closes every undesignated fd independently of these
        // owner flags. The positive control proves this actual route admits an
        // inheritable descriptor; the native descriptors must still not leak.
        unsafe {
            command.pre_exec(|| Ok(()));
        }
        let mut child = Reap(command.spawn().unwrap());
        let deadline = Instant::now() + Duration::from_secs(5);
        let status = loop {
            if let Some(status) = child.0.try_wait().unwrap() {
                break status;
            }
            if Instant::now() >= deadline {
                child.0.kill().unwrap();
                let _ = child.0.wait();
                panic!("actual exec inheritance child exceeded five-second bound");
            }
            std::thread::sleep(Duration::from_millis(10));
        };
        assert!(
            status.success(),
            "native descriptors must close across actual exec"
        );
        drop(publication);
        // The exact same stable lock inode remains usable after child exit.
        Publication::acquire(&path).unwrap();
    }
}
