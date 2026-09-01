use std::collections::{BTreeMap, BTreeSet};

use epilogos_workcell_core::{ExternalRef, ProviderAllocation, Result, WorkcellError, WorldRef};
use serde_json::{json, Value};

use super::{
    client::{data_request, require_success_safe, resolve_data_endpoint},
    protocol::{
        OpenSandboxConfig, OpenSandboxTransport, OPENSANDBOX_EXECD_PORT,
        OPENSANDBOX_SOURCE_REVISION,
    },
};

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct OpenSandboxProjectWorldFile {
    /// Path relative to the Project root, for example `src/main.rs` or
    /// `ProjectCentral/now/NOW.md`.
    pub path: String,
    pub content: String,
}

impl OpenSandboxProjectWorldFile {
    pub fn new(path: impl Into<String>, content: impl Into<String>) -> Result<Self> {
        let file = Self {
            path: path.into(),
            content: content.into(),
        };
        validate_relative_path(&file.path)?;
        Ok(file)
    }
}

/// Authored Project/World material supplied by Central (or another source
/// authority) for provider-side staging. The source identity and revision are
/// retained in the receipt; the sandbox path never becomes source authority.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct OpenSandboxProjectWorldSpec {
    pub world_ref: WorldRef,
    pub project_ref: ExternalRef,
    pub source_ref: ExternalRef,
    pub source_revision: String,
    pub source_authority: ExternalRef,
    pub material_root: String,
    pub files: Vec<OpenSandboxProjectWorldFile>,
}

