//! An explicitly selected filesystem view, not a new home, credential owner or
//! session. The caller owns member roles; Workcell owns held objects and mounts.
use crate::write_boundary::WriteBoundaryRequirements;
use serde_json::{json, Value};
#[cfg(target_os = "linux")]
use sha2::{Digest, Sha256};
use std::{
    collections::BTreeSet,
    fmt, io,
    path::{Component, Path, PathBuf},
};

#[cfg(target_os = "linux")]
use std::{fs::File, sync::Arc};

pub const RUNTIME_PROJECTION_SCHEMA: &str = "workcell.runtime-projection/v1";
const MAX_INPUT: u64 = 1_048_576;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RuntimeProjection {
    /// Original invocation route, qualified against the caller's actual cwd.
    /// It is a routing basis, never a new home, identity or write grant.
    pub requested_input_root: PathBuf,
    pub input_root: PathBuf,
    pub runtime_root: PathBuf,
    pub immutable_members: Vec<String>,
    pub mutable_directories: Vec<String>,
    pub mutable_files: Vec<String>,
    pub boundary_digest: String,
}

#[derive(Debug)]
pub struct RuntimeProjectionFailure {
    phase: &'static str,
    cause: io::Error,
    material_setup_started: bool,
    operation: Option<&'static str>,
}
impl RuntimeProjectionFailure {
    pub(crate) fn new(phase: &'static str, cause: io::Error, material_setup_started: bool) -> Self {
        Self {
            phase,
            cause,
            material_setup_started,
            operation: None,
        }
    }
    #[cfg(target_os = "linux")]
    fn with_operation(mut self, operation: Option<&'static str>) -> Self {
        self.operation = operation;
        self
    }
    pub(crate) fn owner(
        phase: &'static str,
        error: impl std::error::Error + Send + Sync + 'static,
    ) -> Self {
        Self::new(phase, io::Error::other(error), false)
    }
    pub(crate) fn after_owner(
        phase: &'static str,
        error: impl std::error::Error + Send + Sync + 'static,
    ) -> Self {
        Self::new(phase, io::Error::other(error), true)
    }
    pub fn after_boundary_error(cause: epilogos_workcell_core::WorkcellError) -> Self {
        Self::after_owner("protocol-stdio", cause)
    }
    pub fn after_exec_error(cause: io::Error) -> Self {
        Self::new("provider-exec", cause, true)
    }
    pub fn as_json(&self) -> Value {
        let mut reading = json!({"schema":"workcell.runtime-projection-failure/v1", "phase":self.phase,
            "executed":false, "material_setup_started":self.material_setup_started,
            "material_effect":"setup may be partial; no rollback or automatic retry",
            "cause":{"kind":format!("{:?}",self.cause.kind()),
                "raw_os_error":self.cause.raw_os_error(),"message":self.cause.to_string()},
            "automatic_retry":false});
        if let Some(operation) = self.operation {
            reading["operation"] = json!(operation);
        }
        reading
    }
}
impl fmt::Display for RuntimeProjectionFailure {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "runtime projection {}: {}", self.phase, self.cause)
    }
}
impl std::error::Error for RuntimeProjectionFailure {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        Some(&self.cause)
    }
}
fn invalid(message: &str) -> RuntimeProjectionFailure {
    RuntimeProjectionFailure::new(
        "request",
        io::Error::new(io::ErrorKind::InvalidInput, message),
        false,
    )
}
impl RuntimeProjection {
    pub fn from_json(raw: &str) -> Result<Self, RuntimeProjectionFailure> {
        if raw.len() as u64 > MAX_INPUT {
            return Err(invalid("projection exceeds 1 MiB"));
        }
        let v: Value = serde_json::from_str(raw).map_err(|e| {
            RuntimeProjectionFailure::new(
                "request-json",
                io::Error::new(io::ErrorKind::InvalidData, e),
                false,
            )
        })?;
        let o = v
            .as_object()
            .ok_or_else(|| invalid("projection must be an object"))?;
        let keys = [
            "schema",
            "requested_input_root",
            "input_root",
            "runtime_root",
            "immutable_members",
            "mutable_directories",
            "mutable_files",
            "boundary_digest",
        ];
        if v["schema"] != RUNTIME_PROJECTION_SCHEMA
            || o.len() != keys.len()
            || o.keys().any(|k| !keys.contains(&k.as_str()))
        {
            return Err(invalid("unknown or missing projection field"));
        }
        let text = |key: &str| -> Result<String, RuntimeProjectionFailure> {
            v[key]
                .as_str()
                .filter(|s| !s.is_empty())
                .map(str::to_owned)
                .ok_or_else(|| invalid("nonempty projection strings required"))
        };
        let mut seen = BTreeSet::new();
        let mut members = |key: &str| -> Result<Vec<String>, RuntimeProjectionFailure> {
            let a = v[key]
                .as_array()
                .ok_or_else(|| invalid("member list required"))?;
            if a.len() > 16 {
                return Err(invalid("at most 16 members per kind"));
            }
            a.iter().map(|s| {
                let s = s.as_str().ok_or_else(|| invalid("member must be text"))?;
                let mut components = Path::new(s).components();
                if s.len() > 128 || !matches!(components.next(), Some(Component::Normal(n)) if n == std::ffi::OsStr::new(s))
                    || components.next().is_some() || s.chars().any(|c| matches!(c, '/' | '\\' | '\0')) || !seen.insert(s.to_owned()) {
                    return Err(invalid("members must be distinct literal single components"));
                }
                Ok(s.to_owned())
            }).collect()
        };
        let immutable_members = members("immutable_members")?;
        let mutable_directories = members("mutable_directories")?;
        let mutable_files = members("mutable_files")?;
        let requested_input_root = PathBuf::from(text("requested_input_root")?);
        if !requested_input_root.is_absolute()
            || requested_input_root.parent().is_none()
            || requested_input_root
                .as_os_str()
                .as_encoded_bytes()
                .contains(&0)
        {
            return Err(invalid(
                "requested input must be an absolute representable invocation route",
            ));
        }
        // Legal lexical aliases and parent spelling stay intact here. The
        // native owner must resolve them to the admitted held input.
        let input_root = PathBuf::from(text("input_root")?);
        let runtime_root = PathBuf::from(text("runtime_root")?);
        for path in [&input_root, &runtime_root] {
            if !path.is_absolute()
                || path.parent().is_none()
                || path
                    .components()
                    .any(|c| matches!(c, Component::ParentDir | Component::CurDir))
            {
                return Err(invalid(
                    "projection roots must be absolute without parent traversal",
                ));
            }
        }
        if input_root.starts_with(&runtime_root) || runtime_root.starts_with(&input_root) {
            return Err(invalid(
                "readonly input and mutable runtime roots must be disjoint",
            ));
        }
        Ok(Self {
            requested_input_root,
            input_root,
            runtime_root,
            immutable_members,
            mutable_directories,
            mutable_files,
            boundary_digest: text("boundary_digest")?,
        })
    }
    /// A checked held regular file, never a metadata-then-blocking-read route.
    pub fn read(path: &Path, expected_digest: &str) -> Result<Self, RuntimeProjectionFailure> {
        #[cfg(target_os = "linux")]
        {
            use std::io::Read;
            use std::os::unix::fs::{MetadataExt, OpenOptionsExt};
            let file = std::fs::OpenOptions::new()
                .read(true)
                .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK | libc::O_CLOEXEC)
                .open(path)
                .map_err(|e| RuntimeProjectionFailure::new("request-open", e, false))?;
            let before = file
                .metadata()
                .map_err(|e| RuntimeProjectionFailure::new("request-stat", e, false))?;
            if !before.is_file()
                || before.nlink() != 1
                || before.len() > MAX_INPUT
                || before.uid() != unsafe { libc::geteuid() }
            {
                return Err(invalid(
                    "projection requires an owned bounded single-link regular file",
                ));
            }
            let mut bytes = Vec::new();
            (&file)
                .take(MAX_INPUT + 1)
                .read_to_end(&mut bytes)
                .map_err(|e| RuntimeProjectionFailure::new("request-read", e, false))?;
            let after = file
                .metadata()
                .map_err(|e| RuntimeProjectionFailure::new("request-stat", e, false))?;
            let named = std::fs::symlink_metadata(path)
                .map_err(|e| RuntimeProjectionFailure::new("request-name", e, false))?;
            if bytes.len() as u64 > MAX_INPUT
                || before.len() != after.len()
                || before.ctime() != after.ctime()
                || before.ctime_nsec() != after.ctime_nsec()
                || named.dev() != after.dev()
                || named.ino() != after.ino()
                || !named.is_file()
                || named.nlink() != 1
            {
                return Err(invalid("projection changed while reading"));
            }
            if format!("sha256:{:x}", Sha256::digest(&bytes)) != expected_digest {
                return Err(invalid("projection digest changed"));
            }
            Self::from_json(std::str::from_utf8(&bytes).map_err(|e| {
                RuntimeProjectionFailure::new(
                    "request-text",
                    io::Error::new(io::ErrorKind::InvalidData, e),
                    false,
                )
            })?)
        }
        #[cfg(not(target_os = "linux"))]
        {
            let _ = (path, expected_digest);
            Err(RuntimeProjectionFailure::new(
                "platform",
                io::Error::new(
                    io::ErrorKind::Unsupported,
                    "runtime projection requires the supported native Linux namespace owner",
                ),
                false,
            ))
        }
    }
    pub(crate) fn validate_boundary(
        &self,
        requirements: &WriteBoundaryRequirements,
    ) -> Result<(), RuntimeProjectionFailure> {
        let checked = Self::from_json(&json!({"schema":RUNTIME_PROJECTION_SCHEMA,
            "requested_input_root":self.requested_input_root,"input_root":self.input_root,"runtime_root":self.runtime_root,
            "immutable_members":self.immutable_members,"mutable_directories":self.mutable_directories,
            "mutable_files":self.mutable_files,"boundary_digest":self.boundary_digest}).to_string())?;
        if checked != *self {
            return Err(invalid("projection structure changed"));
        }
        if self.boundary_digest != requirements.digest()
            || !requirements
                .writable_paths
                .iter()
                .any(|p| self.runtime_root.starts_with(p) && self.runtime_root != *p)
        {
            return Err(invalid("runtime projection needs the same exact write boundary and a strict descendant of its writable material"));
        }
        if requirements
            .writable_paths
            .iter()
            .any(|p| p.starts_with(&self.input_root) || self.input_root.starts_with(p))
            || requirements
                .protected_paths
                .iter()
                .any(|p| p.starts_with(&self.input_root) || self.input_root.starts_with(p))
        {
            return Err(invalid(
                "projection cannot alias a writable or protected boundary object",
            ));
        }
        Ok(())
    }
}

