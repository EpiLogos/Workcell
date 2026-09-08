//! workcell-keychain: macOS Keychain as a Workcell secret source.
//!
//! Implements the existing `SecretProvider` contract from
//! `epilogos_workcell_core` over the native Security framework — no
//! shell-out to the `security` CLI. Credential refs use the
//! `central.security/v1` ref scheme: `keychain://<service>/<account>`.
//!
//! Two deliberate boundaries:
//!   * `resolve` reads only. Materialisation classes, receipts and output
//!     redaction stay with workcell-runtime — this crate never prints
//!     material and inherits `SecretValue`'s redacted Debug.
//!   * Store/remove are bootstrap operations, kept off the `SecretProvider`
//!     trait on purpose: the trait is the consumer seam, and consumers must
//!     not be able to write the vault. ACL policy is applied at store time
//!     (`KeychainAclPolicy`) and documented in `acl_policy()`.
//!
//! Portability: on macOS this hits the real Keychain via
//! `security-framework`. Everywhere else `resolve` returns
//! `WorkcellError::Unavailable` — a declared-source refusal, never a silent
//! empty result, mirroring the detection law that "could not run" never
//! collapses into "ran and found nothing".

use epilogos_workcell_core::{
    ExternalRef, ProviderRef, ProviderSecretMaterial, Result, SecretProvider,
    SecretRevocationState, SecretValue, WorkcellError,
};

pub const KEYCHAIN_PROVIDER_REF: &str = "secret-provider:keychain/macos";
pub const KEYCHAIN_REF_SCHEME: &str = "keychain://";
pub const KEYCHAIN_SOURCE_REVISION: &str = "central-security-map-T3";

/// One keychain credential ref, parsed and validated from the
/// `keychain://<service>/<account>` scheme. Refs are location only — the
/// value never appears here.
#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct KeychainCredentialRef {
    service: String,
    account: String,
}

impl KeychainCredentialRef {
    pub fn new(service: impl Into<String>, account: impl Into<String>) -> Result<Self> {
        let service = service.into();
        let account = account.into();
        for (label, value) in [("service", &service), ("account", &account)] {
            if value.trim().is_empty() {
                return Err(WorkcellError::InvalidDemand(format!(
                    "keychain credential {label} must not be empty"
                )));
            }
            if value
                .chars()
                .any(|c| c.is_whitespace() || c == '/' || c == '\\')
            {
                return Err(WorkcellError::InvalidDemand(format!(
                    "keychain credential {label} must not contain whitespace or path separators"
                )));
            }
        }
        Ok(Self { service, account })
    }

    pub fn parse(value: &str) -> Result<Self> {
        let rest = value.strip_prefix(KEYCHAIN_REF_SCHEME).ok_or_else(|| {
            WorkcellError::InvalidDemand(format!(
                "keychain credential ref must start with {KEYCHAIN_REF_SCHEME}"
            ))
        })?;
        let (service, account) = rest.split_once('/').ok_or_else(|| {
            WorkcellError::InvalidDemand(
                "keychain credential ref must be keychain://<service>/<account>".into(),
            )
        })?;
        if account.contains('/') {
            return Err(WorkcellError::InvalidDemand(
                "keychain credential ref takes exactly one service and one account".into(),
            ));
        }
        Self::new(service, account)
    }

    pub fn service(&self) -> &str {
        &self.service
    }

    pub fn account(&self) -> &str {
        &self.account
    }
}

/// ACL policy applied when bootstrap material is stored. The policy is a
/// declaration enforced by the native access-control object on the item;
/// `resolve` honours whatever the Keychain enforces at read time.
#[derive(Clone, Copy, Debug, Default, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub enum KeychainAclPolicy {
    /// Item readable while the device is unlocked, this device only. The
    /// default for agent bootstrap material: no cloud sync, no backup.
    #[default]
    ThisDeviceUnlocked,
    /// Additionally requires user presence (Touch ID / passcode) at access
    /// time. For material whose consumption should be human-gated.
    UserPresenceRequired,
}

