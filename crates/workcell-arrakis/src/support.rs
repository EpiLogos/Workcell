use std::collections::BTreeMap;

use epilogos_workcell_core::{HealthState, WorkcellError};

pub const ARRAKIS_SOURCE_REVISION: &str = "877231496acbf3b3091ab33340d2d126a251c4d5";
pub const ARRAKIS_API_VERSION: &str = "2.0.0";
pub const ARRAKIS_LICENSE: &str = "AGPL-3.0-or-commercial";
pub const ARRAKIS_INTEGRATION_SEAM: &str = "first-party-arrakis-client-to-rest-api";

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

pub(crate) fn source_metadata() -> BTreeMap<String, String> {
    BTreeMap::from([
        ("implementation".into(), ARRAKIS_INTEGRATION_SEAM.into()),
        ("arrakis_source_revision".into(), ARRAKIS_SOURCE_REVISION.into()),
        ("arrakis_api_version".into(), ARRAKIS_API_VERSION.into()),
        ("arrakis_license".into(), ARRAKIS_LICENSE.into()),
    ])
}

pub(crate) fn status_health(output: &str) -> Result<HealthState, WorkcellError> {
    let normalized = output.to_ascii_uppercase();
    if normalized.contains("STATUS: RUNNING") || normalized.contains("\"STATUS\":\"RUNNING\"") {
        return Ok(HealthState::Healthy);
    }
    if normalized.contains("STATUS: PAUSED")
        || normalized.contains("STATUS: STOPPED")
        || normalized.contains("\"STATUS\":\"PAUSED\"")
        || normalized.contains("\"STATUS\":\"STOPPED\"")
    {
        return Ok(HealthState::Degraded);
    }
    if normalized.contains("STATUS:") || normalized.contains("\"STATUS\"") {
        return Ok(HealthState::Unknown);
    }
    Err(WorkcellError::OperationFailed(
        "Arrakis VM inspection did not contain a status".into(),
    ))
}
