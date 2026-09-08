//! workcell-onepassword: 1Password as a Workcell secret source.
//!
//! Implements the existing `SecretProvider` contract from
//! `epilogos_workcell_core` over the genuine 1Password CLI (`op read`) —
//! the same resolution primitive the pinned
//! `@varlock/1password-plugin@2.0.0` pattern uses inside varlock schemas.
//! Credential refs use the `central.security/v1` scheme:
//! `op://<vault>/<item>/<field>`.
//!
//! Boundaries, mirroring workcell-keychain:
//!   * `resolve` reads only and never logs material — `SecretValue`'s
//!     redacted Debug is the last line of defence, not the first.
//!   * No live vault calls in tests: the command runner is injectable, so
//!     the suite is hermetic. Live resolution additionally requires a
//!     1Password service-account token, which T3's keychain adapter stores
//!     under `keychain://workcell/op-service-account`.
//!   * Auth failure is reported as a capability fact ("not signed in;
//!     provide a service-account token"), never flattened into a generic
//!     resolve failure.

use std::process::Command;

use epilogos_workcell_core::{
    ExternalRef, ProviderRef, ProviderSecretMaterial, Result, SecretProvider,
    SecretRevocationState, SecretValue, WorkcellError,
};

pub const ONEPASSWORD_PROVIDER_REF: &str = "secret-provider:onepassword/cli";
pub const ONEPASSWORD_REF_SCHEME: &str = "op://";
pub const ONEPASSWORD_PLUGIN_SPEC: &str = "@varlock/1password-plugin@2.0.0";
pub const ONEPASSWORD_SOURCE_REVISION: &str = "central-security-map-T2";

/// One 1Password credential ref, parsed and validated from the
/// `op://<vault>/<item>/<field>` scheme. Refs are location only.
#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct OnePasswordCredentialRef {
    vault: String,
    item: String,
    field: String,
}

impl OnePasswordCredentialRef {
    pub fn new(
        vault: impl Into<String>,
        item: impl Into<String>,
        field: impl Into<String>,
    ) -> Result<Self> {
        let vault = vault.into();
        let item = item.into();
        let field = field.into();
        for (label, value) in [("vault", &vault), ("item", &item), ("field", &field)] {
            if value.trim().is_empty() {
                return Err(WorkcellError::InvalidDemand(format!(
                    "onepassword credential {label} must not be empty"
                )));
            }
            if value.chars().any(|c| c.is_whitespace() || c == '/') {
                return Err(WorkcellError::InvalidDemand(format!(
                    "onepassword credential {label} must not contain whitespace or '/'"
                )));
            }
        }
        Ok(Self { vault, item, field })
    }

    pub fn parse(value: &str) -> Result<Self> {
        let rest = value.strip_prefix(ONEPASSWORD_REF_SCHEME).ok_or_else(|| {
            WorkcellError::InvalidDemand(format!(
                "onepassword credential ref must start with {ONEPASSWORD_REF_SCHEME}"
            ))
        })?;
        let parts: Vec<&str> = rest.split('/').collect();
        if parts.len() != 3 || parts.iter().any(|part| part.is_empty()) {
            return Err(WorkcellError::InvalidDemand(
                "onepassword credential ref must be op://<vault>/<item>/<field>".into(),
            ));
        }
        Self::new(parts[0], parts[1], parts[2])
    }

    pub fn vault(&self) -> &str {
        &self.vault
    }

    pub fn item(&self) -> &str {
        &self.item
    }

    pub fn field(&self) -> &str {
        &self.field
    }
}

impl std::fmt::Display for OnePasswordCredentialRef {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "{ONEPASSWORD_REF_SCHEME}{}/{}/{}",
            self.vault, self.item, self.field
        )
    }
}

/// The materialisation primitive: run `op read <ref>`, capture stdout.
/// Injectable in tests so the suite never touches a live vault.
pub trait OnePasswordRunner: Send + Sync {
    fn read(
        &self,
        credential_ref: &OnePasswordCredentialRef,
    ) -> std::result::Result<String, String>;
}

/// Default runner over the real 1Password CLI. The OP_SERVICE_ACCOUNT_TOKEN
/// environment variable is honoured by `op` itself; T3's keychain adapter is
/// the durable home for that token (keychain://workcell/op-service-account).
#[derive(Clone, Copy, Debug, Default)]
pub struct OnePasswordCli;

