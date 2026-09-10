//! Bounded material write protection, not an interpretation of placement law.
//! Central/authority owners supply the exact paths, revision, expiry and required
//! coverage. Unsupported coverage is refused rather than silently weakened.
use crate::material_path::MaterialPath;
use epilogos_workcell_core::{Result, WorkcellError};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::{
    path::PathBuf,
    process::Command,
    time::{SystemTime, UNIX_EPOCH},
};

pub const WRITE_BOUNDARY_SCHEMA: &str = "workcell.write-boundary/v1";
pub const WRITE_BOUNDARY_COVERAGE: &[&str] = &[
    "file-content",
    "file-creation",
    "file-removal",
    "rename-link",
    "truncate",
    "descendant-processes",
];
pub const WRITE_BOUNDARY_UNCOVERED: &[&str] = &[
    "metadata-chmod-chown-xattr-time",
    "read-confidentiality",
    "network-and-delegated-services",
    "preexisting-hardlink-aliases",
    "outside-processes",
    "live-policy-revocation",
    "privileged-workloads",
    "device-ioctl",
    "external-mount-or-rename-of-granted-objects",
];

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct WriteBoundaryRequirements {
    pub policy_ref: String,
    pub policy_revision: String,
    pub authority_ref: String,
    pub writable_paths: Vec<PathBuf>,
    pub protected_paths: Vec<PathBuf>,
    pub required_coverage: Vec<String>,
    pub expires_at_unix_ms: u64,
}
impl WriteBoundaryRequirements {
    pub fn from_json(raw: &str) -> Result<Self> {
        if raw.len() > 1_048_576 {
            return Err(invalid("write boundary exceeds 1 MiB"));
        }
        let value: Value =
            serde_json::from_str(raw).map_err(|e| invalid(&format!("write boundary JSON: {e}")))?;
        let object = value
            .as_object()
            .ok_or_else(|| invalid("write boundary must be an object"))?;
        const KEYS: &[&str] = &[
            "schema",
            "policy_ref",
            "policy_revision",
            "authority_ref",
            "writable_paths",
            "protected_paths",
            "required_coverage",
            "expires_at_unix_ms",
        ];
        if value["schema"] != WRITE_BOUNDARY_SCHEMA
            || object.keys().any(|k| !KEYS.contains(&k.as_str()))
        {
            return Err(invalid("unknown write boundary schema or field"));
        }
        let string = |key: &str| {
            value[key]
                .as_str()
                .filter(|s| !s.trim().is_empty())
                .map(str::to_owned)
                .ok_or_else(|| invalid(&format!("{key} must be a nonempty string")))
        };
        let strings = |key: &str| -> Result<Vec<String>> {
            let array = value[key]
                .as_array()
                .ok_or_else(|| invalid(&format!("{key} must be an array")))?;
            if array.len() > 64 {
                return Err(invalid("at most 64 paths/coverage entries per boundary"));
            }
            array
                .iter()
                .map(|v| {
                    v.as_str()
                        .filter(|s| !s.trim().is_empty())
                        .map(str::to_owned)
                        .ok_or_else(|| invalid("boundary arrays require nonempty strings"))
                })
                .collect()
        };
        let requirement = Self {
            policy_ref: string("policy_ref")?,
            policy_revision: string("policy_revision")?,
            authority_ref: string("authority_ref")?,
            writable_paths: strings("writable_paths")?
                .into_iter()
                .map(PathBuf::from)
                .collect(),
            protected_paths: strings("protected_paths")?
                .into_iter()
                .map(PathBuf::from)
                .collect(),
            required_coverage: strings("required_coverage")?,
            expires_at_unix_ms: value["expires_at_unix_ms"]
                .as_u64()
                .ok_or_else(|| invalid("expires_at_unix_ms is required"))?,
        };
        requirement.validate(&requirement.policy_revision)?;
        Ok(requirement)
    }
    pub fn as_json(&self) -> Value {
        json!({"schema": WRITE_BOUNDARY_SCHEMA, "policy_ref": self.policy_ref, "policy_revision": self.policy_revision,
            "authority_ref": self.authority_ref, "writable_paths": self.writable_paths, "protected_paths": self.protected_paths,
            "required_coverage": self.required_coverage, "expires_at_unix_ms": self.expires_at_unix_ms})
    }
    pub fn digest(&self) -> String {
        format!(
            "sha256:{:x}",
            Sha256::digest(self.as_json().to_string().as_bytes())
        )
    }
    pub fn validate(&self, current_policy_revision: &str) -> Result<()> {
        if self.policy_ref.trim().is_empty()
            || self.policy_revision.trim().is_empty()
            || self.authority_ref.trim().is_empty()
        {
            return Err(invalid(
                "external policy, revision and authority references are required",
            ));
        }
        if self.policy_revision != current_policy_revision {
            return Err(WorkcellError::OperationFailed(
                "placement policy revision changed; re-resolve before dispatch".into(),
            ));
        }
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_err(|e| invalid(&format!("clock: {e}")))?
            .as_millis();
        if now >= u128::from(self.expires_at_unix_ms) {
            return Err(WorkcellError::OperationFailed(
                "write boundary authority has expired".into(),
            ));
        }
        if self.writable_paths.len() > 64
            || self.protected_paths.len() > 64
            || self.required_coverage.len() > 64
        {
            return Err(invalid("at most 64 paths/coverage entries per boundary"));
        }
        for required in &self.required_coverage {
            if !WRITE_BOUNDARY_COVERAGE.contains(&required.as_str()) {
                return Err(WorkcellError::Unsupported(format!(
                    "required write coverage `{required}` is not enforced by this provider"
                )));
            }
        }
        Ok(())
    }
}
fn invalid(message: &str) -> WorkcellError {
    WorkcellError::InvalidDemand(message.into())
}

