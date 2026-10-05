use epilogos_workcell_core::WorkcellRef;
use epilogos_workcell_runtime::{
    build_instance_record, InstanceObservation, InstanceRegistry, LivenessUpdate, RegisterOutcome,
    EVIDENCE_DECLARED_UNVERIFIED, LIVENESS_LIVE, LIVENESS_STALE,
};
use serde_json::{json, Value};
use std::{
    fs,
    path::PathBuf,
    sync::{Arc, Barrier},
};

struct Fixture(PathBuf);
impl Fixture {
    fn new() -> Self {
        use std::sync::atomic::{AtomicU64, Ordering};
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../../ProjectCentral/now/tmp")
            .join(format!(
                "instance-publication-{}-{}-{}",
                std::process::id(),
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap()
                    .as_nanos(),
                NEXT.fetch_add(1, Ordering::Relaxed)
            ));
        fs::create_dir_all(&path).unwrap();
        Self(path)
    }
    fn registry(&self) -> InstanceRegistry {
        InstanceRegistry::new(
            &self.0,
            WorkcellRef::new("workcell:publication-proof").unwrap(),
        )
    }
    fn path(&self) -> PathBuf {
        self.0.join("instances/registry.json")
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        fs::remove_dir_all(&self.0).unwrap();
    }
}
fn record(slug: &str) -> Value {
    build_instance_record(
        &WorkcellRef::new("workcell:publication-proof").unwrap(),
        &InstanceObservation {
            slug: slug.into(),
            executable: PathBuf::new(),
            executable_sha256: String::new(),
            identity_material: format!("declared:{slug}"),
            pids: Vec::new(),
            executions: Vec::new(),
            evidence_grade: EVIDENCE_DECLARED_UNVERIFIED.into(),
            seams: Vec::new(),
        },
    )
}

fn live_record(slug: &str) -> Value {
    use sha2::{Digest, Sha256};
    use std::io::Read;
    let executable = std::env::current_exe().unwrap();
    let mut file = fs::File::open(&executable).unwrap();
    let mut hasher = Sha256::new();
    let mut buffer = [0u8; 8192];
    loop {
        let read = file.read(&mut buffer).unwrap();
        if read == 0 {
            break;
        }
        hasher.update(&buffer[..read]);
    }
    let sha = hasher
        .finalize()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect();
    // This test process is the actual executable and live PID. As with the
    // public manual-registration route, an absent start marker stays an
    // explicit evidence gap rather than an invented process generation.
    build_instance_record(
        &WorkcellRef::new("workcell:publication-proof").unwrap(),
        &InstanceObservation {
            slug: slug.into(),
            executable: executable.clone(),
            executable_sha256: sha,
            identity_material: executable.display().to_string(),
            pids: vec![std::process::id()],
            executions: Vec::new(),
            evidence_grade: epilogos_workcell_runtime::EVIDENCE_LIVE_PID.into(),
            seams: Vec::new(),
        },
    )
}

#[test]
fn concurrent_real_owner_declarations_preserve_every_acknowledged_identity() {
    let fixture = Fixture::new();
    fixture
        .registry()
        .declare("retained", None, "declared:retained")
        .unwrap();
    let mut seeded = fixture.registry().load().unwrap();
    seeded["owner_extension"] = json!({"retained":true});
    fs::write(fixture.path(), serde_json::to_vec(&seeded).unwrap()).unwrap();
    let barrier = Arc::new(Barrier::new(12));
    let workers: Vec<_> = (0..12)
        .map(|index| {
            let registry = fixture.registry();
            let barrier = barrier.clone();
            std::thread::spawn(move || {
                barrier.wait();
                registry.declare(
                    &format!("lane-{index}"),
                    None,
                    &format!("declared:lane-{index}"),
                )
            })
        })
        .collect();
    for worker in workers {
        assert_eq!(worker.join().unwrap().unwrap(), RegisterOutcome::Registered);
    }
    let reloaded = fixture.registry().load().unwrap();
    assert_eq!(reloaded["owner_extension"], seeded["owner_extension"]);
    assert_eq!(reloaded["instances"].as_object().unwrap().len(), 13);
    for index in 0..12 {
        assert!(fixture
            .registry()
            .list()
            .unwrap()
            .iter()
            .any(|r| r["harness_ref"] == format!("harness/lane-{index}")));
    }
}

