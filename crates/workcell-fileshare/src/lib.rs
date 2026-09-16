//! SMB file-share provider: stages a tailnet-gated Samba share as service
//! material under the [`ServiceProvider`] port.
//!
//! Standing law (`Work/Workcell/docs/REMOTE-FILE-SHARING.md`): staging is not
//! installation. This provider writes the exact Samba config and a one-shot
//! installer into a staging root; installation happens on the target machine
//! under the operator's own sudo, outside this process. Access is gated by
//! Samba `hosts allow` (loopback + Tailscale ranges), never by interface
//! binding — a /32 point-to-point Tailscale peer falls back to loopback-only
//! binding with no error at start-up.

use std::{
    collections::BTreeMap,
    fs,
    path::{Path, PathBuf},
};

use epilogos_workcell_core::{
    validate_allocation, Availability, DemandRef, HealthState, OfferRef, OperationalOffer,
    ProviderAllocation, ProviderObservation, ProviderPort, ProviderPortKind, ProviderRef,
    ProviderReleaseResult, ReleaseDisposition, Result, RetentionExpectation,
    ServiceMaterialRequest, ServiceProvider, WorkcellError,
};

pub const SMB_SHARE_PROVIDER_REF: &str = "provider:fileshare-smb";

/// Loopback plus the Tailscale IPv4 and IPv6 ranges. Nothing else may talk to
/// the share: the gate is the point of the surface.
pub const DEFAULT_ALLOWED_RANGES: &[&str] =
    &["127.0.0.0/8", "100.64.0.0/10", "fd7a:115c:a1e0::/48"];

const RUNBOOK_REF: &str = "Work/Workcell/docs/REMOTE-FILE-SHARING.md";

/// Provider-native blueprint of one share. Demand stays neutral — the request
/// carries a semantic connection requirement such as `files:home`; the SMB
/// specifics live here, at the provider, where they are true.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SmbShareBlueprint {
    /// `homes` emits the standard per-user home share; any other name emits a
    /// path share for `server_path`.
    pub share_name: String,
    pub server_path: String,
    pub smb_user: String,
    pub allowed_ranges: Vec<String>,
}

impl SmbShareBlueprint {
    /// The proven shape: the user's home directory, tailnet-gated.
    pub fn homes(smb_user: impl Into<String>) -> Self {
        Self {
            share_name: "homes".into(),
            server_path: String::new(),
            smb_user: smb_user.into(),
            allowed_ranges: DEFAULT_ALLOWED_RANGES
                .iter()
                .map(|s| s.to_string())
                .collect(),
        }
    }

    fn validate(&self) -> Result<()> {
        let fail = |what: &str| {
            Err(WorkcellError::InvalidDemand(format!(
                "smb share blueprint {what} must be non-empty and free of control characters"
            )))
        };
        for (what, value) in [
            ("share_name", &self.share_name),
            ("smb_user", &self.smb_user),
        ] {
            if value.trim().is_empty() || value.chars().any(|c| c.is_control()) {
                return fail(what);
            }
        }
        // The homes share has no server path of its own; a path share must.
        let path_needed = self.share_name != "homes";
        if path_needed
            && (self.server_path.trim().is_empty()
                || self.server_path.chars().any(|c| c.is_control()))
        {
            return fail("server_path for a path share");
        }
        if self.allowed_ranges.is_empty() {
            return fail("allowed_ranges");
        }
        for range in &self.allowed_ranges {
            if range.trim().is_empty() || range.chars().any(|c| c.is_control()) {
                return fail("allowed_ranges entry");
            }
        }
        Ok(())
    }
}

/// Stages a tailnet-gated Samba share. The materialised surface is the staged
/// pair (`smb.conf.staged`, `enable-file-share.sh`) plus the truth about how
/// it may be installed; the running daemon belongs to the operator's machine.
pub struct SmbShareProvider {
    provider_ref: ProviderRef,
    staging_root: PathBuf,
    blueprint: SmbShareBlueprint,
    availability: Availability,
}

impl SmbShareProvider {
    pub fn new(staging_root: impl Into<PathBuf>, blueprint: SmbShareBlueprint) -> Result<Self> {
        blueprint.validate()?;
        let staging_root = staging_root.into();
        let availability = match fs::create_dir_all(&staging_root) {
            Ok(()) => Availability::Available,
            Err(_) => Availability::Unavailable,
        };
        Ok(Self {
            provider_ref: ProviderRef::new(SMB_SHARE_PROVIDER_REF)?,
            staging_root,
            blueprint,
            availability,
        })
    }

    pub fn blueprint(&self) -> &SmbShareBlueprint {
        &self.blueprint
    }