impl OnePasswordRunner for OnePasswordCli {
    fn read(
        &self,
        credential_ref: &OnePasswordCredentialRef,
    ) -> std::result::Result<String, String> {
        let run = Command::new("op")
            .args(["read", &credential_ref.to_string()])
            .output();
        match run {
            Ok(output) if output.status.success() => String::from_utf8(output.stdout)
                .map(|value| value.trim_end_matches(['\n', '\r']).to_string())
                .map_err(|_| "op read returned non-UTF-8 material".to_string()),
            Ok(output) => {
                let stderr = String::from_utf8_lossy(&output.stderr);
                Err(classify_op_error(&stderr))
            }
            Err(error) => Err(format!(
                "failed to spawn `op` (is the 1Password CLI installed?): {error}"
            )),
        }
    }
}

/// Map `op` stderr to a precise capability statement. The distinction that
/// matters: auth absence is a HITL fact (provide a service-account token),
/// not a generic failure.
fn classify_op_error(stderr: &str) -> String {
    let lowered = stderr.to_lowercase();
    if lowered.contains("not signed in") || lowered.contains("no accounts configured") {
        "1Password CLI is not signed in; provide OP_SERVICE_ACCOUNT_TOKEN (durable home: keychain://workcell/op-service-account)".to_string()
    } else if lowered.contains("item not found") || lowered.contains("isn't an item") {
        "1Password item not found for the given op:// ref".to_string()
    } else if lowered.contains("doesn't have the access") || lowered.contains("access denied") {
        "1Password service account lacks access to this vault".to_string()
    } else {
        format!(
            "op read failed: {}",
            stderr
                .trim()
                .lines()
                .next()
                .unwrap_or("(no stderr)")
                .chars()
                .take(200)
                .collect::<String>()
        )
    }
}

/// 1Password-backed `SecretProvider`. Read-only through the trait; the CLI
/// is the SDK boundary, so no plugin host code is needed for resolution
/// itself — the varlock schema surface below is the plugin seam.
#[derive(Clone, Debug)]
pub struct OnePasswordSecretProvider<R: OnePasswordRunner = OnePasswordCli> {
    provider_ref: ProviderRef,
    runner: R,
}

impl<R: OnePasswordRunner> OnePasswordSecretProvider<R> {
    pub fn new(runner: R) -> Result<Self> {
        Ok(Self {
            provider_ref: ProviderRef::new(ONEPASSWORD_PROVIDER_REF)?,
            runner,
        })
    }
}

impl<R: OnePasswordRunner> SecretProvider for OnePasswordSecretProvider<R> {
    fn provider_ref(&self) -> &ProviderRef {
        &self.provider_ref
    }

    fn resolve(&self, credential_ref: &ExternalRef) -> Result<ProviderSecretMaterial> {
        let parsed = OnePasswordCredentialRef::parse(credential_ref.as_str())?;
        let value = self
            .runner
            .read(&parsed)
            .map_err(WorkcellError::Unavailable)?;
        if value.is_empty() {
            return Err(WorkcellError::Unavailable(
                "op read returned empty material for the given ref".into(),
            ));
        }
        Ok(ProviderSecretMaterial {
            value: SecretValue::new(value)?,
            revision_or_lease_class: None,
            expires_at: None,
            revocation_state: SecretRevocationState::Active,
        })
    }
}

/// The varlock schema surface for this provider: a `.env.schema` fragment
/// declaring the pinned 1Password plugin and one resolved secret, matching
/// the `@varlock/1password-plugin` pattern verified by the Workcell
/// conformance fixture. Owning the shape in code keeps the plugin wiring
/// reviewable instead of buried in heredocs.
pub fn varlock_schema(secret_ref: &OnePasswordCredentialRef) -> String {
    format!(
        "# @plugin({ONEPASSWORD_PLUGIN_SPEC})\n\
         # @initOp(token=$OP_TOKEN, allowAppAuth=false)\n\
         # ---\n\
         # @type=opServiceAccountToken @sensitive @internal\n\
         OP_TOKEN=\n\
         \n\
         # @sensitive @required\n\
         TARGET_SECRET=op({secret_ref})\n"
    )
}