#[test]
fn stale_liveness_basis_refuses_all_updates_and_retains_newer_owner_state() {
    let fixture = Fixture::new();
    let registry = fixture.registry();
    let first = live_record("first");
    let second = live_record("second");
    registry.register(first.clone()).unwrap();
    registry.register(second.clone()).unwrap();
    let first_ref = first["instance_ref"].as_str().unwrap();
    let second_ref = second["instance_ref"].as_str().unwrap();
    let first_basis = registry.show(first_ref).unwrap();
    let second_basis = registry.show(second_ref).unwrap();
    let mut newer = second_basis.clone();
    // A newly observed, actually present material seam makes the second
    // registration newer without inventing a timestamp or another process.
    let material = fixture.0.join("new-seam");
    fs::create_dir(&material).unwrap();
    newer["seams"] =
        json!([{"kind":"publication-proof","path":material.display().to_string(),"exists":true}]);
    registry.register(newer).unwrap();
    let acknowledged = fs::read(fixture.path()).unwrap();
    let updates = [
        LivenessUpdate {
            previous_record: first_basis,
            consecutive_misses: 3,
            liveness: LIVENESS_STALE.into(),
        },
        LivenessUpdate {
            previous_record: second_basis,
            consecutive_misses: 3,
            liveness: LIVENESS_STALE.into(),
        },
    ];
    let error = registry.apply_liveness(&updates).unwrap_err().to_string();
    assert!(error.contains("stale instance liveness basis"), "{error}");
    assert_eq!(fs::read(fixture.path()).unwrap(), acknowledged);
    assert_eq!(registry.show(first_ref).unwrap()["consecutive_misses"], 0);
    assert_eq!(
        registry.show(second_ref).unwrap()["seams"][0]["path"],
        material.display().to_string()
    );
    let fresh = registry.show(first_ref).unwrap();
    registry
        .apply_liveness(&[LivenessUpdate {
            previous_record: fresh,
            consecutive_misses: 3,
            liveness: LIVENESS_STALE.into(),
        }])
        .unwrap();
    assert_eq!(
        registry.show(first_ref).unwrap()["liveness"],
        LIVENESS_STALE
    );
}

#[test]
fn unchanged_liveness_and_reregistration_preserve_exact_source_bytes() {
    let fixture = Fixture::new();
    let registry = fixture.registry();
    let first = record("retained");
    registry.register(first.clone()).unwrap();
    let mut file = registry.load().unwrap();
    file["owner_extension"] = json!({"private":true});
    fs::write(fixture.path(), serde_json::to_vec(&file).unwrap()).unwrap();
    let before = fs::read(fixture.path()).unwrap();
    let metadata = fs::metadata(fixture.path()).unwrap();
    assert_eq!(
        registry.register(first.clone()).unwrap(),
        RegisterOutcome::Unchanged
    );
    registry
        .apply_liveness(&[LivenessUpdate {
            previous_record: first,
            consecutive_misses: 0,
            liveness: LIVENESS_STALE.into(),
        }])
        .unwrap();
    assert_eq!(fs::read(fixture.path()).unwrap(), before);
    assert_eq!(
        fs::metadata(fixture.path()).unwrap().modified().unwrap(),
        metadata.modified().unwrap()
    );
}

#[test]
fn invalid_or_duplicate_liveness_updates_refuse_before_effect() {
    let fixture = Fixture::new();
    let registry = fixture.registry();
    let first = record("retained");
    registry.register(first.clone()).unwrap();
    let before = fs::read(fixture.path()).unwrap();
    assert!(registry
        .apply_liveness(&[LivenessUpdate {
            previous_record: first.clone(),
            consecutive_misses: 0,
            liveness: "invented".into()
        }])
        .is_err());
    let update = LivenessUpdate {
        previous_record: first,
        consecutive_misses: 0,
        liveness: LIVENESS_LIVE.into(),
    };
    assert!(registry.apply_liveness(&[update.clone(), update]).is_err());
    assert_eq!(fs::read(fixture.path()).unwrap(), before);
}

#[test]
fn adoption_keeps_acknowledged_extension_fields_and_native_lineage() {
    let fixture = Fixture::new();
    let registry = fixture.registry();
    registry
        .declare("adopted", None, "declared:adopted")
        .unwrap();
    let declared = registry.list().unwrap().pop().unwrap();
    let declared_ref = declared["instance_ref"].as_str().unwrap().to_owned();
    let mut file = registry.load().unwrap();
    file["instances"][&declared_ref]["owner_extension"] = json!({"retained":true});
    fs::write(fixture.path(), serde_json::to_vec(&file).unwrap()).unwrap();
    let observed = live_record("adopted");
    let observed_ref = observed["instance_ref"].as_str().unwrap().to_owned();
    assert_eq!(
        registry.adopt(observed, &declared_ref).unwrap(),
        RegisterOutcome::Registered
    );
    assert!(registry.show(&declared_ref).is_err());
    let adopted = registry.show(&observed_ref).unwrap();
    assert_eq!(adopted["owner_extension"], json!({"retained":true}));
    assert_eq!(adopted["lineage"]["adopted_from"], declared_ref);
    assert_eq!(adopted["pids"], json!([std::process::id()]));
}

