use std::collections::BTreeMap;

use epilogos_workcell_core::{ProviderAllocation, Result, WorkcellError};

use super::{
    client::{data_request, require_success_safe, resolve_data_endpoint},
    protocol::{
        OpenSandboxConfig, OpenSandboxTransport, OPENSANDBOX_EXECD_PORT,
        OPENSANDBOX_EXECD_SPEC_BLOB, OPENSANDBOX_SOURCE_REVISION,
    },
};

/// Native OpenSandbox file reading. The bytes remain data-plane material;
/// Workcell records only enough provenance to explain where the reading came
/// from and does not turn file content into Workcell semantic state.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct OpenSandboxFileReading {
    pub sandbox_material_ref: String,
    pub path: String,
    pub bytes: Vec<u8>,
    pub provenance: BTreeMap<String, String>,
}

/// Narrow public façade over OpenSandbox's native execd data plane.
///
/// This intentionally does not implement a second Workcell control protocol.
/// It resolves the already-materialised sandbox's execd endpoint, preserves
/// provider-required endpoint authentication internally for the request, and
/// returns only the target-native result plus safe provenance.
pub struct OpenSandboxDataPlane<T> {
    config: OpenSandboxConfig,
    transport: T,
}

impl<T> OpenSandboxDataPlane<T>
where
    T: OpenSandboxTransport,
{
    pub fn new(config: OpenSandboxConfig, transport: T) -> Result<Self> {
        config.validate()?;
        Ok(Self { config, transport })
    }

    /// Read one absolute file path through execd's `/files/download` API.
    /// Endpoint header values never appear in the returned reading.
    pub fn read_file(
        &self,
        allocation: &ProviderAllocation,
        path: impl Into<String>,
    ) -> Result<OpenSandboxFileReading> {
        let path = path.into();
        validate_file_path(&path)?;
        let endpoint = resolve_data_endpoint(
            &self.config,
            &self.transport,
            allocation,
            OPENSANDBOX_EXECD_PORT,
        )?;
        let request_path = format!("/files/download?path={}", percent_encode(&path));
        let response = data_request(
            &self.transport,
            &endpoint,
            "GET",
            &request_path,
            BTreeMap::new(),
            Vec::new(),
        )?;
        require_success_safe(&response, "execd file download")?;
        Ok(OpenSandboxFileReading {
            sandbox_material_ref: allocation.material_ref.clone(),
            path,
            bytes: response.body,
            provenance: BTreeMap::from([
                ("provider".into(), "opensandbox:execd-filesystem".into()),
                (
                    "upstream.revision".into(),
                    OPENSANDBOX_SOURCE_REVISION.into(),
                ),
                (
                    "upstream.execd_spec_blob".into(),
                    OPENSANDBOX_EXECD_SPEC_BLOB.into(),
                ),
                ("execd.port".into(), OPENSANDBOX_EXECD_PORT.to_string()),
                ("data_plane".into(), "native-opensandbox".into()),
            ]),
        })
    }
}

fn validate_file_path(path: &str) -> Result<()> {
    if path.trim().is_empty() || !path.starts_with('/') || path.contains('\0') {
        return Err(WorkcellError::InvalidDemand(
            "OpenSandbox file reading requires a non-empty absolute path without NUL bytes".into(),
        ));
    }
    Ok(())
}

fn percent_encode(value: &str) -> String {
    let mut out = String::new();
    for byte in value.as_bytes() {
        match *byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' => {
                out.push(*byte as char)
            }
            other => out.push_str(&format!("%{other:02X}")),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Mutex};

    use epilogos_workcell_core::{HealthState, ProviderPortKind, ProviderRef};
    use serde_json::json;

    use super::*;
    use crate::{OpenSandboxHttpRequest, OpenSandboxHttpResponse};

    #[derive(Clone)]
    struct FixtureTransport {
        requests: Arc<Mutex<Vec<OpenSandboxHttpRequest>>>,
        responses: Arc<Mutex<Vec<OpenSandboxHttpResponse>>>,
    }

    impl FixtureTransport {
        fn with_responses(responses: Vec<OpenSandboxHttpResponse>) -> Self {
            Self {
                requests: Arc::new(Mutex::new(Vec::new())),
                responses: Arc::new(Mutex::new(responses.into_iter().rev().collect())),
            }
        }

        fn requests(&self) -> Vec<OpenSandboxHttpRequest> {
            self.requests.lock().unwrap().clone()
        }
    }

    impl OpenSandboxTransport for FixtureTransport {
        fn request(&self, request: OpenSandboxHttpRequest) -> Result<OpenSandboxHttpResponse> {
            self.requests.lock().unwrap().push(request);
            self.responses
                .lock()
                .unwrap()
                .pop()
                .ok_or_else(|| WorkcellError::OperationFailed("fixture response exhausted".into()))
        }
    }

    fn response(status: u16, body: Vec<u8>) -> OpenSandboxHttpResponse {
        OpenSandboxHttpResponse {
            status,
            headers: BTreeMap::new(),
            body,
        }
    }

    fn config() -> OpenSandboxConfig {
        let mut config = OpenSandboxConfig::local(
            ProviderRef::new("provider:opensandbox").unwrap(),
            "opensandbox/code-interpreter:v1.1.0",
            vec!["/opt/code-interpreter/code-interpreter.sh".into()],
        )
        .unwrap();
        config.api_key_env = None;
        config
    }

    fn allocation() -> ProviderAllocation {
        ProviderAllocation {
            provider_ref: ProviderRef::new("provider:opensandbox").unwrap(),
            port: ProviderPortKind::Execution,
            material_ref: "sbx_file_fixture".into(),
            health: HealthState::Healthy,
            properties: BTreeMap::new(),
            provenance: BTreeMap::new(),
        }
    }

    #[test]
    fn file_read_uses_native_execd_endpoint_without_workcell_proxying() {
        let endpoint_body = serde_json::to_vec(&json!({
            "endpoint": "http://execd.fixture:44772",
            "headers": {"X-EXECD-TOKEN": "provider-secret"}
        }))
        .unwrap();
        let transport = FixtureTransport::with_responses(vec![
            response(200, endpoint_body),
            response(200, b"native file bytes\n".to_vec()),
        ]);
        let inspect = transport.clone();
        let data_plane = OpenSandboxDataPlane::new(config(), transport).unwrap();

        let reading = data_plane
            .read_file(&allocation(), "/workspace/ProjectCentral/now/NOW.md")
            .unwrap();

        assert_eq!(reading.bytes, b"native file bytes\n");
        assert_eq!(reading.sandbox_material_ref, "sbx_file_fixture");
        assert!(!format!("{reading:?}").contains("provider-secret"));
        let requests = inspect.requests();
        assert_eq!(requests.len(), 2);
        assert!(requests[0]
            .url
            .contains("/sandboxes/sbx_file_fixture/endpoints/44772"));
        assert_eq!(
            requests[1].url,
            "http://execd.fixture:44772/files/download?path=%2Fworkspace%2FProjectCentral%2Fnow%2FNOW.md"
        );
        assert_eq!(
            requests[1].headers.get("X-EXECD-TOKEN"),
            Some(&"provider-secret".to_string())
        );
    }

    #[test]
    fn file_surface_requires_an_absolute_target_native_path() {
        let transport = FixtureTransport::with_responses(Vec::new());
        let data_plane = OpenSandboxDataPlane::new(config(), transport).unwrap();
        assert!(data_plane.read_file(&allocation(), "relative/path").is_err());
    }
}