    fn slug(demand_ref: &DemandRef) -> String {
        demand_ref
            .as_str()
            .chars()
            .map(|c| {
                if c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '_') {
                    c
                } else {
                    '-'
                }
            })
            .collect()
    }

    fn staging_dir(&self, demand_ref: &DemandRef) -> PathBuf {
        self.staging_root.join(format!(
            "{}-{}",
            Self::slug(demand_ref),
            self.blueprint.share_name
        ))
    }

    fn global_block(&self) -> String {
        format!(
            "[global]\n   server role = standalone server\n   server min protocol = SMB2\n   \
             map to guest = never\n\n   # Listen everywhere; only loopback and the tailnet may talk.\n   \
             hosts allow = {ranges}\n   hosts deny = ALL\n\n   log file = /var/log/samba/%m.log\n",
            ranges = self.blueprint.allowed_ranges.join(" ")
        )
    }

    fn expected_config(&self) -> String {
        let share_block = if self.blueprint.share_name == "homes" {
            "   comment = Home on %h\n   browseable = no\n   valid users = %S\n   writable = yes\n"
                .to_string()
        } else {
            format!(
                "   comment = Staged by {}\n   path = {}\n   browseable = yes\n   valid users = {}\n   \
                 writable = yes\n",
                SMB_SHARE_PROVIDER_REF, self.blueprint.server_path, self.blueprint.smb_user
            )
        };
        format!(
            "{}\n[{}]\n{}",
            self.global_block(),
            self.blueprint.share_name,
            share_block
        )
    }

    fn expected_installer(&self, staging_dir: &Path) -> String {
        format!(
            "#!/bin/sh\n# One-shot installer staged by {provider}. Runbook: {runbook}.\n\
             # Needs root on the target machine; staging is not installation.\nset -e\n\
             install -o root -g root -m 0644 {dir}/smb.conf.staged /etc/samba/smb.conf\n\
             echo \"== Choose a password for SMB access as user '{user}' (independent of the login password) ==\"\n\
             smbpasswd -a {user}\nsystemctl enable --now smb.service\n\
             echo \"== port 445 (expect 0.0.0.0 or *; 127.0.0.1 alone means loopback-only) ==\"\n\
             ss -tln | grep ':445 ' || echo \"445 NOT LISTENING\"\n",
            provider = SMB_SHARE_PROVIDER_REF,
            runbook = RUNBOOK_REF,
            dir = staging_dir.display(),
            user = self.blueprint.smb_user,
        )
    }

    fn stage(&self, staging_dir: &Path, config: &str, installer: &str) -> Result<()> {
        fs::create_dir_all(staging_dir).map_err(|error| {
            WorkcellError::OperationFailed(format!("create staging dir: {error}"))
        })?;
        fs::write(staging_dir.join("smb.conf.staged"), config).map_err(|error| {
            WorkcellError::OperationFailed(format!("write staged config: {error}"))
        })?;
        fs::write(staging_dir.join("enable-file-share.sh"), installer).map_err(|error| {
            WorkcellError::OperationFailed(format!("write staged installer: {error}"))
        })?;
        Ok(())
    }

    fn observed_state(&self, staging_dir: &Path) -> (String, String, HealthState) {
        let config = fs::read_to_string(staging_dir.join("smb.conf.staged"));
        let installer = fs::read_to_string(staging_dir.join("enable-file-share.sh"));
        let config_state = match &config {
            Err(_) => "absent".to_string(),
            Ok(text) if *text == self.expected_config() => "ok".to_string(),
            Ok(_) => "drifted".to_string(),
        };
        let installer_state = match &installer {
            Err(_) => "absent".to_string(),
            Ok(text) if *text == self.expected_installer(staging_dir) => "ok".to_string(),
            Ok(_) => "drifted".to_string(),
        };
        let health = if config_state == "ok" && installer_state == "ok" {
            HealthState::Healthy
        } else {
            HealthState::Degraded
        };
        (config_state, installer_state, health)
    }
}

impl ProviderPort for SmbShareProvider {
    fn provider_ref(&self) -> &ProviderRef {
        &self.provider_ref
    }

    fn port_kind(&self) -> ProviderPortKind {
        ProviderPortKind::Service
    }

    fn offers(&self) -> Result<Vec<OperationalOffer>> {
        Ok(vec![OperationalOffer {
            offer_ref: OfferRef::new("offer:fileshare-smb/stage")
                .map_err(|error| WorkcellError::OperationFailed(error.into()))?,
            provider_ref: self.provider_ref.clone(),
            port: ProviderPortKind::Service.as_str().into(),
            affordances: vec!["smb-share-staging".into()],
            connections: vec!["files:tailnet".into()],
            exposures: Vec::new(),
            isolation_trust: Vec::new(),
            availability: self.availability.clone(),
            health: match self.availability {
                Availability::Available => HealthState::Healthy,
                Availability::Degraded => HealthState::Degraded,
                Availability::Unavailable => HealthState::Unavailable,
            },
            capacity: BTreeMap::new(),
            metadata: BTreeMap::from([
                (
                    "stages".into(),
                    "tailnet-gated smb.conf + one-shot installer".into(),
                ),
                (
                    "installation".into(),
                    "operator sudo on the target machine; staging is not installation".into(),
                ),
                ("runbook".into(), RUNBOOK_REF.into()),
            ]),
        }])
    }
}