#[test]
fn occupied_lock_refuses_within_bound_and_release_restores_native_writer() {
    use std::os::unix::io::AsRawFd;
    let fixture = Fixture::new();
    let registry = fixture.registry();
    registry
        .declare("retained", None, "declared:retained")
        .unwrap();
    let lock = fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open(
            fixture
                .path()
                .with_file_name(".registry.json.publication.lock"),
        )
        .unwrap();
    assert_eq!(unsafe { libc::flock(lock.as_raw_fd(), libc::LOCK_EX) }, 0);
    let before = fs::read(fixture.path()).unwrap();
    let started = std::time::Instant::now();
    let error = registry
        .declare("new", None, "declared:new")
        .unwrap_err()
        .to_string();
    assert!(error.contains("five seconds"), "{error}");
    assert!(started.elapsed() < std::time::Duration::from_secs(7));
    assert_eq!(fs::read(fixture.path()).unwrap(), before);
    drop(lock);
    registry.declare("new", None, "declared:new").unwrap();
    assert_eq!(registry.list().unwrap().len(), 2);
}

#[test]
fn source_aliases_are_refused_and_legacy_staging_is_retained() {
    let fixture = Fixture::new();
    let registry = fixture.registry();
    registry
        .declare("retained", None, "declared:retained")
        .unwrap();
    let path = fixture.path();
    let retained = path.with_file_name("retained.json");
    let legacy = path.with_file_name("registry.json.tmp");
    fs::write(&legacy, b"uncommitted legacy evidence").unwrap();
    registry.declare("new", None, "declared:new").unwrap();
    assert_eq!(fs::read(&legacy).unwrap(), b"uncommitted legacy evidence");
    let before = fs::read(&path).unwrap();
    fs::rename(&path, &retained).unwrap();
    std::os::unix::fs::symlink(&retained, &path).unwrap();
    assert!(registry
        .declare("redirected", None, "declared:redirected")
        .is_err());
    assert!(registry.load().is_err());
    assert_eq!(fs::read(&retained).unwrap(), before);
    fs::remove_file(&path).unwrap();
    fs::hard_link(&retained, &path).unwrap();
    assert!(registry
        .declare("redirected", None, "declared:redirected")
        .is_err());
    assert_eq!(fs::read(&retained).unwrap(), before);
}

#[cfg(target_os = "macos")]
#[test]
fn registry_mutation_preserves_actual_acl_xattr_owner_and_mode() {
    use std::{
        os::unix::fs::{MetadataExt, PermissionsExt},
        process::Command,
    };
    let fixture = Fixture::new();
    let registry = fixture.registry();
    registry
        .declare("retained", None, "declared:retained")
        .unwrap();
    let path = fixture.path();
    fs::set_permissions(&path, fs::Permissions::from_mode(0o640)).unwrap();
    assert!(Command::new("/usr/bin/xattr")
        .args(["-w", "com.workcell.publication-test", "retained"])
        .arg(&path)
        .status()
        .unwrap()
        .success());
    assert!(Command::new("/bin/chmod")
        .args(["+a", "everyone allow read"])
        .arg(&path)
        .status()
        .unwrap()
        .success());
    fn acl(path: &std::path::Path) -> Vec<String> {
        let output = Command::new("/bin/ls")
            .arg("-lde")
            .arg(path)
            .output()
            .unwrap();
        assert!(output.status.success());
        String::from_utf8(output.stdout)
            .unwrap()
            .lines()
            .skip(1)
            .map(str::to_owned)
            .collect()
    }
    let before = fs::metadata(&path).unwrap();
    let before_acl = acl(&path);
    assert!(!before_acl.is_empty());
    registry.declare("new", None, "declared:new").unwrap();
    let after = fs::metadata(&path).unwrap();
    assert_eq!(
        (after.uid(), after.gid(), after.mode()),
        (before.uid(), before.gid(), before.mode())
    );
    assert_eq!(acl(&path), before_acl);
    let attr = Command::new("/usr/bin/xattr")
        .args(["-p", "com.workcell.publication-test"])
        .arg(&path)
        .output()
        .unwrap();
    assert!(attr.status.success());
    assert_eq!(String::from_utf8(attr.stdout).unwrap().trim(), "retained");
}

