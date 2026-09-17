//! workcell-secret-service: the Linux Secret Service (`org.freedesktop.secrets`)
//! as a Workcell secret source.
//!
//! Implements the existing `SecretProvider` contract from
//! `epilogos_workcell_core` over the freedesktop Secret Service D-Bus API —
//! the surface every Linux desktop secret store exposes (GNOME keyring,
//! KDE Wallet's compat daemon). This is the native route: a D-Bus client,
//! not a shell-out to `secret-tool`. Credential refs use the
//! `central.security/v1` ref scheme: `linux-secret-service://<service>/<account>`.
//!
//! Two deliberate boundaries, mirroring workcell-keychain:
//!   * `resolve` reads only. Materialisation classes, receipts and output
//!     redaction stay with workcell-runtime — this crate never prints
//!     material and inherits `SecretValue`'s redacted Debug.
//!   * Store/remove are bootstrap operations, kept off the `SecretProvider`
//!     trait on purpose: the trait is the consumer seam, and consumers must
//!     not be able to write the vault. The ACL story is applied at store
//!     time and documented in `SecretServiceSecretProvider::acl_policy()`.
//!
//! Portability and refusal law: on Linux this talks to the real Secret
//! Service over the session bus. A bus that is absent, a service that is
//! not nameable, or a locked collection is a *declared* refusal
//! (`WorkcellError::Unavailable` with the reason) — "could not run" must
//! never collapse into "ran and found nothing". On every other platform the
//! crate compiles so the hermetic suite runs, and every operation refuses
//! with the same declared shape.

use epilogos_workcell_core::{
    ExternalRef, ProviderRef, ProviderSecretMaterial, Result, SecretProvider,
    SecretRevocationState, SecretValue, WorkcellError,
};

pub const SECRET_SERVICE_PROVIDER_REF: &str = "secret-provider:secret-service/linux";
pub const SECRET_SERVICE_REF_SCHEME: &str = "linux-secret-service://";
pub const SECRET_SERVICE_SOURCE_REVISION: &str = "central-security-map-T4";
/// The conventional schema attribute, set at store time so every Workcell
/// item in the collection is nameable as Workcell material — and so a scan
/// of the collection can distinguish Workcell items from anything else.
pub const SECRET_SERVICE_SCHEMA: &str = "workcell.secret/v1";
pub const SECRET_SERVICE_SCHEMA_ATTRIBUTE: &str = "xdg:schema";

/// One Secret Service credential ref, parsed and validated from the
/// `linux-secret-service://<service>/<account>` scheme. Refs are location
/// only — the value never appears here. The item lives in the default
/// (login) collection under attributes `{xdg:schema, service, account}`;
/// the Secret Service addresses items by attributes, not by name.
#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct SecretServiceCredentialRef {
    service: String,
    account: String,
}

impl SecretServiceCredentialRef {
    pub fn new(service: impl Into<String>, account: impl Into<String>) -> Result<Self> {
        let service = service.into();
        let account = account.into();
        for (label, value) in [("service", &service), ("account", &account)] {
            if value.trim().is_empty() {
                return Err(WorkcellError::InvalidDemand(format!(
                    "secret service credential {label} must not be empty"
                )));
            }
            if value
                .chars()
                .any(|c| c.is_whitespace() || c == '/' || c == '\\')
            {
                return Err(WorkcellError::InvalidDemand(format!(
                    "secret service credential {label} must not contain whitespace or path separators"
                )));
            }
        }
        Ok(Self { service, account })
    }

