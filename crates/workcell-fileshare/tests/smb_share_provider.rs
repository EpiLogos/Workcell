use std::{
    fs,
    path::{Path, PathBuf},
    sync::atomic::{AtomicU64, Ordering},
    time::{SystemTime, UNIX_EPOCH},
};

use epilogos_workcell_core::{
    validate_allocation, DemandRef, HealthState, LogicalConnectionRequirement, ProviderAllocation,
    ProviderPort, ProviderPortKind, ProviderRef, RetentionExpectation, ServiceMaterialRequest,
    ServiceProvider,
};
use epilogos_workcell_fileshare::{SmbShareBlueprint, SmbShareProvider, SMB_SHARE_PROVIDER_REF};
use epilogos_workcell_sdk::testkit::verify_provider_port;

fn temp_root(label: &str) -> PathBuf {
    // Tests in one binary share a pid; a loaded run can quantise two
    // same-instant calls onto one clock tick.
    static SEQ: AtomicU64 = AtomicU64::new(0);
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let seq = SEQ.fetch_add(1, Ordering::Relaxed);
    std::env::temp_dir().join(format!(
        "epilogos-workcell-fileshare-{label}-{}-{nonce}-{seq}",
        std::process::id()
    ))
}

fn request() -> ServiceMaterialRequest {
    ServiceMaterialRequest {
        demand_ref: DemandRef::new("demand:fileshare-home").unwrap(),
        connection: LogicalConnectionRequirement::new("files:home").unwrap(),
        persistence: None,
        retention: RetentionExpectation::Release,
    }
}

fn homes_provider(root: &Path) -> SmbShareProvider {
    SmbShareProvider::new(root, SmbShareBlueprint::homes("frank")).unwrap()
}

fn resolve_into(provider: &mut SmbShareProvider) -> ProviderAllocation {
    provider.resolve_service(&request()).unwrap()
}

#[test]
fn offer_is_truthful_and_conformant() {
    let root = temp_root("offers");
    let provider = homes_provider(&root);
    let report = verify_provider_port(&provider).unwrap();
    assert_eq!(report.provider_ref.as_str(), SMB_SHARE_PROVIDER_REF);
    assert_eq!(report.offer_count, 1);
    assert_eq!(report.available_offers, 1);
    assert_eq!(report.degraded_offers, 0);
    let offer = &provider.offers().unwrap()[0];
    assert_eq!(offer.port, "service");
    assert_eq!(
        offer.metadata.get("installation").unwrap(),
        "operator sudo on the target machine; staging is not installation"
    );
}

#[test]
fn resolve_stages_gated_config_and_installer_idempotently() {
    let root = temp_root("resolve");
    let mut provider = homes_provider(&root);
    let allocation = resolve_into(&mut provider);
    validate_allocation(&provider, &allocation).unwrap();

    let staging_dir = PathBuf::from(allocation.properties.get("staging_dir").unwrap());
    let config = fs::read_to_string(staging_dir.join("smb.conf.staged")).unwrap();
    assert!(config.contains("hosts allow = 127.0.0.0/8 100.64.0.0/10 fd7a:115c:a1e0::/48"));
    assert!(config.contains("hosts deny = ALL"));
    // The binding trap this provider exists to avoid must never reappear.
    assert!(!config.contains("bind interfaces only"));
    assert!(config.contains("[homes]"));

    let installer = fs::read_to_string(staging_dir.join("enable-file-share.sh")).unwrap();
    assert!(installer.contains("smbpasswd -a frank"));
    assert!(installer.contains("systemctl enable --now smb.service"));
    assert!(installer.contains("445 NOT LISTENING"));

    let repeated = resolve_into(&mut provider);
    assert_eq!(repeated.material_ref, allocation.material_ref);
    assert_eq!(allocation.properties.get("gate").unwrap(), "hosts-allow");
    assert!(allocation
        .properties
        .get("binding_expected")
        .unwrap()
        .contains("loopback-only"));
}

