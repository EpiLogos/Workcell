use std::{collections::BTreeMap, path::Path};

use epilogos_workcell_core::{
    PersistenceScope, Result, RetentionExpectation, WorkcellError,
};

use crate::{DockerCommand, DockerCommandRunner};

pub const DOCKER_ENGINE_SOURCE_PIN: &str = "29.6.2";
pub const DOCKER_COMPOSE_SOURCE_PIN: &str = "5.1.4";
pub const DOCKER_INTEGRATION_SEAM: &str = "docker-cli+compose-plugin";

pub(crate) fn stable_key(parts: &[&str]) -> String {
    let mut hash = 0xcbf29ce484222325_u64;
    for part in parts {
        for byte in part.as_bytes() {
            hash ^= u64::from(*byte);
            hash = hash.wrapping_mul(0x100000001b3);
        }
        hash ^= 0xff;
        hash = hash.wrapping_mul(0x100000001b3);
    }
    format!("{hash:016x}")
}

pub(crate) fn probe_engine(runner: &dyn DockerCommandRunner) -> Result<String> {
    let output = runner.run(&DockerCommand::new([
        "version",
        "--format",
        "{{.Server.Version}}",
    ]))?;
    nonempty_output("Docker Engine server version", output.stdout)
}

pub(crate) fn probe_compose(runner: &dyn DockerCommandRunner) -> Result<String> {
    let output = runner.run(&DockerCommand::new([
        "compose",
        "version",
        "--short",
    ]))?;
    nonempty_output("Docker Compose version", output.stdout)
}

pub(crate) fn nonempty_output(label: &str, value: String) -> Result<String> {
    let value = value.trim().to_owned();
    if value.is_empty() {
        return Err(WorkcellError::OperationFailed(format!(
            "{label} returned empty output"
        )));
    }
    Ok(value)
}

pub(crate) fn provider_metadata(
    engine_version: &str,
    compose_version: Option<&str>,
) -> BTreeMap<String, String> {
    let mut metadata = BTreeMap::new();
    metadata.insert("implementation".into(), DOCKER_INTEGRATION_SEAM.into());
    metadata.insert("docker_engine".into(), engine_version.into());
    metadata.insert("docker_engine_source_pin".into(), DOCKER_ENGINE_SOURCE_PIN.into());
    if let Some(compose_version) = compose_version {
        metadata.insert("docker_compose".into(), compose_version.into());
        metadata.insert(
            "docker_compose_source_pin".into(),
            DOCKER_COMPOSE_SOURCE_PIN.into(),
        );
    }
    metadata
}

pub(crate) fn short_lived_persistence(scope: Option<&PersistenceScope>) -> bool {
    matches!(
        scope,
        Some(PersistenceScope::Ephemeral)
            | Some(PersistenceScope::TaskOrRun)
            | Some(PersistenceScope::Candidate)
    )
}

pub(crate) fn preserve_requested(retention: &RetentionExpectation) -> bool {
    matches!(retention, RetentionExpectation::Preserve)
}

pub(crate) fn path_string(path: &Path) -> String {
    path.to_string_lossy().into_owned()
}

pub(crate) fn docker_memory_bytes(
    amount: u64,
    unit: Option<&str>,
) -> Result<String> {
    let multiplier = match unit.map(str::to_ascii_lowercase).as_deref() {
        None | Some("b") | Some("bytes") => 1_u64,
        Some("kib") | Some("ki") => 1024,
        Some("mib") | Some("mi") => 1024_u64.pow(2),
        Some("gib") | Some("gi") => 1024_u64.pow(3),
        Some("tib") | Some("ti") => 1024_u64.pow(4),
        Some(other) => {
            return Err(WorkcellError::UnsatisfiedDemand(format!(
                "Docker execution provider cannot map memory unit `{other}`"
            )))
        }
    };
    amount
        .checked_mul(multiplier)
        .map(|bytes| bytes.to_string())
        .ok_or_else(|| WorkcellError::UnsatisfiedDemand("memory requirement overflowed".into()))
}