impl KeychainAclPolicy {
    /// Human-readable policy statement, kept beside the code that enforces
    /// it so the ACL story is reviewable in one place.
    pub fn acl_policy(&self) -> &'static str {
        match self {
            Self::ThisDeviceUnlocked => {
                "AccessibleWhenUnlockedThisDeviceOnly; no access-control constraint; no sync"
            }
            Self::UserPresenceRequired => {
                "AccessibleWhenUnlockedThisDeviceOnly + kSecAccessControlUserPresence; no sync"
            }
        }
    }
}

/// macOS Keychain-backed `SecretProvider`. Reads only through the trait;
/// writes go through `store_bootstrap_material`, which is deliberately not
/// on the trait.
#[derive(Clone, Debug)]
pub struct KeychainSecretProvider {
    provider_ref: ProviderRef,
    acl: KeychainAclPolicy,
}

impl KeychainSecretProvider {
    pub fn new(acl: KeychainAclPolicy) -> Result<Self> {
        Ok(Self {
            provider_ref: ProviderRef::new(KEYCHAIN_PROVIDER_REF)?,
            acl,
        })
    }

    pub fn acl_policy(&self) -> KeychainAclPolicy {
        self.acl
    }
}

impl SecretProvider for KeychainSecretProvider {
    fn provider_ref(&self) -> &ProviderRef {
        &self.provider_ref
    }

    fn resolve(&self, credential_ref: &ExternalRef) -> Result<ProviderSecretMaterial> {
        let parsed = KeychainCredentialRef::parse(credential_ref.as_str())?;
        let bytes = platform::read_generic_password(parsed.service(), parsed.account())?
            .ok_or_else(|| {
                WorkcellError::Unavailable(format!(
                    "keychain entry not found: {KEYCHAIN_REF_SCHEME}{}/{}/",
                    parsed.service(),
                    parsed.account()
                ))
            })?;
        let value = String::from_utf8(bytes).map_err(|_| {
            WorkcellError::Unavailable(
                "keychain entry is not UTF-8 material; refusing to materialise opaque bytes".into(),
            )
        })?;
        Ok(ProviderSecretMaterial {
            value: SecretValue::new(value)?,
            revision_or_lease_class: None,
            expires_at: None,
            revocation_state: SecretRevocationState::Active,
        })
    }
}

/// Store bootstrap material under a validated ref, applying the provider's
/// ACL policy to the keychain item. Not part of `SecretProvider`: consumers
/// resolve, they do not write.
pub fn store_bootstrap_material(
    provider: &KeychainSecretProvider,
    credential_ref: &ExternalRef,
    material: &[u8],
) -> Result<()> {
    if material.is_empty() {
        return Err(WorkcellError::InvalidDemand(
            "refusing to store empty bootstrap material".into(),
        ));
    }
    let parsed = KeychainCredentialRef::parse(credential_ref.as_str())?;
    platform::write_generic_password(
        parsed.service(),
        parsed.account(),
        material,
        provider.acl_policy(),
    )
}

/// Remove bootstrap material. Intended for rotation flows and test cleanup.
pub fn remove_bootstrap_material(credential_ref: &ExternalRef) -> Result<()> {
    let parsed = KeychainCredentialRef::parse(credential_ref.as_str())?;
    platform::delete_generic_password(parsed.service(), parsed.account())
}

#[cfg(target_os = "macos")]
mod platform {
    use security_framework::access_control::{ProtectionMode, SecAccessControl};
    use security_framework::base::Error;
    use security_framework::passwords::{
        delete_generic_password as sec_delete_generic_password,
        get_generic_password as sec_get_generic_password,
        set_generic_password as sec_set_generic_password, set_generic_password_options,
        AccessControlOptions, PasswordOptions,
    };
    use security_framework_sys::base::errSecItemNotFound;

    use epilogos_workcell_core::{Result, WorkcellError};

    use crate::KeychainAclPolicy;

    // errSecMissingEntitlement is not exported by security-framework-sys;
    // the value is stable across macOS releases.
    const ERR_SEC_MISSING_ENTITLEMENT: i32 = -34018;

    fn unavailable(context: &str, error: Error) -> WorkcellError {
        WorkcellError::Unavailable(format!(
            "keychain {context} failed: {}",
            error
                .message()
                .unwrap_or_else(|| format!("code {}", error.code()))
        ))
    }

