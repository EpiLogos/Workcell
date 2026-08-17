use std::{
    collections::BTreeMap,
    fs::{self, OpenOptions},
    io::Write,
    path::Path,
    process::{Command, Output, Stdio},
};

#[cfg(unix)]
use std::os::unix::fs::OpenOptionsExt;

use epilogos_workcell_core::{
    receipt_for, ProviderSecretMaterial, Result, SecretMaterialReceipt, SecretMaterialisationClass,
    SecretMaterialisationRequest, SecretProvider, SecretRefreshRequirement, SecretRevocationState,
    WorkcellError,
};

#[derive(Debug)]
pub struct MaterialisedChild {
    pub output: Output,
    pub receipt: SecretMaterialReceipt,
}

fn resolve_active<P: SecretProvider>(
    provider: &P,
    request: &SecretMaterialisationRequest,
) -> Result<ProviderSecretMaterial> {
    request.validate()?;
    if provider.provider_ref() != &request.provider_ref {
        return Err(WorkcellError::UnsatisfiedDemand(
            "selected SecretProvider does not match materialisation request".into(),
        ));
    }
    let material = provider.resolve(&request.credential_ref)?;
    if material.revocation_state != SecretRevocationState::Active {
        return Err(WorkcellError::Unavailable(
            "credential material is expired or revoked".into(),
        ));
    }
    Ok(material)
}

fn child_command(
    program: &str,
    args: &[String],
    environment: &BTreeMap<String, String>,
) -> Command {
    let mut command = Command::new(program);
    command.args(args).env_clear().envs(environment);
    command
}

fn redact_output(mut output: Output, secret: &str) -> Output {
    let replacement = b"[REDACTED]";
    output.stdout = replace_bytes(&output.stdout, secret.as_bytes(), replacement);
    output.stderr = replace_bytes(&output.stderr, secret.as_bytes(), replacement);
    output
}

fn replace_bytes(input: &[u8], needle: &[u8], replacement: &[u8]) -> Vec<u8> {
    if needle.is_empty() {
        return input.to_vec();
    }
    let mut result = Vec::with_capacity(input.len());
    let mut offset = 0;
    while offset < input.len() {
        if input[offset..].starts_with(needle) {
            result.extend_from_slice(replacement);
            offset += needle.len();
        } else {
            result.push(input[offset]);
            offset += 1;
        }
    }
    result
}

/// Materialise one credential in exactly one child environment. The parent/global shell is never
/// mutated. `environment` is an explicit allowlist; `env_clear` prevents ambient inheritance.
pub fn run_with_secret_env<P: SecretProvider>(
    provider: &P,
    request: &SecretMaterialisationRequest,
    program: &str,
    args: &[String],
    environment: &BTreeMap<String, String>,
) -> Result<MaterialisedChild> {
    if !matches!(
        request.class,
        SecretMaterialisationClass::ProcessEnv | SecretMaterialisationClass::OneShotChildProcess
    ) {
        return Err(WorkcellError::InvalidDemand(
            "child env delivery requires ProcessEnv or OneShotChildProcess class".into(),
        ));
    }
    let material = resolve_active(provider, request)?;
    let secret = material.value.expose_for_materialisation();
    let mut command = child_command(program, args, environment);
    command.env(&request.destination, secret);
    let output = command.output().map_err(|error| {
        WorkcellError::OperationFailed(format!("child process failed: {error}"))
    })?;
    let output = redact_output(output, secret);
    let refresh = match request.class {
        SecretMaterialisationClass::ProcessEnv => SecretRefreshRequirement::RestartRequired,
        SecretMaterialisationClass::OneShotChildProcess => SecretRefreshRequirement::NoneAfterExit,
        _ => unreachable!(),
    };
    Ok(MaterialisedChild {
        output,
        receipt: receipt_for(request, &material, refresh),
    })
}