#[test]
fn exact_identity_material_survives_declare_register_restart_and_adoption() {
    let fixture = Fixture::new();
    let exact = "  declared origin\nsource:material-proof  ";
    fixture
        .registry()
        .declare("provenance", None, exact)
        .unwrap();
    let expected_ref = format!(
        "instance:provenance:{}",
        epilogos_workcell_runtime::identity_hash("", exact)
    );
    let restarted = fixture.registry();
    let declared = restarted.show(&expected_ref).unwrap();
    assert_eq!(declared["identity_material"], exact);
    assert_eq!(
        restarted.register(declared.clone()).unwrap(),
        RegisterOutcome::Unchanged
    );
    assert_eq!(
        fixture.registry().show(&expected_ref).unwrap()["identity_material"],
        exact
    );

    let observed = live_record("provenance");
    let observed_ref = observed["instance_ref"].as_str().unwrap().to_owned();
    let observed_material = observed["identity_material"].clone();
    restarted.adopt(observed, &expected_ref).unwrap();
    let adopted = fixture.registry().show(&observed_ref).unwrap();
    assert_eq!(adopted["identity_material"], observed_material);
    assert_eq!(adopted["lineage"]["adopted_from"], expected_ref);
    assert_eq!(adopted["lineage"]["declared_identity_material"], exact);
}

#[test]
fn legacy_missing_or_null_material_is_not_reconstructed_or_reidentified() {
    for missing in [true, false] {
        let fixture = Fixture::new();
        let registry = fixture.registry();
        registry
            .declare("legacy", None, "legacy-original-material")
            .unwrap();
        let mut legacy = registry.list().unwrap().pop().unwrap();
        let legacy_ref = legacy["instance_ref"].as_str().unwrap().to_owned();
        if missing {
            legacy.as_object_mut().unwrap().remove("identity_material");
        } else {
            legacy["identity_material"] = Value::Null;
        }
        let mut file = registry.load().unwrap();
        file["instances"][&legacy_ref] = legacy.clone();
        fs::write(fixture.path(), serde_json::to_vec(&file).unwrap()).unwrap();
        let before = fs::read(fixture.path()).unwrap();
        assert_eq!(
            fixture.registry().register(legacy.clone()).unwrap(),
            RegisterOutcome::Unchanged
        );
        assert_eq!(fs::read(fixture.path()).unwrap(), before);
        assert_eq!(fixture.registry().show(&legacy_ref).unwrap(), legacy);
        let observed = live_record("legacy");
        let observed_ref = observed["instance_ref"].as_str().unwrap().to_owned();
        registry.adopt(observed, &legacy_ref).unwrap();
        let adopted = fixture.registry().show(&observed_ref).unwrap();
        assert_eq!(adopted["lineage"]["adopted_from"], legacy_ref);
        assert!(adopted["lineage"]["declared_identity_material"].is_null());
        assert_eq!(
            adopted["identity_material"],
            std::env::current_exe().unwrap().display().to_string()
        );
    }
}

#[cfg(target_os = "linux")]
#[test]
fn registry_mutation_retains_real_linux_descriptor_xattr_bytes() {
    use std::{ffi::CString, fs::File, os::unix::io::AsRawFd};
    let fixture = Fixture::new();
    let registry = fixture.registry();
    registry
        .declare("retained", None, "declared:retained")
        .unwrap();
    let path = fixture.path();
    let attribute = CString::new("user.native-publication-test").unwrap();
    let value = b"retained\0source";
    let file = File::open(&path).unwrap();
    assert_eq!(
        unsafe {
            libc::fsetxattr(
                file.as_raw_fd(),
                attribute.as_ptr(),
                value.as_ptr().cast(),
                value.len(),
                0,
            )
        },
        0
    );
    drop(file);
    registry.declare("new", None, "declared:new").unwrap();
    let file = File::open(&path).unwrap();
    let mut retained = [0u8; 64];
    let read = unsafe {
        libc::fgetxattr(
            file.as_raw_fd(),
            attribute.as_ptr(),
            retained.as_mut_ptr().cast(),
            retained.len(),
        )
    };
    assert_eq!(read, value.len() as libc::ssize_t);
    assert_eq!(&retained[..read as usize], value);
}