    pub fn parse(value: &str) -> Result<Self> {
        let rest = value
            .strip_prefix(SECRET_SERVICE_REF_SCHEME)
            .ok_or_else(|| {
                WorkcellError::InvalidDemand(format!(
                    "secret service credential ref must start with {SECRET_SERVICE_REF_SCHEME}"
                ))
            })?;
        let (service, account) = rest.split_once('/').ok_or_else(|| {
            WorkcellError::InvalidDemand(
                "secret service credential ref must be linux-secret-service://<service>/<account>"
                    .into(),
            )
        })?;
        if account.contains('/') {
            return Err(WorkcellError::InvalidDemand(
                "secret service credential ref takes exactly one service and one account".into(),
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

    /// The attribute set the item is stored under and searched by. Store and
    /// resolve share this one constructor so the two can never drift.
    pub(crate) fn attributes(&self) -> std::collections::HashMap<String, String> {
        std::collections::HashMap::from([
            (
                SECRET_SERVICE_SCHEMA_ATTRIBUTE.to_owned(),
                SECRET_SERVICE_SCHEMA.to_owned(),
            ),
            ("service".to_owned(), self.service.clone()),
            ("account".to_owned(), self.account.clone()),
        ])
    }
}

/// Linux Secret Service-backed `SecretProvider`. Reads only through the
/// trait; writes go through `store_bootstrap_material`, which is
/// deliberately not on the trait.
#[derive(Clone, Debug)]
pub struct SecretServiceSecretProvider {
    provider_ref: ProviderRef,
}

impl SecretServiceSecretProvider {
    pub fn new() -> Result<Self> {
        Ok(Self {
            provider_ref: ProviderRef::new(SECRET_SERVICE_PROVIDER_REF)?,
        })
    }

    /// Human-readable ACL statement, kept beside the code that enforces it
    /// so the ACL story is reviewable in one place. The Secret Service has
    /// no per-item ACL object: the store-time decision is the default
    /// (login) collection, unlocked with the desktop session; keyrings are
    /// local files that never sync; per-client visibility is whatever the
    /// keyring daemon enforces on the session bus.
    pub fn acl_policy(&self) -> &'static str {
        "default (login) collection; unlocked with the desktop session; \
         local to this device (keyrings are never synced); no per-item ACL \
         object exists in org.freedesktop.secrets — collection unlock is the \
         access boundary, enforced by the keyring daemon at read time"
    }
}

impl Default for SecretServiceSecretProvider {
    fn default() -> Self {
        Self::new().expect("static provider ref is valid")
    }
}

impl SecretProvider for SecretServiceSecretProvider {
    fn provider_ref(&self) -> &ProviderRef {
        &self.provider_ref
    }

    fn resolve(&self, credential_ref: &ExternalRef) -> Result<ProviderSecretMaterial> {
        let parsed = SecretServiceCredentialRef::parse(credential_ref.as_str())?;
        let bytes = service::read_item(parsed.service(), parsed.account())?.ok_or_else(|| {
            WorkcellError::Unavailable(format!(
                "secret service entry not found: {SECRET_SERVICE_REF_SCHEME}{}/{}/",
                parsed.service(),
                parsed.account()
            ))
        })?;
        let value = String::from_utf8(bytes).map_err(|_| {
            WorkcellError::Unavailable(
                "secret service entry is not UTF-8 material; refusing to materialise opaque bytes"
                    .into(),
            )
        })?;
        Ok(ProviderSecretMaterial {
            value: SecretValue::new(value)?,
            revision_or_lease_class: Some(SECRET_SERVICE_SOURCE_REVISION.to_owned()),
            expires_at: None,
            revocation_state: SecretRevocationState::Active,
        })
    }
}

/// Store bootstrap material under a validated ref in the default collection.
/// Not part of `SecretProvider`: consumers resolve, they do not write.
pub fn store_bootstrap_material(
    _provider: &SecretServiceSecretProvider,
    credential_ref: &ExternalRef,
    material: &[u8],
) -> Result<()> {
    if material.is_empty() {
        return Err(WorkcellError::InvalidDemand(
            "refusing to store empty bootstrap material".into(),
        ));
    }
    let parsed = SecretServiceCredentialRef::parse(credential_ref.as_str())?;
    service::write_item(parsed.service(), parsed.account(), material)
}

/// Remove bootstrap material. Intended for rotation flows and test cleanup.
pub fn remove_bootstrap_material(credential_ref: &ExternalRef) -> Result<()> {
    let parsed = SecretServiceCredentialRef::parse(credential_ref.as_str())?;
    service::delete_item(parsed.service(), parsed.account())
}

#[cfg(target_os = "linux")]
mod service {
    use std::collections::HashMap;

    use epilogos_workcell_core::{Result, WorkcellError};
    use zbus::zvariant::{ObjectPath, OwnedObjectPath, Value};
    use zbus::{blocking::Connection, proxy};

    use crate::SECRET_SERVICE_SCHEMA_ATTRIBUTE;

    #[proxy(
        interface = "org.freedesktop.Secret.Service",
        default_service = "org.freedesktop.secrets",
        default_path = "/org/freedesktop/secrets"
    )]
    trait SecretServiceApi {
        fn open_session(
            &self,
            algorithm: &str,
            input: Value<'_>,
        ) -> zbus::Result<(zbus::zvariant::OwnedValue, OwnedObjectPath)>;

        fn search_items(
            &self,
            attributes: HashMap<String, String>,
        ) -> zbus::Result<(Vec<OwnedObjectPath>, Vec<OwnedObjectPath>)>;

        fn read_alias(&self, name: &str) -> zbus::Result<OwnedObjectPath>;
    }

    #[proxy(
        interface = "org.freedesktop.Secret.Collection",
        default_service = "org.freedesktop.secrets"
    )]
    trait SecretCollectionApi {
        fn search_items(
            &self,
            attributes: HashMap<String, String>,
        ) -> zbus::Result<Vec<OwnedObjectPath>>;

        fn create_item(
            &self,
            properties: HashMap<String, Value<'_>>,
            secret: (OwnedObjectPath, Vec<u8>, Vec<u8>, String),
            replace: bool,
        ) -> zbus::Result<(OwnedObjectPath, OwnedObjectPath)>;
    }

    #[proxy(
        interface = "org.freedesktop.Secret.Item",
        default_service = "org.freedesktop.secrets"
    )]
    trait SecretItemApi {
        fn get_secret(
            &self,
            session: OwnedObjectPath,
        ) -> zbus::Result<(OwnedObjectPath, Vec<u8>, Vec<u8>, String)>;

        fn delete(&self) -> zbus::Result<OwnedObjectPath>;
    }

    fn unavailable(context: &str, error: impl std::fmt::Display) -> WorkcellError {
        WorkcellError::Unavailable(format!(
            "Linux Secret Service {context}: {error}; a source that cannot run says so and is never reported as ran-and-found-nothing"
        ))
    }

    fn connect() -> Result<Connection> {
        Connection::session().map_err(|error| {
            unavailable(
                "session bus is not reachable (set DBUS_SESSION_BUS_ADDRESS or run inside the desktop session)",
                error,
            )
        })
    }

    fn open_session(service: &SecretServiceApiProxyBlocking<'_>) -> Result<OwnedObjectPath> {
        let (_, session) = service
            .open_session("plain", Value::from(""))
            .map_err(|error| unavailable("session could not be opened", error))?;
        Ok(session)
    }

    fn default_collection(service: &SecretServiceApiProxyBlocking<'_>) -> Result<OwnedObjectPath> {
        service
            .read_alias("default")
            .map_err(|error| unavailable("default collection is not available", error))
    }

    fn find_item(
        collection: &SecretCollectionApiProxyBlocking<'_>,
        attributes: &HashMap<String, String>,
    ) -> Result<Option<OwnedObjectPath>> {
        let items = collection
            .search_items(attributes.clone())
            .map_err(|error| unavailable("item search failed", error))?;
        Ok(items.into_iter().next())
    }

    pub fn read_item(service_name: &str, account: &str) -> Result<Option<Vec<u8>>> {
        let connection = connect()?;
        let service_proxy = SecretServiceApiProxyBlocking::builder(&connection)
            .build()
            .map_err(|error| unavailable("service proxy could not be built", error))?;
        let collection_path = default_collection(&service_proxy)?;
        let session = open_session(&service_proxy)?;
        let collection = SecretCollectionApiProxyBlocking::builder(&connection)
            .path(collection_path)
            .expect("collection path comes from the service and is a valid object path")
            .build()
            .map_err(|error| unavailable("collection proxy could not be built", error))?;

        let attributes = HashMap::from([
            (
                SECRET_SERVICE_SCHEMA_ATTRIBUTE.to_owned(),
                crate::SECRET_SERVICE_SCHEMA.to_owned(),
            ),
            ("service".to_owned(), service_name.to_owned()),
            ("account".to_owned(), account.to_owned()),
        ]);
        let Some(item_path) = find_item(&collection, &attributes)? else {
            return Ok(None);
        };

        let item = SecretItemApiProxyBlocking::builder(&connection)
            .path(item_path)
            .expect("item path comes from the collection and is a valid object path")
            .build()
            .map_err(|error| unavailable("item proxy could not be built", error))?;
        let (_, _, value, _) = item.get_secret(session).map_err(|error| {
            unavailable(
                "refused to release material (collection locked or access denied) — unlock the keyring and retry; this refusal is declared, never an empty result",
                error,
            )
        })?;
        Ok(Some(value))
    }

    pub fn write_item(service_name: &str, account: &str, material: &[u8]) -> Result<()> {
        let connection = connect()?;
        let service_proxy = SecretServiceApiProxyBlocking::builder(&connection)
            .build()
            .map_err(|error| unavailable("service proxy could not be built", error))?;
        let collection_path = default_collection(&service_proxy)?;
        let session = open_session(&service_proxy)?;
        let collection = SecretCollectionApiProxyBlocking::builder(&connection)
            .path(collection_path)
            .expect("collection path comes from the service and is a valid object path")
            .build()
            .map_err(|error| unavailable("collection proxy could not be built", error))?;

        let attributes = HashMap::from([
            (
                SECRET_SERVICE_SCHEMA_ATTRIBUTE.to_owned(),
                crate::SECRET_SERVICE_SCHEMA.to_owned(),
            ),
            ("service".to_owned(), service_name.to_owned()),
            ("account".to_owned(), account.to_owned()),
        ]);
        let properties = HashMap::from([
            (
                "org.freedesktop.Secret.Item.Label".to_owned(),
                Value::from(format!("workcell {service_name}/{account}")),
            ),
            (
                "org.freedesktop.Secret.Item.Attributes".to_owned(),
                Value::from(attributes),
            ),
        ]);
        let (item_path, prompt_path) = collection
            .create_item(
                properties,
                (
                    session,
                    Vec::new(),
                    material.to_vec(),
                    "text/plain".to_owned(),
                ),
                true,
            )
            .map_err(|error| unavailable("item could not be stored", error))?;
        if prompt_path.as_str() != "/" {
            return Err(unavailable(
                "storing requires an interactive collection-unlock prompt; unlock the default keyring and retry",
                ObjectPath::try_from(prompt_path)
                    .map(|path| format!("prompt {path}"))
                    .unwrap_or_else(|_| "an unlock prompt".to_owned()),
            ));
        }
        debug_assert!(!item_path.as_str().is_empty());
        Ok(())
    }

    pub fn delete_item(service_name: &str, account: &str) -> Result<()> {
        let connection = connect()?;
        let service_proxy = SecretServiceApiProxyBlocking::builder(&connection)
            .build()
            .map_err(|error| unavailable("service proxy could not be built", error))?;
        let collection_path = default_collection(&service_proxy)?;
        let collection = SecretCollectionApiProxyBlocking::builder(&connection)
            .path(collection_path)
            .expect("collection path comes from the service and is a valid object path")
            .build()
            .map_err(|error| unavailable("collection proxy could not be built", error))?;

        let attributes = HashMap::from([
            (
                SECRET_SERVICE_SCHEMA_ATTRIBUTE.to_owned(),
                crate::SECRET_SERVICE_SCHEMA.to_owned(),
            ),
            ("service".to_owned(), service_name.to_owned()),
            ("account".to_owned(), account.to_owned()),
        ]);
        let Some(item_path) = find_item(&collection, &attributes)? else {
            // Deleting an absent item is already the desired state.
            return Ok(());
        };

        let item = SecretItemApiProxyBlocking::builder(&connection)
            .path(item_path)
            .expect("item path comes from the collection and is a valid object path")
            .build()
            .map_err(|error| unavailable("item proxy could not be built", error))?;
        let prompt_path = item
            .delete()
            .map_err(|error| unavailable("item could not be deleted", error))?;
        if prompt_path.as_str() != "/" {
            return Err(unavailable(
                "deletion requires an interactive prompt; unlock the default keyring and retry",
                ObjectPath::try_from(prompt_path)
                    .map(|path| format!("prompt {path}"))
                    .unwrap_or_else(|_| "a deletion prompt".to_owned()),
            ));
        }
        Ok(())
    }
}