/// Write material to the child's stdin pipe. No secret-bearing environment variable is created.
pub fn run_with_secret_pipe<P: SecretProvider>(
    provider: &P,
    request: &SecretMaterialisationRequest,
    program: &str,
    args: &[String],
    environment: &BTreeMap<String, String>,
) -> Result<MaterialisedChild> {
    if request.class != SecretMaterialisationClass::FdOrPipe {
        return Err(WorkcellError::InvalidDemand(
            "stdin pipe delivery requires FdOrPipe class".into(),
        ));
    }
    let material = resolve_active(provider, request)?;
    let secret = material.value.expose_for_materialisation();
    let mut command = child_command(program, args, environment);
    command
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let mut child = command.spawn().map_err(|error| {
        WorkcellError::OperationFailed(format!("child process failed: {error}"))
    })?;
    let mut stdin = child
        .stdin
        .take()
        .ok_or_else(|| WorkcellError::OperationFailed("child stdin pipe was not created".into()))?;
    stdin.write_all(secret.as_bytes()).map_err(|error| {
        WorkcellError::OperationFailed(format!("secret pipe write failed: {error}"))
    })?;
    drop(stdin);
    let output = child
        .wait_with_output()
        .map_err(|error| WorkcellError::OperationFailed(format!("child wait failed: {error}")))?;
    let output = redact_output(output, secret);
    Ok(MaterialisedChild {
        output,
        receipt: receipt_for(request, &material, SecretRefreshRequirement::NoneAfterExit),
    })
}

/// Materialise a one-shot 0600 file at the explicitly approved destination and delete it after the
/// child exits. This is a plain file delivery proof, not a claim of tmpfs/native secret mounts.
pub fn run_with_secret_file<P: SecretProvider>(
    provider: &P,
    request: &SecretMaterialisationRequest,
    program: &str,
    args: &[String],
    environment: &BTreeMap<String, String>,
) -> Result<MaterialisedChild> {
    if request.class != SecretMaterialisationClass::File {
        return Err(WorkcellError::InvalidDemand(
            "file delivery requires File materialisation class".into(),
        ));
    }
    let material = resolve_active(provider, request)?;
    let secret = material.value.expose_for_materialisation();
    let path = Path::new(&request.destination);
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    options.mode(0o600);
    let mut file = options.open(path).map_err(|error| {
        WorkcellError::OperationFailed(format!("secret file creation failed: {error}"))
    })?;
    file.write_all(secret.as_bytes()).map_err(|error| {
        WorkcellError::OperationFailed(format!("secret file write failed: {error}"))
    })?;
    drop(file);

    let output_result = child_command(program, args, environment).output();
    let cleanup_result = fs::remove_file(path);
    let output = output_result.map_err(|error| {
        WorkcellError::OperationFailed(format!("child process failed: {error}"))
    })?;
    cleanup_result.map_err(|error| {
        WorkcellError::CleanupFailed(format!("secret file cleanup failed: {error}"))
    })?;

    Ok(MaterialisedChild {
        output: redact_output(output, secret),
        receipt: receipt_for(request, &material, SecretRefreshRequirement::NoneAfterExit),
    })
}

#[cfg(test)]
mod tests {
    use std::{env, fs};

    use epilogos_workcell_core::{
        BindingRef, ExternalRef, ProviderRef, ProviderSecretMaterial, SecretRevocationState,
        SecretValue,
    };

    use super::*;

    struct FixedProvider {
        provider_ref: ProviderRef,
        value: String,
        revision: String,
    }

    impl SecretProvider for FixedProvider {
        fn provider_ref(&self) -> &ProviderRef {
            &self.provider_ref
        }

        fn resolve(&self, _credential_ref: &ExternalRef) -> Result<ProviderSecretMaterial> {
            Ok(ProviderSecretMaterial {
                value: SecretValue::new(self.value.clone())?,
                revision_or_lease_class: Some(self.revision.clone()),
                expires_at: None,
                revocation_state: SecretRevocationState::Active,
            })
        }
    }

    fn request(
        class: SecretMaterialisationClass,
        destination: String,
    ) -> SecretMaterialisationRequest {
        SecretMaterialisationRequest {
            credential_ref: ExternalRef::new("credential:fixture/service").unwrap(),
            provider_ref: ProviderRef::new("secret-provider:fixture").unwrap(),
            binding_ref: BindingRef::new("secret-binding:fixture/service").unwrap(),
            consumer_ref: ExternalRef::new("workload:child-fixture").unwrap(),
            workload_ref: Some(ExternalRef::new("workload:child-fixture").unwrap()),
            class,
            purpose: "fixture-call".into(),
            destination,
            scope: "fixture:read".into(),
        }
    }