#[test]
fn observe_names_drift_and_absence_as_degraded() {
    let root = temp_root("observe");
    let mut provider = homes_provider(&root);
    let allocation = resolve_into(&mut provider);

    let observation = provider.observe_service(&allocation).unwrap();
    assert_eq!(observation.health, HealthState::Healthy);
    assert_eq!(observation.detail.get("config").unwrap(), "ok");
    assert_eq!(observation.detail.get("installer").unwrap(), "ok");

    let staging_dir = PathBuf::from(allocation.properties.get("staging_dir").unwrap());
    let config_path = staging_dir.join("smb.conf.staged");
    let original = fs::read_to_string(&config_path).unwrap();
    fs::write(&config_path, format!("{original}\n# tampered\n")).unwrap();
    let drifted = provider.observe_service(&allocation).unwrap();
    assert_eq!(drifted.health, HealthState::Degraded);
    assert_eq!(drifted.detail.get("config").unwrap(), "drifted");

    fs::remove_file(staging_dir.join("enable-file-share.sh")).unwrap();
    let absent = provider.observe_service(&allocation).unwrap();
    assert_eq!(absent.health, HealthState::Degraded);
    assert_eq!(absent.detail.get("installer").unwrap(), "absent");
}

#[test]
fn observe_refuses_foreign_allocations() {
    let root = temp_root("foreign");
    let mut provider = homes_provider(&root);
    let allocation = resolve_into(&mut provider);
    let foreign = ProviderAllocation {
        provider_ref: ProviderRef::new("provider:other").unwrap(),
        port: ProviderPortKind::Service,
        material_ref: allocation.material_ref.clone(),
        health: HealthState::Healthy,
        properties: allocation.properties.clone(),
        provenance: allocation.provenance.clone(),
    };
    assert!(provider.observe_service(&foreign).is_err());
}

#[test]
fn release_semantics_follow_retention() {
    let root = temp_root("release");
    let mut provider = homes_provider(&root);
    let allocation = resolve_into(&mut provider);
    let staging_dir = PathBuf::from(allocation.properties.get("staging_dir").unwrap());

    let preserved = provider
        .release_service(&allocation, &RetentionExpectation::Preserve)
        .unwrap();
    assert_eq!(
        preserved.disposition,
        epilogos_workcell_core::ReleaseDisposition::Preserved
    );
    assert!(!preserved.changed);
    assert!(staging_dir.exists());

    let released = provider
        .release_service(&allocation, &RetentionExpectation::Release)
        .unwrap();
    assert_eq!(
        released.disposition,
        epilogos_workcell_core::ReleaseDisposition::Released
    );
    assert!(released.changed);
    assert!(!staging_dir.exists());

    let again = provider
        .release_service(&allocation, &RetentionExpectation::Release)
        .unwrap();
    assert_eq!(
        again.disposition,
        epilogos_workcell_core::ReleaseDisposition::Released
    );
    assert!(!again.changed);
}

#[test]
fn blueprint_rejects_unsafe_fields_and_allows_path_shares() {
    let root = temp_root("blueprint");
    let mut broken = SmbShareBlueprint::homes("");
    assert!(SmbShareProvider::new(&root, broken.clone()).is_err());
    broken.smb_user = "frank".into();
    broken.share_name = "bad\nname".into();
    assert!(SmbShareProvider::new(&root, broken).is_err());

    let empty_path = SmbShareBlueprint {
        share_name: "media".into(),
        server_path: String::new(),
        smb_user: "frank".into(),
        allowed_ranges: vec!["127.0.0.0/8".into()],
    };
    assert!(SmbShareProvider::new(&root, empty_path).is_err());

    let media = SmbShareBlueprint {
        share_name: "media".into(),
        server_path: "/srv/media".into(),
        smb_user: "frank".into(),
        allowed_ranges: vec!["127.0.0.0/8".into()],
    };
    let mut provider = SmbShareProvider::new(&root, media).unwrap();
    let allocation = resolve_into(&mut provider);
    let staging_dir = PathBuf::from(allocation.properties.get("staging_dir").unwrap());
    let config = fs::read_to_string(staging_dir.join("smb.conf.staged")).unwrap();
    assert!(config.contains("[media]"));
    assert!(config.contains("path = /srv/media"));
    assert!(config.contains("valid users = frank"));
}