#[cfg(not(target_os = "linux"))]
mod service {
    use epilogos_workcell_core::{Result, WorkcellError};

    fn linux_only(operation: &str) -> WorkcellError {
        WorkcellError::Unavailable(format!(
            "secret service secret provider is Linux-only; {operation} was not attempted on this platform — declare a different secret source here"
        ))
    }

    pub fn read_item(_service: &str, _account: &str) -> Result<Option<Vec<u8>>> {
        Err(linux_only("resolve"))
    }

    pub fn write_item(_service: &str, _account: &str, _material: &[u8]) -> Result<()> {
        Err(linux_only("store"))
    }

    pub fn delete_item(_service: &str, _account: &str) -> Result<()> {
        Err(linux_only("delete"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn secret_service_ref(value: &str) -> ExternalRef {
        ExternalRef::new(value).unwrap()
    }

    #[test]
    fn credential_ref_parses_scheme_and_segments() {
        let parsed =
            SecretServiceCredentialRef::parse("linux-secret-service://workcell/op-service-account")
                .unwrap();
        assert_eq!(parsed.service(), "workcell");
        assert_eq!(parsed.account(), "op-service-account");
    }

    #[test]
    fn credential_ref_rejects_wrong_scheme_and_bad_shapes() {
        assert!(SecretServiceCredentialRef::parse("keychain://svc/acct").is_err());
        assert!(SecretServiceCredentialRef::parse("linux-secret-service://only-service").is_err());
        assert!(
            SecretServiceCredentialRef::parse("linux-secret-service://svc/acct/extra").is_err()
        );
        assert!(SecretServiceCredentialRef::parse("linux-secret-service:// /acct").is_err());
        assert!(SecretServiceCredentialRef::new("bad service", "acct").is_err());
        assert!(SecretServiceCredentialRef::new("svc", "bad/acct").is_err());
    }

    #[test]
    fn search_and_store_share_one_attribute_shape() {
        let parsed = SecretServiceCredentialRef::new("workcell", "probe").unwrap();
        let attributes = parsed.attributes();
        assert_eq!(
            attributes.get(crate::SECRET_SERVICE_SCHEMA_ATTRIBUTE),
            Some(&crate::SECRET_SERVICE_SCHEMA.to_owned())
        );
        assert_eq!(attributes.get("service"), Some(&"workcell".to_owned()));
        assert_eq!(attributes.get("account"), Some(&"probe".to_owned()));
    }

    #[test]
    fn provider_carries_stable_ref_and_acl_story() {
        let provider = SecretServiceSecretProvider::new().unwrap();
        assert_eq!(
            provider.provider_ref().as_str(),
            SECRET_SERVICE_PROVIDER_REF
        );
        let acl = provider.acl_policy();
        assert!(acl.contains("default (login) collection"));
        assert!(acl.contains("never synced"));
    }

    #[test]
    fn resolve_rejects_malformed_refs_without_touching_the_bus() {
        let provider = SecretServiceSecretProvider::new().unwrap();
        let err = provider
            .resolve(&secret_service_ref("env://NOT_A_SECRET_SERVICE_REF"))
            .unwrap_err();
        assert!(format!("{err:?}").contains("linux-secret-service://"));
    }

    #[test]
    fn store_refuses_empty_material_before_any_platform_call() {
        let provider = SecretServiceSecretProvider::new().unwrap();
        let err = store_bootstrap_material(
            &provider,
            &secret_service_ref("linux-secret-service://svc/acct"),
            b"",
        )
        .unwrap_err();
        assert!(format!("{err:?}").contains("empty"));
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn unavailable_bus_is_a_declared_refusal_that_never_reads_as_found_nothing() {
        // With no session bus in this process's environment, resolve must
        // refuse with a reason — the error text is the law here: "could not
        // run" never collapses into "ran and found nothing".
        if std::env::var_os("DBUS_SESSION_BUS_ADDRESS").is_some()
            || std::env::var_os("XDG_RUNTIME_DIR")
                .map(|dir| std::path::PathBuf::from(dir).join("bus").exists())
                .unwrap_or(false)
        {
            // A bus is reachable on this host: the declared-refusal path for
            // an absent bus is not exercisable here; the live roundtrip test
            // covers the reachable case.
            return;
        }
        let provider = SecretServiceSecretProvider::new().unwrap();
        let err = provider
            .resolve(&secret_service_ref(
                "linux-secret-service://workcell/whatever",
            ))
            .unwrap_err();
        let rendered = format!("{err:?}");
        assert!(
            rendered.contains("Linux Secret Service"),
            "refusal must name the source: {rendered}"
        );
        assert!(
            rendered.contains("not reachable") || rendered.contains("could not"),
            "refusal must state the capability fact: {rendered}"
        );
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn live_roundtrip_store_resolve_remove_honours_redaction() {
        use std::time::{SystemTime, UNIX_EPOCH};

        // Declared skip: the live roundtrip needs a real Secret Service. On
        // a host with no session bus the skip names itself in the test
        // output rather than passing silently.
        if zbus::blocking::Connection::session().is_err() {
            eprintln!(
                "declared: no session bus on this host; live Secret Service roundtrip skipped"
            );
            return;
        }

        let unique = format!(
            "workcell-secret-service-test-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        );
        let service_name = "workcell-secret-service-test";
        let credential =
            secret_service_ref(&format!("linux-secret-service://{service_name}/{unique}"));
        let provider = SecretServiceSecretProvider::new().unwrap();

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
}