/// Capability inspection performs no writes and grants no execution authority.
pub fn write_boundary_capabilities() -> Value {
    let result = platform::probe();
    let (supported, abi, reason) = match result {
        Ok(abi) => (true, Some(abi), None),
        Err(e) => (false, None, Some(e.to_string())),
    };
    json!({"schema": "workcell.write-boundary-capabilities/v1", "provider": "linux-landlock",
        "supported": supported, "abi": abi, "reason": reason,
        "coverage": if supported { WRITE_BOUNDARY_COVERAGE } else { &[] }, "uncovered": WRITE_BOUNDARY_UNCOVERED,
        "scope": "unprivileged launched process and descendants; regular filesystem writes only",
        "stdio": "null input and pipe output; other inherited descriptors close-on-exec",
        "policy_authority": "supplied; Workcell does not recognise or interpret governance"})
}

/// A pinned ruleset. Preparation is not execution and cannot certify an installed
/// harness. Revalidate against the current owner revision immediately before use.
#[derive(Clone, Debug)]
pub struct PreparedWriteBoundary {
    requirements: WriteBoundaryRequirements,
    paths: Vec<MaterialPath>,
    protected: Vec<MaterialPath>,
    platform: platform::Ruleset,
}
impl PreparedWriteBoundary {
    pub fn prepare(
        requirements: WriteBoundaryRequirements,
        current_policy_revision: &str,
    ) -> Result<Self> {
        requirements.validate(current_policy_revision)?;
        let paths = requirements
            .writable_paths
            .iter()
            .map(|p| MaterialPath::directory(p))
            .collect::<Result<Vec<_>>>()?;
        let protected = requirements
            .protected_paths
            .iter()
            .map(|p| MaterialPath::directory(p))
            .collect::<Result<Vec<_>>>()?;
        for allowed in &paths {
            if allowed.canonical.parent().is_none()
                || protected
                    .iter()
                    .any(|p| p.canonical.starts_with(&allowed.canonical))
            {
                return Err(invalid(
                    "writable directory includes a protected directory or filesystem root",
                ));
            }
        }
        let platform = platform::Ruleset::prepare(&paths)?;
        Ok(Self {
            requirements,
            paths,
            protected,
            platform,
        })
    }
    pub fn inspect(&self, current_policy_revision: &str) -> Result<Value> {
        self.revalidate(current_policy_revision)?;
        Ok(
            json!({"schema": "workcell.prepared-write-boundary/v1", "state": "prepared-not-executed",
            "requirements_digest": self.requirements.digest(), "requirements": self.requirements.as_json(),
            "capabilities": write_boundary_capabilities(),
            "objects": self.paths.iter().map(|p| json!({"path": p.canonical, "identity": p.identity})).collect::<Vec<_>>() }),
        )
    }
    pub fn revalidate(&self, current_policy_revision: &str) -> Result<()> {
        self.requirements.validate(current_policy_revision)?;
        platform::probe()?;
        for path in self.paths.iter().chain(&self.protected) {
            path.validate()?;
        }
        Ok(())
    }
    pub fn requirements(&self) -> &WriteBoundaryRequirements {
        &self.requirements
    }
    /// Installs enforcement immediately before exec. A failed kernel application
    /// makes spawn fail before the user program runs. The caller must use the
    /// returned pipes for data; inherited writable file handles are not allowed.
    pub fn configure_command(
        &self,
        command: &mut Command,
        current_policy_revision: &str,
    ) -> Result<()> {
        self.revalidate(current_policy_revision)?;
        self.platform.configure(command)
    }
}