/// Validate that a schema fragment carries the pinned plugin spec and the
/// op() resolution template — the two load-bearing lines.
pub fn validate_varlock_schema(schema: &str) -> Result<()> {
    if !schema.contains(&format!("@plugin({ONEPASSWORD_PLUGIN_SPEC})")) {
        return Err(WorkcellError::InvalidDemand(format!(
            "varlock schema must pin {ONEPASSWORD_PLUGIN_SPEC}"
        )));
    }
    if !schema.contains("op(") || !schema.contains(ONEPASSWORD_REF_SCHEME) {
        return Err(WorkcellError::InvalidDemand(
            "varlock schema must resolve at least one op:// ref via op()".into(),
        ));
    }
    if !schema.contains("OP_TOKEN") {
        return Err(WorkcellError::InvalidDemand(
            "varlock schema must declare the OP_TOKEN plugin init line".into(),
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn op_ref(value: &str) -> ExternalRef {
        ExternalRef::new(value).unwrap()
    }

    #[derive(Clone)]
    struct ScriptedRunner {
        result: std::result::Result<String, String>,
    }

    impl OnePasswordRunner for ScriptedRunner {
        fn read(
            &self,
            _credential_ref: &OnePasswordCredentialRef,
        ) -> std::result::Result<String, String> {
            self.result.clone()
        }
    }

    fn provider_with(
        result: std::result::Result<String, String>,
    ) -> OnePasswordSecretProvider<ScriptedRunner> {
        OnePasswordSecretProvider::new(ScriptedRunner { result }).unwrap()
    }

    #[test]
    fn credential_ref_parses_vault_item_field() {
        let parsed =
            OnePasswordCredentialRef::parse("op://Central/central-security/credential").unwrap();
        assert_eq!(parsed.vault(), "Central");
        assert_eq!(parsed.item(), "central-security");
        assert_eq!(parsed.field(), "credential");
        assert_eq!(
            parsed.to_string(),
            "op://Central/central-security/credential"
        );
    }

    #[test]
    fn credential_ref_rejects_wrong_scheme_and_bad_shapes() {
        assert!(OnePasswordCredentialRef::parse("keychain://svc/acct").is_err());
        assert!(OnePasswordCredentialRef::parse("op://vault/item").is_err());
        assert!(OnePasswordCredentialRef::parse("op://vault/item/field/extra").is_err());
        assert!(OnePasswordCredentialRef::parse("op://vault//field").is_err());
        assert!(OnePasswordCredentialRef::new("bad vault", "item", "field").is_err());
        assert!(OnePasswordCredentialRef::new("vault", "bad/item", "field").is_err());
    }

    #[test]
    fn resolve_maps_material_and_keeps_redaction() {
        let provider = provider_with(Ok("op-material-fixture".to_string()));
        let material = provider
            .resolve(&op_ref("op://Central/central-security/credential"))
            .unwrap();
        assert_eq!(
            material.value.expose_for_materialisation(),
            "op-material-fixture"
        );
        assert_eq!(format!("{:?}", material.value), "SecretValue([REDACTED])");
        assert!(!format!("{:?}", material).contains("op-material-fixture"));
    }

    #[test]
    fn resolve_rejects_malformed_refs_without_touching_cli() {
        let provider = provider_with(Err("runner must not be called".to_string()));
        let err = provider
            .resolve(&op_ref("env://NOT_AN_OP_REF"))
            .unwrap_err();
        assert!(format!("{err:?}").contains("op://"));
    }

    #[test]
    fn resolve_refuses_empty_material() {
        let provider = provider_with(Ok(String::new()));
        let err = provider
            .resolve(&op_ref("op://Central/central-security/credential"))
            .unwrap_err();
        assert!(format!("{err:?}").contains("empty"));
    }

    #[test]
    fn auth_failure_is_a_capability_fact_not_a_generic_error() {
        let provider = provider_with(Err(
            "1Password CLI is not signed in; provide OP_SERVICE_ACCOUNT_TOKEN (durable home: keychain://workcell/op-service-account)"
                .to_string(),
        ));
        let err = provider
            .resolve(&op_ref("op://Central/central-security/credential"))
            .unwrap_err();
        let rendered = format!("{err:?}");
        assert!(rendered.contains("not signed in"));
        assert!(rendered.contains("keychain://workcell/op-service-account"));
    }

    #[test]
    fn op_error_classifier_separates_auth_absence_item_missing_and_access() {
        assert!(classify_op_error("[ERROR] You are not signed in").contains("not signed in"));
        assert!(
            classify_op_error("[ERROR] no accounts configured for use").contains("not signed in")
        );
        assert!(classify_op_error("[ERROR] item not found").contains("item not found"));
        assert!(classify_op_error("[ERROR] doesn't have the access").contains("lacks access"));
        assert!(classify_op_error("[ERROR] network unreachable").contains("op read failed"));
    }

    #[test]
    fn varlock_schema_pins_plugin_and_resolves_op_ref() {
        let secret_ref =
            OnePasswordCredentialRef::parse("op://Central/central-security/credential").unwrap();
        let schema = varlock_schema(&secret_ref);
        validate_varlock_schema(&schema).unwrap();
        assert!(schema.contains("@plugin(@varlock/1password-plugin@2.0.0)"));
        assert!(schema.contains("TARGET_SECRET=op(op://Central/central-security/credential)"));
        assert!(schema.contains("@initOp(token=$OP_TOKEN, allowAppAuth=false)"));
    }

    #[test]
    fn varlock_schema_validation_rejects_unpinned_plugins() {
        let err = validate_varlock_schema("TARGET_SECRET=op(op://v/i/f)").unwrap_err();
        assert!(format!("{err:?}").contains("pin"));
    }
}