    #[test]
    fn process_env_is_child_scoped_ambient_env_is_cleared_and_output_is_redacted() {
        let provider = FixedProvider {
            provider_ref: ProviderRef::new("secret-provider:fixture").unwrap(),
            value: "RAW_CHILD_SECRET".into(),
            revision: "revision:v1".into(),
        };
        let secret_var = "WORKCELL_FIXTURE_SECRET";
        assert!(env::var_os(secret_var).is_none());

        let args = vec![
            "-c".into(),
            "printf '%s|%s' \"$WORKCELL_FIXTURE_SECRET\" \"${SHOULD_NOT_LEAK-unset}\"".into(),
        ];
        let child = run_with_secret_env(
            &provider,
            &request(SecretMaterialisationClass::ProcessEnv, secret_var.into()),
            "sh",
            &args,
            &BTreeMap::new(),
        )
        .unwrap();

        assert_eq!(
            String::from_utf8(child.output.stdout).unwrap(),
            "[REDACTED]|unset"
        );
        assert!(env::var_os(secret_var).is_none());
        assert_eq!(
            child.receipt.refresh_requirement,
            SecretRefreshRequirement::RestartRequired
        );
        assert!(!format!("{:?}", child.receipt).contains("RAW_CHILD_SECRET"));
    }

    #[test]
    fn stdin_pipe_delivers_without_secret_environment() {
        let provider = FixedProvider {
            provider_ref: ProviderRef::new("secret-provider:fixture").unwrap(),
            value: "RAW_PIPE_SECRET".into(),
            revision: "revision:v1".into(),
        };
        let args = vec![
            "-c".into(),
            "read value; printf '%s|%s' \"$value\" \"${WORKCELL_FIXTURE_SECRET-unset}\"".into(),
        ];
        let child = run_with_secret_pipe(
            &provider,
            &request(SecretMaterialisationClass::FdOrPipe, "stdin".into()),
            "sh",
            &args,
            &BTreeMap::new(),
        )
        .unwrap();

        assert_eq!(
            String::from_utf8(child.output.stdout).unwrap(),
            "[REDACTED]|unset"
        );
    }

    #[test]
    fn one_shot_file_is_removed_and_never_enters_child_environment() {
        let provider = FixedProvider {
            provider_ref: ProviderRef::new("secret-provider:fixture").unwrap(),
            value: "RAW_FILE_SECRET".into(),
            revision: "revision:v1".into(),
        };
        let path = env::temp_dir().join(format!("workcell-secret-{}", std::process::id()));
        let _ = fs::remove_file(&path);
        let args = vec![
            "-c".into(),
            format!(
                "printf '%s|%s' \"$(cat '{}')\" \"${{WORKCELL_FIXTURE_SECRET-unset}}\"",
                path.display()
            ),
        ];
        let child = run_with_secret_file(
            &provider,
            &request(SecretMaterialisationClass::File, path.display().to_string()),
            "sh",
            &args,
            &BTreeMap::new(),
        )
        .unwrap();

        assert_eq!(
            String::from_utf8(child.output.stdout).unwrap(),
            "[REDACTED]|unset"
        );
        assert!(!path.exists());
    }

    #[test]
    fn env_rotation_is_visible_as_restart_bound_revision_change() {
        let v1 = FixedProvider {
            provider_ref: ProviderRef::new("secret-provider:fixture").unwrap(),
            value: "ROTATE_V1".into(),
            revision: "revision:v1".into(),
        };
        let v2 = FixedProvider {
            provider_ref: ProviderRef::new("secret-provider:fixture").unwrap(),
            value: "ROTATE_V2".into(),
            revision: "revision:v2".into(),
        };
        let req = request(
            SecretMaterialisationClass::OneShotChildProcess,
            "WORKCELL_ROTATING_SECRET".into(),
        );
        let args = vec![
            "-c".into(),
            "printf '%s' \"$WORKCELL_ROTATING_SECRET\"".into(),
        ];
        let first = run_with_secret_env(&v1, &req, "sh", &args, &BTreeMap::new()).unwrap();
        let second = run_with_secret_env(&v2, &req, "sh", &args, &BTreeMap::new()).unwrap();

        assert_eq!(first.receipt.credential_ref, second.receipt.credential_ref);
        assert_ne!(
            first.receipt.revision_or_lease_class,
            second.receipt.revision_or_lease_class
        );
    }
}