#[cfg(target_os = "linux")]
mod platform {
    use super::*;
    use std::{
        fs::{File, OpenOptions},
        io,
        os::{
            fd::{AsRawFd, FromRawFd},
            unix::{fs::OpenOptionsExt, process::CommandExt},
        },
        process::Stdio,
        sync::Arc,
    };
    // Stable Linux UAPI; see docs/CAW-MATERIAL-OPERATIONS.md for source and limits.
    const WRITES: u64 = (1 << 1) | (0x7ff << 4); // write, remove/create, refer, truncate
    #[repr(C)]
    struct RulesetAttr {
        handled_access_fs: u64,
    }
    #[repr(C, packed)]
    struct PathBeneath {
        allowed_access: u64,
        parent_fd: i32,
    }
    pub(super) fn probe() -> Result<i32> {
        // Deliberately do not award the same coverage to root/capable workloads.
        if unsafe { libc::geteuid() } == 0 {
            return Err(WorkcellError::Unsupported(
                "Landlock write adapter requires an unprivileged process".into(),
            ));
        }
        let status = std::fs::read_to_string("/proc/self/status").map_err(|e| {
            WorkcellError::Unsupported(format!("cannot inspect process privilege: {e}"))
        })?;
        for name in ["CapEff:", "CapPrm:", "CapAmb:"] {
            let bits = status
                .lines()
                .find_map(|l| l.strip_prefix(name))
                .ok_or_else(|| {
                    WorkcellError::Unsupported("cannot verify process capability state".into())
                })?;
            if u64::from_str_radix(bits.trim(), 16)
                .map_err(|_| invalid("invalid process capability state"))?
                != 0
            {
                return Err(WorkcellError::Unsupported(
                    "Landlock write adapter refuses workloads with process capabilities".into(),
                ));
            }
        }
        let abi = unsafe {
            libc::syscall(
                libc::SYS_landlock_create_ruleset,
                std::ptr::null::<u8>(),
                0usize,
                1u32,
            )
        };
        if abi < 3 {
            return Err(WorkcellError::Unsupported(format!(
                "Linux Landlock ABI >= 3 is required (observed {abi}); no weaker fallback"
            )));
        }
        Ok(abi as i32)
    }
    #[derive(Clone, Debug)]
    pub(super) struct Ruleset {
        fd: Arc<File>,
        _paths: Vec<Arc<File>>,
    }
    impl Ruleset {
        pub(super) fn prepare(paths: &[MaterialPath]) -> Result<Self> {
            probe()?;
            let attr = RulesetAttr {
                handled_access_fs: WRITES,
            };
            let raw = unsafe {
                libc::syscall(
                    libc::SYS_landlock_create_ruleset,
                    &attr as *const RulesetAttr,
                    std::mem::size_of::<RulesetAttr>(),
                    0u32,
                )
            };
            if raw < 0 {
                return Err(WorkcellError::Unsupported(format!(
                    "create Landlock ruleset: {}",
                    io::Error::last_os_error()
                )));
            }
            // SAFETY: a successful syscall returned a new owned descriptor.
            let fd = unsafe { File::from_raw_fd(raw as i32) };
            let mut pinned = Vec::new();
            for path in paths {
                let file = OpenOptions::new()
                    .read(true)
                    .custom_flags(libc::O_PATH | libc::O_CLOEXEC | libc::O_DIRECTORY)
                    .open(&path.canonical)
                    .map_err(|e| {
                        WorkcellError::Unavailable(format!("pin writable directory: {e}"))
                    })?;
                if crate::material_path::object_identity(
                    &file
                        .metadata()
                        .map_err(|e| invalid(&format!("inspect pinned directory: {e}")))?,
                )? != path.identity
                {
                    return Err(WorkcellError::OperationFailed(
                        "directory changed while preparing boundary".into(),
                    ));
                }
                let rule = PathBeneath {
                    allowed_access: WRITES,
                    parent_fd: file.as_raw_fd(),
                };
                if unsafe {
                    libc::syscall(
                        libc::SYS_landlock_add_rule,
                        fd.as_raw_fd(),
                        1u32,
                        &rule as *const PathBeneath,
                        0u32,
                    )
                } < 0
                {
                    return Err(WorkcellError::Unsupported(format!(
                        "add Landlock path rule: {}",
                        io::Error::last_os_error()
                    )));
                }
                pinned.push(Arc::new(file));
            }
            Ok(Self {
                fd: Arc::new(fd),
                _paths: pinned,
            })
        }
        pub(super) fn configure(&self, command: &mut Command) -> Result<()> {
            let fd = Arc::clone(&self.fd);
            command
                .stdin(Stdio::null())
                .stdout(Stdio::piped())
                .stderr(Stdio::piped());
            // SAFETY: only async-signal-safe Linux syscalls and errno extraction
            // run after fork; all rules/files/allocations are prepared in parent.
            unsafe {
                command.pre_exec(move || {
                    if libc::prctl(libc::PR_SET_NO_NEW_PRIVS, 1, 0, 0, 0) != 0 {
                        return Err(io::Error::last_os_error());
                    }
                    // close-on-exec, not close-now: keep Rust's exec-error pipe
                    // and the ruleset usable until exec completes.
                    if libc::syscall(libc::SYS_close_range, 3u32, u32::MAX, 4u32) < 0 {
                        return Err(io::Error::last_os_error());
                    }
                    if libc::syscall(libc::SYS_landlock_restrict_self, fd.as_raw_fd(), 0u32) < 0 {
                        return Err(io::Error::last_os_error());
                    }
                    Ok(())
                });
            }
            Ok(())
        }
    }
}
#[cfg(not(target_os = "linux"))]
mod platform {
    use super::*;
    pub(super) fn probe() -> Result<i32> {
        Err(WorkcellError::Unsupported(
            "no material write adapter implemented for this OS; required protection is refused"
                .into(),
        ))
    }
    #[derive(Clone, Debug)]
    pub(super) struct Ruleset;
    impl Ruleset {
        pub(super) fn prepare(_: &[MaterialPath]) -> Result<Self> {
            probe()?;
            Ok(Self)
        }
        pub(super) fn configure(&self, _: &mut Command) -> Result<()> {
            probe()?;
            Ok(())
        }
    }
}