impl OpenSandboxProjectWorldSpec {
    pub fn validate(&self) -> Result<()> {
        if !self.material_root.starts_with('/') || self.material_root.ends_with('/') {
            return Err(WorkcellError::InvalidDemand(
                "OpenSandbox Project World material_root must be an absolute non-trailing-slash path"
                    .into(),
            ));
        }
        if self.source_revision.trim().is_empty() {
            return Err(WorkcellError::InvalidDemand(
                "Project World source_revision must not be empty".into(),
            ));
        }
        if self.files.is_empty() {
            return Err(WorkcellError::InvalidDemand(
                "Project World materialisation requires at least one file".into(),
            ));
        }

        let mut paths = BTreeSet::new();
        let mut ordinary_source = false;
        let mut project_central = false;
        for file in &self.files {
            validate_relative_path(&file.path)?;
            if !paths.insert(file.path.clone()) {
                return Err(WorkcellError::InvalidDemand(format!(
                    "Project World file `{}` appears more than once",
                    file.path
                )));
            }
            if file.path == "Control" || file.path.starts_with("Control/") {
                return Err(WorkcellError::InvalidDemand(
                    "root Control material requires an explicit eligible binding and cannot be staged as Project source"
                        .into(),
                ));
            }
            if file.path.starts_with("ProjectCentral/") {
                project_central = true;
            } else {
                ordinary_source = true;
            }
        }
        if !ordinary_source || !project_central {
            return Err(WorkcellError::InvalidDemand(
                "Central Project World staging requires both ordinary Project source and ProjectCentral material"
                    .into(),
            ));
        }
        Ok(())
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct OpenSandboxProjectWorldFileReceipt {
    pub source_relative_path: String,
    pub material_path: String,
    pub byte_len: usize,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct OpenSandboxProjectWorldReceipt {
    pub world_ref: WorldRef,
    pub project_ref: ExternalRef,
    pub source_ref: ExternalRef,
    pub source_revision: String,
    pub source_authority: ExternalRef,
    pub sandbox_material_ref: String,
    pub material_root: String,
    pub files: Vec<OpenSandboxProjectWorldFileReceipt>,
    pub source_authority_preserved: bool,
    pub provenance: BTreeMap<String, String>,
}

pub struct OpenSandboxProjectWorldMaterialiser<T> {
    config: OpenSandboxConfig,
    transport: T,
}

impl<T> OpenSandboxProjectWorldMaterialiser<T>
where
    T: OpenSandboxTransport,
{
    pub fn new(config: OpenSandboxConfig, transport: T) -> Result<Self> {
        config.validate()?;
        Ok(Self { config, transport })
    }

    /// Stage and verify one authored Project World through OpenSandbox's native
    /// execd data plane. The returned receipt is material provenance only.
    pub fn materialise(
        &self,
        allocation: &ProviderAllocation,
        spec: &OpenSandboxProjectWorldSpec,
    ) -> Result<OpenSandboxProjectWorldReceipt> {
        spec.validate()?;
        let endpoint = resolve_data_endpoint(
            &self.config,
            &self.transport,
            allocation,
            OPENSANDBOX_EXECD_PORT,
        )?;

        let command = staging_command(spec)?;
        let body = serde_json::to_vec(&json!({
            "command": command,
            "background": false,
        }))
        .map_err(|error| {
            WorkcellError::OperationFailed(format!(
                "encode OpenSandbox Project World staging command: {error}"
            ))
        })?;
        let response = data_request(
            &self.transport,
            &endpoint,
            "POST",
            "/command",
            BTreeMap::from([("Accept".into(), "text/event-stream".into())]),
            body,
        )?;
        require_success_safe(&response, "Project World staging command")?;
        require_command_completion(&response.body)?;

        let mut receipts = Vec::with_capacity(spec.files.len());
        for file in &spec.files {
            let material_path = material_path(&spec.material_root, &file.path);
            let query = format!("/files/download?path={}", percent_encode(&material_path));
            let response = data_request(
                &self.transport,
                &endpoint,
                "GET",
                &query,
                BTreeMap::new(),
                Vec::new(),
            )?;
            require_success_safe(&response, "verify staged Project World file")?;
            if response.body != file.content.as_bytes() {
                return Err(WorkcellError::OperationFailed(format!(
                    "staged Project World file `{}` does not match its authored source material",
                    file.path
                )));
            }
            receipts.push(OpenSandboxProjectWorldFileReceipt {
                source_relative_path: file.path.clone(),
                material_path,
                byte_len: file.content.len(),
            });
        }

        Ok(OpenSandboxProjectWorldReceipt {
            world_ref: spec.world_ref.clone(),
            project_ref: spec.project_ref.clone(),
            source_ref: spec.source_ref.clone(),
            source_revision: spec.source_revision.clone(),
            source_authority: spec.source_authority.clone(),
            sandbox_material_ref: allocation.material_ref.clone(),
            material_root: spec.material_root.clone(),
            files: receipts,
            source_authority_preserved: true,
            provenance: BTreeMap::from([
                ("provider".into(), "opensandbox:execd".into()),
                (
                    "upstream.revision".into(),
                    OPENSANDBOX_SOURCE_REVISION.into(),
                ),
                ("execd.port".into(), OPENSANDBOX_EXECD_PORT.to_string()),
                ("workspace.realisation".into(), "staged-copy".into()),
                ("source.authority".into(), spec.source_authority.to_string()),
            ]),
        })
    }
}

fn validate_relative_path(path: &str) -> Result<()> {
    if path.trim().is_empty() || path.starts_with('/') || path.ends_with('/') {
        return Err(WorkcellError::InvalidDemand(
            "Project World file path must be a non-empty relative file path".into(),
        ));
    }
    if path
        .split('/')
        .any(|part| part.is_empty() || part == "." || part == "..")
    {
        return Err(WorkcellError::InvalidDemand(
            "Project World file path must not contain empty, `.` or `..` segments".into(),
        ));
    }
    Ok(())
}

fn material_path(root: &str, relative: &str) -> String {
    format!("{}/{}", root.trim_end_matches('/'), relative)
}

fn staging_command(spec: &OpenSandboxProjectWorldSpec) -> Result<String> {
    let mut directories = BTreeSet::new();
    directories.insert(spec.material_root.clone());
    for file in &spec.files {
        let path = material_path(&spec.material_root, &file.path);
        if let Some((parent, _)) = path.rsplit_once('/') {
            directories.insert(parent.into());
        }
    }

    let mut commands = vec![format!(
        "mkdir -p {}",
        directories
            .iter()
            .map(|value| shell_quote(value))
            .collect::<Vec<_>>()
            .join(" ")
    )];
    for file in &spec.files {
        let path = material_path(&spec.material_root, &file.path);
        commands.push(format!(
            "printf '%s' {} > {}",
            shell_quote(&file.content),
            shell_quote(&path)
        ));
    }
    Ok(commands.join(" && "))
}

fn shell_quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\"'\"'"))
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

fn require_command_completion(body: &[u8]) -> Result<()> {
    let text = std::str::from_utf8(body).map_err(|error| {
        WorkcellError::OperationFailed(format!(
            "OpenSandbox Project World command stream is not UTF-8: {error}"
        ))
    })?;
    let mut complete = false;
    for line in text.lines() {
        let Some(data) = line.strip_prefix("data:") else {
            continue;
        };
        let data = data.trim();
        if data.is_empty() || data == "[DONE]" {
            continue;
        }
        let value: Value = serde_json::from_str(data).map_err(|error| {
            WorkcellError::OperationFailed(format!(
                "decode OpenSandbox Project World command event: {error}"
            ))
        })?;
        match value.get("type").and_then(Value::as_str) {
            Some("execution_complete") => complete = true,
            Some("error") => {
                return Err(WorkcellError::OperationFailed(
                    "OpenSandbox Project World staging command reported an execution error".into(),
                ))
            }
            _ => {}
        }
    }
    if !complete {
        return Err(WorkcellError::OperationFailed(
            "OpenSandbox Project World staging command did not report completion".into(),
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Mutex};

    use epilogos_workcell_core::{HealthState, ProviderPortKind, ProviderRef};

    use super::*;
    use crate::{OpenSandboxHttpRequest, OpenSandboxHttpResponse};

    #[derive(Clone, Default)]
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

    fn response(status: u16, body: impl Into<Vec<u8>>) -> OpenSandboxHttpResponse {
        OpenSandboxHttpResponse {
            status,
            headers: BTreeMap::new(),
            body: body.into(),
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
            material_ref: "sbx_project_world_fixture".into(),
            health: HealthState::Healthy,
            properties: BTreeMap::new(),
            provenance: BTreeMap::new(),
        }
    }

    fn specimen() -> OpenSandboxProjectWorldSpec {
        OpenSandboxProjectWorldSpec {
            world_ref: WorldRef::new("world:project/specimen").unwrap(),
            project_ref: ExternalRef::new("project:specimen").unwrap(),
            source_ref: ExternalRef::new("central:work/specimen").unwrap(),
            source_revision: "git:abc123".into(),
            source_authority: ExternalRef::new("central:source-authority").unwrap(),
            material_root: "/workspace/Specimen".into(),
            files: vec![
                OpenSandboxProjectWorldFile::new("README.md", "# Specimen\n").unwrap(),
                OpenSandboxProjectWorldFile::new("src/main.rs", "fn main() {}\n").unwrap(),
                OpenSandboxProjectWorldFile::new(
                    "ProjectCentral/user/README.md",
                    "human-authored context\n",
                )
                .unwrap(),
                OpenSandboxProjectWorldFile::new(
                    "ProjectCentral/agents/governance/README.md",
                    "agent governance\n",
                )
                .unwrap(),
                OpenSandboxProjectWorldFile::new(
                    "ProjectCentral/agents/wiki/wiki.json",
                    "{\"version\":1}\n",
                )
                .unwrap(),
                OpenSandboxProjectWorldFile::new(
                    "ProjectCentral/now/NOW.md",
                    "current working field\n",
                )
                .unwrap(),
                OpenSandboxProjectWorldFile::new(
                    "ProjectCentral/relations/world.md",
                    "world relations\n",
                )
                .unwrap(),
            ],
        }
    }

    #[test]
    fn central_project_world_is_staged_and_verified_without_source_authority_collapse() {
        let spec = specimen();
        let endpoint_response = response(
            200,
            serde_json::to_vec(&json!({
                "endpoint": "http://execd.fixture:44772",
                "headers": {"X-EXECD-TOKEN": "provider-secret"}
            }))
            .unwrap(),
        );
        let command_response =
            response(200, b"data: {\"type\":\"execution_complete\"}\n\n".to_vec());
        let mut responses = vec![endpoint_response, command_response];
        responses.extend(
            spec.files
                .iter()
                .map(|file| response(200, file.content.as_bytes().to_vec())),
        );
        let transport = FixtureTransport::with_responses(responses);
        let materialiser =
            OpenSandboxProjectWorldMaterialiser::new(config(), transport.clone()).unwrap();

        let receipt = materialiser.materialise(&allocation(), &spec).unwrap();

        assert_eq!(receipt.world_ref, spec.world_ref);
        assert_eq!(receipt.source_authority, spec.source_authority);
        assert!(receipt.source_authority_preserved);
        assert_eq!(receipt.files.len(), spec.files.len());
        assert!(receipt
            .files
            .iter()
            .any(|file| file.source_relative_path == "README.md"));
        assert!(receipt
            .files
            .iter()
            .any(|file| file.source_relative_path == "ProjectCentral/agents/wiki/wiki.json"));
        assert!(!receipt
            .files
            .iter()
            .any(|file| file.source_relative_path.starts_with("Control/")));

        let requests = transport.requests();
        assert_eq!(requests[1].url, "http://execd.fixture:44772/command");
        assert_eq!(
            requests[1].headers.get("X-EXECD-TOKEN"),
            Some(&"provider-secret".to_string())
        );
        assert!(requests[2]
            .url
            .contains("/files/download?path=%2Fworkspace%2FSpecimen%2FREADME.md"));
    }

    #[test]
    fn root_control_material_is_never_implicitly_staged() {
        let mut spec = specimen();
        spec.files.push(
            OpenSandboxProjectWorldFile::new("Control/user/private.md", "private\n").unwrap(),
        );
        assert!(matches!(
            spec.validate(),
            Err(WorkcellError::InvalidDemand(_))
        ));
    }
}