    pub fn read_generic_password(service: &str, account: &str) -> Result<Option<Vec<u8>>> {
        match sec_get_generic_password(service, account) {
            Ok(bytes) => Ok(Some(bytes)),
            Err(error) if error.code() == errSecItemNotFound => Ok(None),
            Err(error) => Err(unavailable("read", error)),
        }
    }

    pub fn write_generic_password(
        service: &str,
        account: &str,
        material: &[u8],
        acl: KeychainAclPolicy,
    ) -> Result<()> {
        match acl {
            KeychainAclPolicy::ThisDeviceUnlocked => {
                // No access-control object: the item inherits the default
                // when-unlocked accessibility. Deliberately simple — and the
                // only variant that works from an unsigned host binary.
                sec_set_generic_password(service, account, material)
                    .map_err(|error| unavailable("write", error))
            }
            KeychainAclPolicy::UserPresenceRequired => {
                // A user-presence constraint requires the host binary to be
                // codesigned with an application identifier; macOS returns
                // errSecMissingEntitlement otherwise. That is a capability
                // fact about the host, so it is reported precisely rather
                // than flattened into a generic write failure.
                let access_control = SecAccessControl::create_with_protection(
                    Some(ProtectionMode::AccessibleWhenUnlockedThisDeviceOnly),
                    AccessControlOptions::USER_PRESENCE.bits(),
                )
                .map_err(|error| unavailable("access-control creation", error))?;
                let mut options = PasswordOptions::new_generic_password(service, account);
                options.set_access_control(access_control);
                match set_generic_password_options(material, options) {
                    Ok(()) => Ok(()),
                    Err(error) if error.code() == ERR_SEC_MISSING_ENTITLEMENT => {
                        Err(WorkcellError::Unavailable(
                            "user-presence ACL requires a codesigned host binary \
                             (errSecMissingEntitlement); sign the host or use ThisDeviceUnlocked"
                                .into(),
                        ))
                    }
                    Err(error) => Err(unavailable("write", error)),
                }
            }
        }
    }

    pub fn delete_generic_password(service: &str, account: &str) -> Result<()> {
        match sec_delete_generic_password(service, account) {
            Ok(()) => Ok(()),
            Err(error) if error.code() == errSecItemNotFound => Ok(()),
            Err(error) => Err(unavailable("delete", error)),
        }
    }
}

#[cfg(not(target_os = "macos"))]
mod platform {
    use epilogos_workcell_core::{Result, WorkcellError};

    pub fn read_generic_password(_service: &str, _account: &str) -> Result<Option<Vec<u8>>> {
        Err(WorkcellError::Unavailable(
            "keychain secret provider is macOS-only; declare a different secret source for this platform".into(),
        ))
    }

    pub fn write_generic_password(
        _service: &str,
        _account: &str,
        _material: &[u8],
        _acl: crate::KeychainAclPolicy,
    ) -> Result<()> {
        Err(WorkcellError::Unavailable(
            "keychain secret provider is macOS-only; refusing to store elsewhere silently".into(),
        ))
    }