#[cfg(target_os = "linux")]
pub(crate) struct ProjectionAlias {
    pub file: Arc<File>,
    pub directory: bool,
}
#[cfg(target_os = "linux")]
pub(crate) struct AppliedProjection {
    pub aliases: Vec<ProjectionAlias>,
    _held: Vec<File>,
}

#[cfg(not(target_os = "linux"))]
pub(crate) struct AppliedProjection;

#[cfg(target_os = "linux")]
mod linux {
    use super::*;
    use std::{
        ffi::CString,
        fs,
        os::{
            fd::{AsRawFd, FromRawFd},
            unix::{
                ffi::OsStrExt,
                fs::{MetadataExt, OpenOptionsExt},
            },
        },
    };
    struct Held {
        path: PathBuf,
        file: File,
        dev: u64,
        ino: u64,
    }
    impl Held {
        fn open(path: &Path, directory: bool) -> io::Result<Self> {
            Self::open_observed(
                path,
                directory,
                &mut None,
                ["open", "fstat", "form", "canonicalize", "route"],
            )
        }
        // The same held reader; the optional label records the entered operation,
        // never an inferred errno meaning or a pathname/authority.
        fn open_observed(
            path: &Path,
            directory: bool,
            operation: &mut Option<&'static str>,
            operations: [&'static str; 5],
        ) -> io::Result<Self> {
            let flags = libc::O_NOFOLLOW
                | libc::O_NONBLOCK
                | libc::O_CLOEXEC
                | if directory { libc::O_DIRECTORY } else { 0 };
            *operation = Some(operations[0]);
            let file = fs::OpenOptions::new()
                .read(true)
                .custom_flags(flags)
                .open(path)?;
            *operation = Some(operations[1]);
            let m = file.metadata()?;
            *operation = Some(operations[2]);
            if if directory {
                !m.is_dir()
            } else {
                !m.is_file() || m.nlink() != 1
            } {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidInput,
                    "wrong projection object form",
                ));
            }
            *operation = Some(operations[3]);
            let canonical = fs::canonicalize(path)?;
            *operation = Some(operations[4]);
            if canonical != path {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidInput,
                    "projection path redirected",
                ));
            }
            Ok(Self {
                path: path.into(),
                file,
                dev: m.dev(),
                ino: m.ino(),
            })
        }
        // File descriptors remember their mount namespace. Preserve the original
        // reader and admit a same-object descriptor in the namespace that will
        // mount it; a pathname reopen alone is never an identity witness.
        fn mount_handle(&self, operation: &mut Option<&'static str>) -> io::Result<Self> {
            let basis = |m: &fs::Metadata| {
                (
                    m.dev(),
                    m.ino(),
                    m.mode(),
                    m.nlink(),
                    m.uid(),
                    m.gid(),
                    m.len(),
                    m.mtime(),
                    m.mtime_nsec(),
                    m.ctime(),
                    m.ctime_nsec(),
                )
            };
            *operation = Some("readonly-input.reaffiliate.named-before");
            self.check()?;
            *operation = Some("readonly-input.reaffiliate.held-before");
            let original = self.file.metadata()?;
            let current = Self::open_observed(
                &self.path,
                original.is_dir(),
                operation,
                [
                    "readonly-input.reaffiliate.open",
                    "readonly-input.reaffiliate.fstat",
                    "readonly-input.reaffiliate.form",
                    "readonly-input.reaffiliate.canonicalize",
                    "readonly-input.reaffiliate.route",
                ],
            )?;
            *operation = Some("readonly-input.reaffiliate.original-after");
            let original_after = self.file.metadata()?;
            *operation = Some("readonly-input.reaffiliate.current-after");
            let current_after = current.file.metadata()?;
            *operation = Some("readonly-input.reaffiliate.named-after");
            let named = fs::symlink_metadata(&self.path)?;
            *operation = Some("readonly-input.reaffiliate.identity");
            if basis(&original) != basis(&original_after)
                || basis(&original) != basis(&current_after)
                || basis(&original) != basis(&named)
                || current.dev != self.dev
                || current.ino != self.ino
                || named.file_type().is_symlink()
            {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidInput,
                    "mount object changed during namespace reaffiliation",
                ));
            }
            Ok(current)
        }
        fn check(&self) -> io::Result<()> {
            let m = fs::symlink_metadata(&self.path)?;
            if m.dev() != self.dev || m.ino() != self.ino || m.file_type().is_symlink() {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidInput,
                    "projection object replaced",
                ));
            }
            Ok(())
        }
        fn check_requested(&self, requested: &Path) -> io::Result<()> {
            let resolved = fs::canonicalize(requested)?;
            if resolved != self.path {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidInput,
                    "requested input route no longer resolves to the admitted origin",
                ));
            }
            // Before mounting this is the held original; at the final
            // checkpoint it is the held assembled view at the same route.
            // Original lower member identity remains checked through its fd.
            self.check()
        }
        fn fd_path(&self) -> PathBuf {
            PathBuf::from(format!("/proc/self/fd/{}", self.file.as_raw_fd()))
        }
    }
    fn open_at(parent: &Held, name: &std::ffi::OsStr, directory: bool) -> io::Result<Held> {
        let name = CString::new(name.as_bytes())
            .map_err(|e| io::Error::new(io::ErrorKind::InvalidInput, e))?;
        let flags = libc::O_RDONLY
            | libc::O_NOFOLLOW
            | libc::O_NONBLOCK
            | libc::O_CLOEXEC
            | if directory { libc::O_DIRECTORY } else { 0 };
        let raw = unsafe { libc::openat(parent.file.as_raw_fd(), name.as_ptr(), flags) };
        if raw < 0 {
            return Err(io::Error::last_os_error());
        }
        // SAFETY: successful openat returns one new owned descriptor.
        let file = unsafe { File::from_raw_fd(raw) };
        let m = file.metadata()?;
        if if directory {
            !m.is_dir()
        } else {
            !m.is_file() || m.nlink() != 1
        } {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "wrong runtime object form",
            ));
        }
        let path = parent
            .path
            .join(std::ffi::OsStr::from_bytes(name.as_bytes()));
        let h = Held {
            path,
            file,
            dev: m.dev(),
            ino: m.ino(),
        };
        h.check()?;
        if fs::canonicalize(&h.path)? != h.path {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "runtime route redirected",
            ));
        }
        Ok(h)
    }
    fn named_basis(parent: &Held, name: &str, expected: Option<&Held>) -> io::Result<()> {
        let native = CString::new(name.as_bytes())
            .map_err(|e| io::Error::new(io::ErrorKind::InvalidInput, e))?;
        let mut observed = std::mem::MaybeUninit::<libc::stat>::uninit();
        // SAFETY: valid owned directory fd, bounded NUL-free member, writable
        // stat output. No following a replacement symlink or reading its body.
        if unsafe {
            libc::fstatat(
                parent.file.as_raw_fd(),
                native.as_ptr(),
                observed.as_mut_ptr(),
                libc::AT_SYMLINK_NOFOLLOW,
            )
        } != 0
        {
            let error = io::Error::last_os_error();
            if error.kind() == io::ErrorKind::NotFound && expected.is_none() {
                return Ok(());
            }
            return Err(error);
        }
        // SAFETY: successful fstatat initialized the complete native stat.
        let observed = unsafe { observed.assume_init() };
        match expected {
            Some(held) => {
                let metadata = held.file.metadata()?;
                if observed.st_dev != held.dev
                    || observed.st_ino != held.ino
                    || observed.st_mode & libc::S_IFMT
                        != if metadata.is_dir() {
                            libc::S_IFDIR
                        } else {
                            libc::S_IFREG
                        }
                    || (!metadata.is_dir() && observed.st_nlink != 1)
                {
                    return Err(io::Error::new(
                        io::ErrorKind::InvalidInput,
                        "original member basis changed",
                    ));
                }
            }
            None => {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidInput,
                    "originally absent member appeared",
                ))
            }
        }
        Ok(())
    }
    fn private_dir(parent: &Held, name: &std::ffi::OsStr) -> io::Result<Held> {
        parent.check()?;
        let native = CString::new(name.as_bytes())
            .map_err(|e| io::Error::new(io::ErrorKind::InvalidInput, e))?;
        if unsafe { libc::mkdirat(parent.file.as_raw_fd(), native.as_ptr(), 0o700) } != 0 {
            let e = io::Error::last_os_error();
            if e.kind() != io::ErrorKind::AlreadyExists {
                return Err(e);
            }
        }
        let h = open_at(parent, name, true)?;
        let m = h.file.metadata()?;
        if m.uid() != unsafe { libc::geteuid() } || m.mode() & 0o077 != 0 {
            return Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                "retained runtime directory must be owner-private",
            ));
        }
        Ok(h)
    }
    fn fresh_dir(parent: &Held, name: &std::ffi::OsStr) -> io::Result<Held> {
        parent.check()?;
        let native = CString::new(name.as_bytes())
            .map_err(|e| io::Error::new(io::ErrorKind::InvalidInput, e))?;
        if unsafe { libc::mkdirat(parent.file.as_raw_fd(), native.as_ptr(), 0o700) } != 0 {
            return Err(io::Error::last_os_error());
        }
        open_at(parent, name, true)
    }
    fn private_file(parent: &Held, name: &std::ffi::OsStr) -> io::Result<Held> {
        parent.check()?;
        let native = CString::new(name.as_bytes())
            .map_err(|e| io::Error::new(io::ErrorKind::InvalidInput, e))?;
        let raw = unsafe {
            libc::openat(
                parent.file.as_raw_fd(),
                native.as_ptr(),
                libc::O_WRONLY | libc::O_CREAT | libc::O_EXCL | libc::O_NOFOLLOW | libc::O_CLOEXEC,
                0o600,
            )
        };
        if raw < 0 {
            let e = io::Error::last_os_error();
            if e.kind() != io::ErrorKind::AlreadyExists {
                return Err(e);
            }
        } else {
            unsafe { File::from_raw_fd(raw) }.sync_all()?;
        }
        let h = open_at(parent, name, false)?;
        let m = h.file.metadata()?;
        // The native provider may set installation_id to 0644. Its containing
        // held directory remains 0700; never silently chmod retained material.
        if m.uid() != unsafe { libc::geteuid() } || m.mode() & 0o022 != 0 {
            return Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                "runtime file ownership/write privacy changed",
            ));
        }
        let write = unsafe {
            libc::openat(
                parent.file.as_raw_fd(),
                native.as_ptr(),
                libc::O_WRONLY | libc::O_NOFOLLOW | libc::O_NONBLOCK | libc::O_CLOEXEC,
            )
        };
        if write < 0 {
            return Err(io::Error::last_os_error());
        }
        let write = unsafe { File::from_raw_fd(write) };
        let w = write.metadata()?;
        if w.dev() != h.dev || w.ino() != h.ino {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "runtime write admission inode changed",
            ));
        }
        Ok(h)
    }
    fn c(path: &Path) -> io::Result<CString> {
        CString::new(path.as_os_str().as_bytes())
            .map_err(|e| io::Error::new(io::ErrorKind::InvalidInput, e))
    }
    fn mount(
        source: Option<&Path>,
        target: &Path,
        kind: Option<&str>,
        flags: libc::c_ulong,
        data: Option<&str>,
    ) -> io::Result<()> {
        let source = source.map(c).transpose()?;
        let target = c(target)?;
        let kind = kind
            .map(CString::new)
            .transpose()
            .map_err(|e| io::Error::new(io::ErrorKind::InvalidInput, e))?;
        let data = data
            .map(CString::new)
            .transpose()
            .map_err(|e| io::Error::new(io::ErrorKind::InvalidInput, e))?;
        if unsafe {
            libc::mount(
                source.as_ref().map_or(std::ptr::null(), |s| s.as_ptr()),
                target.as_ptr(),
                kind.as_ref().map_or(std::ptr::null(), |s| s.as_ptr()),
                flags,
                data.as_ref()
                    .map_or(std::ptr::null(), |s| s.as_ptr().cast()),
            )
        } != 0
        {
            return Err(io::Error::last_os_error());
        }
        Ok(())
    }
    fn readonly_bind(h: &Held) -> io::Result<()> {
        readonly_bind_observed(h, &mut None, ["bind", "remount"])
    }
    fn readonly_bind_observed(
        h: &Held,
        operation: &mut Option<&'static str>,
        operations: [&'static str; 2],
    ) -> io::Result<()> {
        *operation = Some(operations[0]);
        mount(Some(&h.fd_path()), &h.path, None, libc::MS_BIND, None)?;
        *operation = Some(operations[1]);
        mount(
            None,
            &h.path,
            None,
            libc::MS_BIND | libc::MS_REMOUNT | libc::MS_RDONLY | libc::MS_NOSUID | libc::MS_NODEV,
            None,
        )
    }
    fn enter_namespace() -> io::Result<()> {
        let uid = unsafe { libc::geteuid() };
        let gid = unsafe { libc::getegid() };
        if uid == 0 {
            return Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                "runtime projection refuses privileged users",
            ));
        }
        if unsafe { libc::unshare(libc::CLONE_NEWUSER | libc::CLONE_NEWNS) } != 0 {
            return Err(io::Error::last_os_error());
        }
        fs::write("/proc/self/setgroups", "deny")?;
        fs::write("/proc/self/uid_map", format!("{uid} {uid} 1\n"))?;
        fs::write("/proc/self/gid_map", format!("{gid} {gid} 1\n"))?;
        mount(
            None,
            Path::new("/"),
            None,
            libc::MS_REC | libc::MS_PRIVATE,
            None,
        )
    }
    fn drop_capabilities() -> io::Result<()> {
        #[repr(C)]
        struct Header {
            version: u32,
            pid: i32,
        }
        #[repr(C)]
        struct Data {
            effective: u32,
            permitted: u32,
            inheritable: u32,
        }
        if unsafe { libc::prctl(libc::PR_SET_NO_NEW_PRIVS, 1, 0, 0, 0) } != 0 {
            return Err(io::Error::last_os_error());
        }
        // Drop the bounding set while the namespace owner still has SETPCAP.
        for cap in 0..64 {
            if unsafe { libc::prctl(libc::PR_CAPBSET_DROP, cap, 0, 0, 0) } != 0 {
                let e = io::Error::last_os_error();
                if e.raw_os_error() == Some(libc::EINVAL) {
                    break;
                }
                return Err(e);
            }
        }
        if unsafe { libc::prctl(47, 4, 0, 0, 0) } != 0 {
            return Err(io::Error::last_os_error());
        } // PR_CAP_AMBIENT_CLEAR_ALL
        let header = Header {
            version: 0x2008_0522,
            pid: 0,
        };
        let data = [
            Data {
                effective: 0,
                permitted: 0,
                inheritable: 0,
            },
            Data {
                effective: 0,
                permitted: 0,
                inheritable: 0,
            },
        ];
        if unsafe { libc::syscall(libc::SYS_capset, &header as *const Header, data.as_ptr()) } != 0
        {
            return Err(io::Error::last_os_error());
        }
        Ok(())
    }
    pub(super) fn apply(
        p: &RuntimeProjection,
        requirements: &WriteBoundaryRequirements,
    ) -> Result<AppliedProjection, RuntimeProjectionFailure> {
        let mut phase = "origin-admission";
        let mut started = false;
        let mut operation = None;
        let result = (|| -> io::Result<AppliedProjection> {
            let input = Held::open(&p.input_root, true)?;
            input.check_requested(&p.requested_input_root)?;
            let anchor = requirements
                .writable_paths
                .iter()
                .filter(|a| p.runtime_root.starts_with(a))
                .max_by_key(|a| a.components().count())
                .ok_or_else(|| {
                    io::Error::new(
                        io::ErrorKind::InvalidInput,
                        "runtime has no writable anchor",
                    )
                })?;
            let anchor = Held::open(anchor, true)?;
            let mut immutable = Vec::new();
            for member in &p.immutable_members {
                let path = input.path.join(member);
                match fs::symlink_metadata(&path) {
                    Ok(m) if m.is_dir() && !m.file_type().is_symlink() => {
                        immutable.push(Some(open_at(&input, std::ffi::OsStr::new(member), true)?))
                    }
                    Ok(m) if m.is_file() && m.nlink() == 1 => {
                        immutable.push(Some(open_at(&input, std::ffi::OsStr::new(member), false)?))
                    }
                    Ok(_) => {
                        return Err(io::Error::new(
                            io::ErrorKind::InvalidInput,
                            "immutable input member redirected or nonordinary",
                        ))
                    }
                    Err(e) if e.kind() == io::ErrorKind::NotFound => immutable.push(None),
                    Err(e) => return Err(e),
                }
            }
            let mut lowers = Vec::new();
            for member in &p.mutable_directories {
                let path = input.path.join(member);
                match fs::symlink_metadata(&path) {
                    Ok(_) => {
                        lowers.push(Some(open_at(&input, std::ffi::OsStr::new(member), true)?))
                    }
                    Err(e) if e.kind() == io::ErrorKind::NotFound => lowers.push(None),
                    Err(e) => return Err(e),
                }
            }
            let mut file_lowers = Vec::new();
            for member in &p.mutable_files {
                let path = input.path.join(member);
                match fs::symlink_metadata(&path) {
                    Ok(m) if m.is_file() && m.nlink() == 1 => file_lowers.push(Some(open_at(
                        &input,
                        std::ffi::OsStr::new(member),
                        false,
                    )?)),
                    Ok(_) => {
                        return Err(io::Error::new(
                            io::ErrorKind::InvalidInput,
                            "mutable original file is redirected or nonordinary",
                        ))
                    }
                    Err(e) if e.kind() == io::ErrorKind::NotFound => file_lowers.push(None),
                    Err(e) => return Err(e),
                }
            }
            // No provider-specific names here. Every unselected original entry
            // remains in the immutable lower; selected source directories alone
            // get a copy-on-write continuation, not a credential-file copy.
            input.check_requested(&p.requested_input_root)?;
            phase = "material-setup";
            started = true;
            let mut cursor = Held::open(&anchor.path, true)?;
            let mut held = Vec::new();
            for component in p
                .runtime_root
                .strip_prefix(&anchor.path)
                .map_err(|e| io::Error::new(io::ErrorKind::InvalidInput, e))?
                .components()
            {
                let next = private_dir(&cursor, component.as_os_str())?;
                held.push(cursor.file);
                cursor = next;
            }
            let root = cursor;
            // The readonly skeleton is exclusive per launch, so retained Task
            // material cannot impersonate a config input through overlay xattrs.
            let stamp = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map_err(io::Error::other)?
                .as_nanos();
            let control = fresh_dir(
                &root,
                std::ffi::OsStr::new(&format!("view-{}-{stamp}", std::process::id())),
            )?;
            let placeholders = private_dir(&control, std::ffi::OsStr::new("readonly-members"))?;
            let dirs = private_dir(&root, std::ffi::OsStr::new("directories"))?;
            let files = private_dir(&root, std::ffi::OsStr::new("files"))?;
            let empty = private_dir(&control, std::ffi::OsStr::new("empty-lower"))?;
            if fs::read_dir(&empty.path)?.next().is_some() {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidInput,
                    "empty lower contains foreign material",
                ));
            }
            let mut views = Vec::new();
            let mut file_views = Vec::new();
            for member in &p.mutable_directories {
                let placeholder = private_dir(&placeholders, std::ffi::OsStr::new(member))?;
                if fs::read_dir(&placeholder.path)?.next().is_some() {
                    return Err(io::Error::new(
                        io::ErrorKind::InvalidInput,
                        "readonly placeholder contains foreign input",
                    ));
                }
                let parent = private_dir(&dirs, std::ffi::OsStr::new(member))?;
                let upper = private_dir(&parent, std::ffi::OsStr::new("upper"))?;
                let work = private_dir(&parent, std::ffi::OsStr::new("work"))?;
                if upper.dev != work.dev {
                    return Err(io::Error::new(
                        io::ErrorKind::InvalidInput,
                        "overlay upper/work filesystems differ",
                    ));
                }
                views.push((placeholder, upper, work));
                held.push(parent.file);
            }
            for (index, member) in p.mutable_files.iter().enumerate() {
                let placeholder = private_file(&placeholders, std::ffi::OsStr::new(member))?;
                if placeholder.file.metadata()?.len() != 0 {
                    return Err(io::Error::new(
                        io::ErrorKind::InvalidInput,
                        "readonly placeholder contains foreign input",
                    ));
                }
                let parent = private_dir(&files, std::ffi::OsStr::new(member))?;
                let upper = private_dir(&parent, std::ffi::OsStr::new("upper"))?;
                let work = private_dir(&parent, std::ffi::OsStr::new("work"))?;
                if upper.dev != work.dev {
                    return Err(io::Error::new(
                        io::ErrorKind::InvalidInput,
                        "file overlay upper/work filesystems differ",
                    ));
                }
                let view = private_dir(&control, std::ffi::OsStr::new(&format!("file-{member}")))?;
                match fs::symlink_metadata(upper.path.join(member)) {
                    Ok(_) => {
                        held.push(private_file(&upper, std::ffi::OsStr::new(member))?.file);
                    }
                    Err(e)
                        if e.kind() == io::ErrorKind::NotFound && file_lowers[index].is_none() =>
                    {
                        held.push(private_file(&upper, std::ffi::OsStr::new(member))?.file);
                    }
                    Err(e) if e.kind() == io::ErrorKind::NotFound => {}
                    Err(e) => return Err(e),
                }
                file_views.push((parent, upper, work, view));
                held.push(placeholder.file);
            }
            let expected = p
                .mutable_directories
                .iter()
                .chain(&p.mutable_files)
                .map(String::as_str)
                .collect::<BTreeSet<_>>();
            let actual = fs::read_dir(&placeholders.path)?
                .map(|e| e.map(|e| e.file_name()))
                .collect::<io::Result<Vec<_>>>()?;
            if actual.len() != expected.len()
                || actual
                    .iter()
                    .any(|n| !n.to_str().is_some_and(|n| expected.contains(n)))
            {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidInput,
                    "readonly skeleton contains unselected material",
                ));
            }
            input.check_requested(&p.requested_input_root)?;
            anchor.check()?;
            root.check()?;
            for (name, expected) in p
                .immutable_members
                .iter()
                .zip(&immutable)
                .chain(p.mutable_directories.iter().zip(&lowers))
                .chain(p.mutable_files.iter().zip(&file_lowers))
            {
                named_basis(&input, name, expected.as_ref())?;
            }
            phase = "namespace";
            enter_namespace()?;
            phase = "readonly-input";
            operation = Some("readonly-input.requested-route");
            input.check_requested(&p.requested_input_root)?;
            let input_mount = input.mount_handle(&mut operation)?;
            let placeholders_mount = placeholders.mount_handle(&mut operation)?;
            let empty_mount = empty.mount_handle(&mut operation)?;
            let mut mount_lowers = Vec::with_capacity(lowers.len());
            for lower in &lowers {
                mount_lowers.push(
                    lower
                        .as_ref()
                        .map(|held| held.mount_handle(&mut operation))
                        .transpose()?,
                );
            }
            let mut mount_views = Vec::with_capacity(views.len());
            for (_, upper, work) in &views {
                mount_views.push((
                    upper.mount_handle(&mut operation)?,
                    work.mount_handle(&mut operation)?,
                ));
            }
            let mut mount_file_views = Vec::with_capacity(file_views.len());
            for (_, upper, work, target) in &file_views {
                mount_file_views.push((
                    upper.mount_handle(&mut operation)?,
                    work.mount_handle(&mut operation)?,
                    target.mount_handle(&mut operation)?,
                ));
            }
            operation = Some("readonly-input.requested-route-after");
            input.check_requested(&p.requested_input_root)?;
            anchor.check()?;
            root.check()?;
            readonly_bind_observed(
                &placeholders_mount,
                &mut operation,
                [
                    "readonly-input.skeleton.bind",
                    "readonly-input.skeleton.remount",
                ],
            )?;
            readonly_bind_observed(
                &empty_mount,
                &mut operation,
                ["readonly-input.empty.bind", "readonly-input.empty.remount"],
            )?;
            let options = format!(
                "lowerdir={}:{},userxattr,redirect_dir=nofollow",
                placeholders_mount.fd_path().display(),
                input_mount.fd_path().display()
            );
            operation = Some("readonly-input.overlay.mount");
            mount(
                Some(Path::new("overlay")),
                &input.path,
                Some("overlay"),
                libc::MS_RDONLY | libc::MS_NOSUID | libc::MS_NODEV,
                Some(&options),
            )?;
            let view = Held::open_observed(
                &input.path,
                true,
                &mut operation,
                [
                    "readonly-input.view.open",
                    "readonly-input.view.fstat",
                    "readonly-input.view.form",
                    "readonly-input.view.canonicalize",
                    "readonly-input.view.route",
                ],
            )?;
            operation = None;
            let mut aliases = Vec::new();
            phase = "mutable-directory-view";
            for (index, member) in p.mutable_directories.iter().enumerate() {
                let (upper, work) = &mount_views[index];
                upper.check()?;
                work.check()?;
                let lower = mount_lowers[index].as_ref().unwrap_or(&empty_mount);
                let options = format!(
                    "lowerdir={},upperdir={},workdir={},userxattr,redirect_dir=nofollow",
                    lower.fd_path().display(),
                    upper.fd_path().display(),
                    work.fd_path().display()
                );
                let target = view.fd_path().join(member);
                mount(
                    Some(Path::new("overlay")),
                    &target,
                    Some("overlay"),
                    libc::MS_NOSUID | libc::MS_NODEV,
                    Some(&options),
                )?;
                let alias = Held::open(&input.path.join(member), true)?;
                aliases.push(ProjectionAlias {
                    file: Arc::new(alias.file),
                    directory: true,
                });
            }
            phase = "mutable-file-view";
            for (index, member) in p.mutable_files.iter().enumerate() {
                let (upper, work, target) = &mount_file_views[index];
                upper.check()?;
                work.check()?;
                target.check()?;
                let options = format!(
                    "lowerdir={},upperdir={},workdir={},userxattr,redirect_dir=nofollow",
                    input_mount.fd_path().display(),
                    upper.fd_path().display(),
                    work.fd_path().display()
                );
                mount(
                    Some(Path::new("overlay")),
                    &target.path,
                    Some("overlay"),
                    libc::MS_NOSUID | libc::MS_NODEV,
                    Some(&options),
                )?;
                let file_view = Held::open(&target.path, true)?;
                // The original file is only copied up by the kernel for this
                // selected runtime role. Actual ACL/mode write denial stays a
                // native cause. No immutable auth/config file is opened write.
                let native = CString::new(member.as_bytes())
                    .map_err(|e| io::Error::new(io::ErrorKind::InvalidInput, e))?;
                let raw = unsafe {
                    libc::openat(
                        file_view.file.as_raw_fd(),
                        native.as_ptr(),
                        libc::O_WRONLY | libc::O_NOFOLLOW | libc::O_NONBLOCK | libc::O_CLOEXEC,
                    )
                };
                if raw < 0 {
                    return Err(io::Error::last_os_error());
                }
                let write = unsafe { File::from_raw_fd(raw) };
                let metadata = write.metadata()?;
                if !metadata.is_file() || metadata.nlink() != 1 {
                    return Err(io::Error::new(
                        io::ErrorKind::InvalidInput,
                        "file continuation is not ordinary",
                    ));
                }
                let material = open_at(&file_view, std::ffi::OsStr::new(member), false)?;
                mount(
                    Some(&material.fd_path()),
                    &view.fd_path().join(member),
                    None,
                    libc::MS_BIND,
                    None,
                )?;
                let alias = Held::open(&input.path.join(member), false)?;
                // Hide the writable containing overlay behind a readonly bind.
                // The only remaining writable view is this selected file mount;
                // Task-visible sibling auth/config entries cannot be copied up.
                readonly_bind(&file_view)?;
                aliases.push(ProjectionAlias {
                    file: Arc::new(alias.file),
                    directory: false,
                });
                held.extend([write, material.file, file_view.file]);
            }
            // Protect the skeleton's physical route too: the normal Task grant
            // must not let this body plant auth/config entries into its lower.
            phase = "final-basis";
            view.check_requested(&p.requested_input_root)?;
            anchor.check()?;
            root.check()?;
            // The original fd still addresses the readonly lower object even
            // though its visible pathname now addresses the assembled view.
            for (name, expected) in p
                .immutable_members
                .iter()
                .zip(&immutable)
                .chain(p.mutable_directories.iter().zip(&lowers))
                .chain(p.mutable_files.iter().zip(&file_lowers))
            {
                named_basis(&input, name, expected.as_ref())?;
            }
            phase = "capability-retirement";
            drop_capabilities()?;
            held.extend([
                input.file,
                anchor.file,
                root.file,
                placeholders.file,
                dirs.file,
                files.file,
                empty.file,
                view.file,
                control.file,
            ]);
            // Keep original reader handles and same-object mount handles alive
            // through the existing AppliedProjection/exec lifecycle.
            held.extend([input_mount.file, placeholders_mount.file, empty_mount.file]);
            held.extend(mount_lowers.into_iter().flatten().map(|h| h.file));
            for (upper, work) in mount_views {
                held.extend([upper.file, work.file]);
            }
            for (upper, work, target) in mount_file_views {
                held.extend([upper.file, work.file, target.file]);
            }
            held.extend(immutable.into_iter().flatten().map(|h| h.file));
            for lower in lowers.into_iter().flatten() {
                held.push(lower.file);
            }
            for (placeholder, upper, work) in views {
                held.extend([placeholder.file, upper.file, work.file]);
            }
            for lower in file_lowers.into_iter().flatten() {
                held.push(lower.file);
            }
            for (parent, upper, work, view) in file_views {
                held.extend([parent.file, upper.file, work.file, view.file]);
            }
            Ok(AppliedProjection {
                aliases,
                _held: held,
            })
        })();
        result
            .map_err(|e| RuntimeProjectionFailure::new(phase, e, started).with_operation(operation))
    }

    #[cfg(test)]
    mod observation_tests {
        use super::*;
        use std::{error::Error, process::Command, time::Duration};

        const CHILD: &str = "WORKCELL_READONLY_OBSERVATION_CHILD";
        const VIEW_OPERATIONS: [&str; 5] = [
            "readonly-input.view.open",
            "readonly-input.view.fstat",
            "readonly-input.view.form",
            "readonly-input.view.canonicalize",
            "readonly-input.view.route",
        ];
        const CASE: &str = "runtime_projection::linux::observation_tests::actual_readonly_bind_suboperation_retains_original_kernel_refusal";

        fn fixture() -> PathBuf {
            let base =
                PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../ProjectCentral/now/tmp");
            fs::create_dir_all(&base).unwrap();
            let base = base.canonicalize().unwrap();
            let root = base.join(format!(
                "readonly-operation-{}-{}",
                std::process::id(),
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap()
                    .as_nanos()
            ));
            fs::create_dir(&root).unwrap();
            // Retain actual controlled filesystem material; no sweeping failed
            // or old fixtures and no claim of native Project/Task authority.
            root
        }

        fn capture_case(mut command: Command, root: &Path) -> crate::BoundedProcessOutput {
            command
                .stdin(std::process::Stdio::null())
                .stdout(std::process::Stdio::piped())
                .stderr(std::process::Stdio::piped());
            match crate::capture_bounded_process(command, Duration::from_secs(10), 65_536) {
                Ok(output) => output,
                Err(failure) => {
                    fs::write(root.join("child.stdout"), failure.stdout()).unwrap();
                    fs::write(root.join("child.stderr"), failure.stderr()).unwrap();
                    fs::write(
                        root.join("child-capture-failure.json"),
                        failure.observation().to_string(),
                    )
                    .unwrap();
                    panic!("same native finite capture failed; actual evidence retained");
                }
            }
        }

        fn assert_native_cause(failure: &RuntimeProjectionFailure, errno: i32) {
            let source = failure
                .source()
                .unwrap()
                .downcast_ref::<io::Error>()
                .unwrap();
            assert!(std::ptr::eq(source, &failure.cause));
            assert_eq!(source.raw_os_error(), Some(errno));
            assert_eq!(failure.as_json()["cause"]["raw_os_error"], errno);
            assert_eq!(failure.as_json()["executed"], false);
            assert_eq!(failure.as_json()["automatic_retry"], false);
            assert_eq!(failure.as_json()["material_setup_started"], true);
        }

        #[test]
        fn actual_missing_view_observation_retains_original_io_and_legacy_json() {
            let root = fixture();
            let mut operation = None;
            let cause =
                Held::open_observed(&root.join("absent"), true, &mut operation, VIEW_OPERATIONS)
                    .err()
                    .expect("actual absent directory must refuse");
            let failure = RuntimeProjectionFailure::new("readonly-input", cause, true)
                .with_operation(operation);
            assert_native_cause(&failure, libc::ENOENT);
            let reading = failure.as_json();
            assert_eq!(reading["phase"], "readonly-input");
            assert_eq!(reading["operation"], "readonly-input.view.open");
            assert!(!reading.to_string().contains(root.to_str().unwrap()));
            let legacy_cause = fs::File::open(root.join("legacy-absent")).unwrap_err();
            let legacy = RuntimeProjectionFailure::new("readonly-input", legacy_cause, true);
            assert_native_cause(&legacy, libc::ENOENT);
            assert!(legacy.as_json().get("operation").is_none());
            assert!(legacy
                .to_string()
                .starts_with("runtime projection readonly-input: "));
            fs::write(root.join("failure.json"), reading.to_string()).unwrap();
        }

        #[test]
        fn actual_view_observation_retains_form_and_alias_refusal() {
            let root = fixture();
            let directory = root.join("input");
            fs::create_dir(&directory).unwrap();
            fs::write(directory.join("unchanged"), b"CONTROLLED_INPUT_UNCHANGED").unwrap();
            let mut operation = None;
            let admitted =
                Held::open_observed(&directory, true, &mut operation, VIEW_OPERATIONS).unwrap();
            admitted.check_requested(&directory).unwrap();
            let metadata = fs::metadata(&directory).unwrap();
            assert_eq!(
                (admitted.dev, admitted.ino),
                (metadata.dev(), metadata.ino())
            );
            assert_eq!(operation, Some("readonly-input.view.route"));
            let alias = root.join("alias");
            std::os::unix::fs::symlink(&directory, &alias).unwrap();
            let flags = libc::O_NOFOLLOW | libc::O_NONBLOCK | libc::O_CLOEXEC | libc::O_DIRECTORY;
            let direct = fs::OpenOptions::new()
                .read(true)
                .custom_flags(flags)
                .open(&alias)
                .unwrap_err();
            let errno = direct
                .raw_os_error()
                .expect("genuine native nofollow refusal");
            let cause = Held::open_observed(&alias, true, &mut operation, VIEW_OPERATIONS)
                .err()
                .expect("final alias must refuse");
            let failure = RuntimeProjectionFailure::new("readonly-input", cause, true)
                .with_operation(operation);
            assert_native_cause(&failure, errno);
            assert_eq!(failure.cause.kind(), direct.kind());
            assert_eq!(failure.as_json()["operation"], "readonly-input.view.open");
            let fifo = root.join("fifo");
            let native = c(&fifo).unwrap();
            assert_eq!(unsafe { libc::mkfifo(native.as_ptr(), 0o600) }, 0);
            let cause = Held::open_observed(&fifo, false, &mut operation, VIEW_OPERATIONS)
                .err()
                .expect("actual fifo must refuse without blocking");
            let failure = RuntimeProjectionFailure::new("readonly-input", cause, true)
                .with_operation(operation);
            assert_eq!(failure.cause.kind(), io::ErrorKind::InvalidInput);
            assert_eq!(failure.cause.raw_os_error(), None);
            assert_eq!(failure.as_json()["operation"], "readonly-input.view.form");
            assert_eq!(
                fs::read(directory.join("unchanged")).unwrap(),
                b"CONTROLLED_INPUT_UNCHANGED"
            );
            admitted.check_requested(&directory).unwrap();
            let current = admitted.mount_handle(&mut operation).unwrap();
            assert_eq!((current.dev, current.ino), (admitted.dev, admitted.ino));
            let retained = root.join("retained-original");
            fs::rename(&directory, &retained).unwrap();
            fs::create_dir(&directory).unwrap();
            let replacement = admitted
                .mount_handle(&mut operation)
                .err()
                .expect("replacement must not reaffiliate to another object");
            assert_eq!(replacement.kind(), io::ErrorKind::InvalidInput);
            assert_eq!(operation, Some("readonly-input.reaffiliate.named-before"));
            fs::remove_dir(&directory).unwrap();
            std::os::unix::fs::symlink(&retained, &directory).unwrap();
            assert_eq!(
                admitted.mount_handle(&mut operation).err().unwrap().kind(),
                io::ErrorKind::InvalidInput
            );
            fs::remove_file(&directory).unwrap();
            fs::rename(&retained, &directory).unwrap();
            admitted.check_requested(&directory).unwrap();
            assert_eq!(
                fs::read(directory.join("unchanged")).unwrap(),
                b"CONTROLLED_INPUT_UNCHANGED"
            );
            fs::write(
                root.join("form-refusal.json"),
                failure.as_json().to_string(),
            )
            .unwrap();
        }

        #[test]
        #[ignore = "requires real nonroot Linux user/mount namespace, no availability-to-success skip"]
        fn actual_readonly_bind_suboperation_retains_original_kernel_refusal() {
            assert_ne!(
                unsafe { libc::geteuid() },
                0,
                "actual unprivileged prerequisite"
            );
            if let Some(root) = std::env::var_os(CHILD) {
                let root = PathBuf::from(root);
                enter_namespace().expect("actual namespace prerequisite must succeed");
                let target = root.join("input");
                // Open in this actual mount namespace to isolate genuine target
                // absence, rather than a pre-unshare descriptor compatibility.
                let held = Held::open(&target, true).unwrap();
                fs::rename(&target, root.join("retained-input")).unwrap();
                let mut operation = None;
                let cause = readonly_bind_observed(
                    &held,
                    &mut operation,
                    [
                        "readonly-input.skeleton.bind",
                        "readonly-input.skeleton.remount",
                    ],
                )
                .expect_err("actual absent mount target must refuse");
                let failure = RuntimeProjectionFailure::new("readonly-input", cause, true)
                    .with_operation(operation);
                assert_native_cause(&failure, libc::ENOENT);
                assert_eq!(
                    failure.as_json()["operation"],
                    "readonly-input.skeleton.bind"
                );
                assert_eq!(
                    fs::read(root.join("retained-input/unchanged")).unwrap(),
                    b"CONTROLLED_INPUT_UNCHANGED"
                );
                fs::write(
                    root.join("kernel-refusal.json"),
                    failure.as_json().to_string(),
                )
                .unwrap();
                return;
            }
            let root = fixture();
            fs::create_dir(root.join("input")).unwrap();
            fs::write(root.join("input/unchanged"), b"CONTROLLED_INPUT_UNCHANGED").unwrap();
            let mut command = Command::new(std::env::current_exe().unwrap());
            command.args([CASE, "--exact", "--ignored", "--nocapture"]);
            command.env(CHILD, &root);
            let captured = capture_case(command, &root);
            fs::write(root.join("child.stdout"), &captured.stdout).unwrap();
            fs::write(root.join("child.stderr"), &captured.stderr).unwrap();
            fs::write(
                root.join("child-outcome.json"),
                json!({
                    "status":captured.status.code(),"timed_out":captured.timed_out,
                    "output_complete":captured.output_complete,
                    "output_truncated":captured.output_truncated
                })
                .to_string(),
            )
            .unwrap();
            assert!(!captured.timed_out && captured.output_complete && !captured.output_truncated);
            assert!(
                captured.status.success(),
                "genuine namespace/mount assertions must execute"
            );
            let reading: Value =
                serde_json::from_slice(&fs::read(root.join("kernel-refusal.json")).unwrap())
                    .unwrap();
            assert_eq!(reading["phase"], "readonly-input");
            assert_eq!(reading["operation"], "readonly-input.skeleton.bind");
            assert_eq!(reading["cause"]["raw_os_error"], libc::ENOENT);
            assert_eq!(reading["executed"], false);
            assert_eq!(
                fs::read(root.join("retained-input/unchanged")).unwrap(),
                b"CONTROLLED_INPUT_UNCHANGED"
            );
        }

        #[test]
        #[ignore = "requires real nonroot Linux namespace/readonly overlay; unsupported is a failure"]
        fn actual_current_namespace_mount_handles_preserve_original_input() {
            const CHILD_POSITIVE: &str = "WORKCELL_NAMESPACE_MOUNT_CHILD";
            const POSITIVE: &str = "runtime_projection::linux::observation_tests::actual_current_namespace_mount_handles_preserve_original_input";
            assert_ne!(
                unsafe { libc::geteuid() },
                0,
                "actual unprivileged prerequisite"
            );
            if let Some(root) = std::env::var_os(CHILD_POSITIVE) {
                let root = PathBuf::from(root);
                let input = Held::open(&root.join("input"), true).unwrap();
                let skeleton = Held::open(&root.join("skeleton"), true).unwrap();
                let empty = Held::open(&root.join("empty"), true).unwrap();
                enter_namespace().expect("actual namespace prerequisite must succeed");
                // Retain the real former-namespace outcome, without requiring
                // every supported kernel to reproduce Linux 7.2.3's old refusal.
                let old = readonly_bind(&skeleton);
                let old_reading = match old {
                    Ok(()) => json!({"ok":true,"operation":"preheld-bind-and-remount"}),
                    Err(cause) => {
                        assert_eq!(cause.raw_os_error(), Some(libc::EINVAL));
                        json!({"ok":false,"operation":"preheld-bind-and-remount",
                            "cause":{"kind":format!("{:?}",cause.kind()),
                            "raw_os_error":cause.raw_os_error(),"message":cause.to_string()}})
                    }
                };
                fs::write(root.join("preheld-outcome.json"), old_reading.to_string()).unwrap();
                let mut operation = None;
                let input_mount = input.mount_handle(&mut operation).unwrap();
                let skeleton_mount = skeleton.mount_handle(&mut operation).unwrap();
                let empty_mount = empty.mount_handle(&mut operation).unwrap();
                for (original, current) in [
                    (&input, &input_mount),
                    (&skeleton, &skeleton_mount),
                    (&empty, &empty_mount),
                ] {
                    assert_eq!((original.dev, original.ino), (current.dev, current.ino));
                }
                readonly_bind(&skeleton_mount).unwrap();
                readonly_bind(&empty_mount).unwrap();
                let options = format!(
                    "lowerdir={}:{},userxattr,redirect_dir=nofollow",
                    skeleton_mount.fd_path().display(),
                    input_mount.fd_path().display()
                );
                mount(
                    Some(Path::new("overlay")),
                    &input.path,
                    Some("overlay"),
                    libc::MS_RDONLY | libc::MS_NOSUID | libc::MS_NODEV,
                    Some(&options),
                )
                .unwrap();
                let refusal = fs::write(input.path.join("unchanged"), b"MUST_NOT_WRITE")
                    .expect_err("genuine readonly view must deny the actual write");
                assert_eq!(refusal.raw_os_error(), Some(libc::EROFS));
                assert_eq!(
                    fs::read(input.path.join("unchanged")).unwrap(),
                    b"CONTROLLED_INPUT_UNCHANGED"
                );
                let source_member =
                    open_at(&input, std::ffi::OsStr::new("unchanged"), false).unwrap();
                let original_file = fs::metadata(input.fd_path().join("unchanged")).unwrap();
                assert_eq!(
                    (source_member.dev, source_member.ino),
                    (original_file.dev(), original_file.ino())
                );
                named_basis(&input, "unchanged", Some(&source_member)).unwrap();
                fs::write(
                    root.join("current-namespace-outcome.json"),
                    json!({
                        "ok":true,"namespace_descriptor_identity_equal":true,
                        "readonly_bind_and_overlay_completed":true,
                        "actual_write_errno":refusal.raw_os_error(),
                        "original_input_unchanged":true,
                        "provider_executed":false
                    })
                    .to_string(),
                )
                .unwrap();
                return;
            }
            let root = fixture();
            for name in ["input", "skeleton", "empty"] {
                fs::create_dir(root.join(name)).unwrap();
            }
            fs::write(root.join("input/unchanged"), b"CONTROLLED_INPUT_UNCHANGED").unwrap();
            let mut command = Command::new(std::env::current_exe().unwrap());
            command.args([POSITIVE, "--exact", "--ignored", "--nocapture"]);
            command.env(CHILD_POSITIVE, &root);
            let captured = capture_case(command, &root);
            fs::write(root.join("child.stdout"), &captured.stdout).unwrap();
            fs::write(root.join("child.stderr"), &captured.stderr).unwrap();
            fs::write(
                root.join("child-outcome.json"),
                json!({
                    "status":captured.status.code(),"timed_out":captured.timed_out,
                    "output_complete":captured.output_complete,
                    "output_truncated":captured.output_truncated
                })
                .to_string(),
            )
            .unwrap();
            assert!(!captured.timed_out && captured.output_complete && !captured.output_truncated);
            assert!(
                captured.status.success(),
                "actual bind/overlay/readonly invariants must execute"
            );
            let reading: Value = serde_json::from_slice(
                &fs::read(root.join("current-namespace-outcome.json")).unwrap(),
            )
            .unwrap();
            assert_eq!(reading["ok"], true);
            assert_eq!(reading["readonly_bind_and_overlay_completed"], true);
            assert_eq!(reading["actual_write_errno"], libc::EROFS);
            assert_eq!(reading["provider_executed"], false);
            assert_eq!(
                fs::read(root.join("input/unchanged")).unwrap(),
                b"CONTROLLED_INPUT_UNCHANGED"
            );
        }
    }
}

pub(crate) fn apply(
    p: &RuntimeProjection,
    requirements: &WriteBoundaryRequirements,
) -> Result<AppliedProjection, RuntimeProjectionFailure> {
    p.validate_boundary(requirements)?;
    #[cfg(target_os = "linux")]
    {
        linux::apply(p, requirements)
    }
    #[cfg(not(target_os = "linux"))]
    {
        Err(RuntimeProjectionFailure::new(
            "platform",
            io::Error::new(
                io::ErrorKind::Unsupported,
                "no supported runtime projection provider on this platform",
            ),
            false,
        ))
    }
}