impl ServiceProvider for SmbShareProvider {
    fn resolve_service(&mut self, request: &ServiceMaterialRequest) -> Result<ProviderAllocation> {
        if self.availability == Availability::Unavailable {
            return Err(WorkcellError::Unavailable(
                "smb share staging root is not creatable".into(),
            ));
        }
        let staging_dir = self.staging_dir(&request.demand_ref);
        let config = self.expected_config();
        let installer = self.expected_installer(&staging_dir);
        self.stage(&staging_dir, &config, &installer)?;
        let slug = Self::slug(&request.demand_ref);
        Ok(ProviderAllocation {
            provider_ref: self.provider_ref.clone(),
            port: ProviderPortKind::Service,
            material_ref: format!("smb-share:{}:{}", slug, self.blueprint.share_name),
            health: HealthState::Healthy,
            properties: BTreeMap::from([
                ("staging_dir".into(), staging_dir.display().to_string()),
                ("config_path".into(), staging_dir.join("smb.conf.staged").display().to_string()),
                (
                    "installer_path".into(),
                    staging_dir.join("enable-file-share.sh").display().to_string(),
                ),
                ("share".into(), self.blueprint.share_name.clone()),
                ("smb_user".into(), self.blueprint.smb_user.clone()),
                ("gate".into(), "hosts-allow".into()),
                ("allowed_ranges".into(), self.blueprint.allowed_ranges.join(",")),
                ("port".into(), "445".into()),
                (
                    "binding_expected".into(),
                    "0.0.0.0:445 or *:445 in ss -tln; 127.0.0.1:445 alone means loopback-only".into(),
                ),
            ]),
            provenance: BTreeMap::from([
                ("runbook".into(), RUNBOOK_REF.into()),
                (
                    "gate_model".into(),
                    "samba hosts allow (loopback + tailnet); interface binding unusable on a /32 peer"
                        .into(),
                ),
                ("connection_ref".into(), request.connection.as_str().into()),
            ]),
        })
    }

    fn observe_service(&self, allocation: &ProviderAllocation) -> Result<ProviderObservation> {
        validate_allocation(self, allocation)?;
        let staging_dir = allocation.properties.get("staging_dir").ok_or_else(|| {
            WorkcellError::OperationFailed(
                "smb share allocation carries no staging_dir property".into(),
            )
        })?;
        let staging_dir = PathBuf::from(staging_dir);
        let (config_state, installer_state, health) = self.observed_state(&staging_dir);
        Ok(ProviderObservation {
            provider_ref: self.provider_ref.clone(),
            material_ref: allocation.material_ref.clone(),
            health,
            detail: BTreeMap::from([
                ("config".into(), config_state),
                ("installer".into(), installer_state),
                ("share".into(), self.blueprint.share_name.clone()),
            ]),
        })
    }

    fn release_service(
        &mut self,
        allocation: &ProviderAllocation,
        retention: &RetentionExpectation,
    ) -> Result<ProviderReleaseResult> {
        validate_allocation(self, allocation)?;
        let staging_dir = allocation.properties.get("staging_dir").ok_or_else(|| {
            WorkcellError::OperationFailed(
                "smb share allocation carries no staging_dir property".into(),
            )
        })?;
        if *retention == RetentionExpectation::Preserve {
            return Ok(ProviderReleaseResult {
                provider_ref: self.provider_ref.clone(),
                material_ref: allocation.material_ref.clone(),
                disposition: ReleaseDisposition::Preserved,
                changed: false,
            });
        }
        // Staged material is regenerable by a later resolve; there is nothing
        // to suspend or snapshot, so every other expectation releases.
        let staging_dir = PathBuf::from(staging_dir);
        let changed = match fs::remove_dir_all(&staging_dir) {
            Ok(()) => true,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => false,
            Err(error) => {
                return Err(WorkcellError::CleanupFailed(format!(
                    "remove staged smb share: {error}"
                )))
            }
        };
        Ok(ProviderReleaseResult {
            provider_ref: self.provider_ref.clone(),
            material_ref: allocation.material_ref.clone(),
            disposition: ReleaseDisposition::Released,
            changed,
        })
    }
}