    pub fn delete_generic_password(_service: &str, _account: &str) -> Result<()> {
        Err(WorkcellError::Unavailable(
            "keychain secret provider is macOS-only; nothing was deleted".into(),
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn keychain_ref(value: &str) -> ExternalRef {
        ExternalRef::new(value).unwrap()
    }

    #[test]
    fn credential_ref_parses_scheme_and_segments() {
        let parsed =
            KeychainCredentialRef::parse("keychain://workcell/op-service-account").unwrap();
        assert_eq!(parsed.service(), "workcell");
        assert_eq!(parsed.account(), "op-service-account");
    }

    #[test]
    fn credential_ref_rejects_wrong_scheme_and_bad_shapes() {
        assert!(KeychainCredentialRef::parse("op://vault/item").is_err());
        assert!(KeychainCredentialRef::parse("keychain://only-service").is_err());
        assert!(KeychainCredentialRef::parse("keychain://svc/acct/extra").is_err());
        assert!(KeychainCredentialRef::parse("keychain:// /acct").is_err());
        assert!(KeychainCredentialRef::new("bad service", "acct").is_err());
        assert!(KeychainCredentialRef::new("svc", "bad/acct").is_err());
    }

    #[test]
    fn acl_policy_statement_matches_variant() {
        assert!(KeychainAclPolicy::ThisDeviceUnlocked
            .acl_policy()
            .contains("AccessibleWhenUnlockedThisDeviceOnly"));
        assert!(KeychainAclPolicy::UserPresenceRequired
            .acl_policy()
            .contains("UserPresence"));
        assert_eq!(
            KeychainAclPolicy::default(),
            KeychainAclPolicy::ThisDeviceUnlocked
        );
    }

    #[test]
    fn provider_carries_stable_ref_and_acl() {
        let provider =
            KeychainSecretProvider::new(KeychainAclPolicy::UserPresenceRequired).unwrap();
        assert_eq!(provider.provider_ref().as_str(), KEYCHAIN_PROVIDER_REF);
        assert_eq!(
            provider.acl_policy(),
            KeychainAclPolicy::UserPresenceRequired
        );
    }

    #[test]
    fn resolve_rejects_malformed_refs_without_touching_keychain() {
        let provider = KeychainSecretProvider::new(KeychainAclPolicy::default()).unwrap();
        let err = provider
            .resolve(&keychain_ref("env://NOT_A_KEYCHAIN_REF"))
            .unwrap_err();
        assert!(format!("{err:?}").contains("keychain://"));
    }

    #[test]
    fn store_refuses_empty_material_before_any_platform_call() {
        let provider = KeychainSecretProvider::new(KeychainAclPolicy::default()).unwrap();
        let err = store_bootstrap_material(&provider, &keychain_ref("keychain://svc/acct"), b"")
            .unwrap_err();
        assert!(format!("{err:?}").contains("empty"));
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn live_roundtrip_store_resolve_remove_honours_redaction() {
        let unique = format!(
            "workcell-keychain-test-{}-{}",
            std::process::id(),
            chrono_free_nanos()
        );
        let service = "workcell-keychain-test";
        let credential = keychain_ref(&format!("keychain://{service}/{unique}"));
        let provider = KeychainSecretProvider::new(KeychainAclPolicy::ThisDeviceUnlocked).unwrap();

        store_bootstrap_material(&provider, &credential, b"roundtrip-fixture-material").unwrap();
        let material = provider.resolve(&credential).unwrap();
        assert_eq!(
            material.value.expose_for_materialisation(),
            "roundtrip-fixture-material"
        );
        // The material boundary is explicit: Debug stays redacted.
        assert_eq!(format!("{:?}", material.value), "SecretValue([REDACTED])");
        assert!(!format!("{:?}", material).contains("roundtrip-fixture-material"));

        remove_bootstrap_material(&credential).unwrap();
        let err = provider.resolve(&credential).unwrap_err();
        assert!(format!("{err:?}").contains("not found"));
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn user_presence_acl_reports_capability_precisely_on_unsigned_hosts() {
        let unique = format!(
            "workcell-keychain-acl-{}-{}",
            std::process::id(),
            chrono_free_nanos()
        );
        let credential = keychain_ref(&format!("keychain://workcell-keychain-test/{unique}"));
        let provider =
            KeychainSecretProvider::new(KeychainAclPolicy::UserPresenceRequired).unwrap();

        match store_bootstrap_material(&provider, &credential, b"acl-fixture-material") {
            Ok(()) => {
                // Host is codesigned (e.g. a packaged Workcell binary): the
                // ACL applied cleanly, material must still roundtrip.
                let material = provider.resolve(&credential).unwrap();
                assert_eq!(
                    material.value.expose_for_materialisation(),
                    "acl-fixture-material"
                );
                remove_bootstrap_material(&credential).unwrap();
            }
            Err(error) => {
                // Unsigned host (e.g. cargo test): the error names the
                // capability fact, never a flattened write failure.
                let rendered = format!("{error:?}");
                assert!(
                    rendered.contains("codesigned host binary"),
                    "unexpected ACL failure: {rendered}"
                );
            }
        }
    }

    #[cfg(target_os = "macos")]
    fn chrono_free_nanos() -> u128 {
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    }
}
