use std::{collections::BTreeMap, env};

use epilogos_workcell_core::{ProviderAllocation, Result, WorkcellError};
use serde_json::Value;

use super::protocol::{
    OpenSandboxConfig, OpenSandboxHttpRequest, OpenSandboxHttpResponse, OpenSandboxTransport,
    OPENSANDBOX_API_KEY_HEADER,
};

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct DataEndpoint {
    pub base_url: String,
    pub headers: BTreeMap<String, String>,
}

pub(crate) fn lifecycle_request<T: OpenSandboxTransport>(
    config: &OpenSandboxConfig,
    transport: &T,
    method: &str,
    path: &str,
    body: Vec<u8>,
) -> Result<OpenSandboxHttpResponse> {
    let mut headers = BTreeMap::new();
    if let Some(env_name) = &config.api_key_env {
        let value = env::var(env_name).map_err(|_| {
            WorkcellError::Unavailable(format!(
                "OpenSandbox API key environment `{env_name}` is not available"
            ))
        })?;
        headers.insert(OPENSANDBOX_API_KEY_HEADER.into(), value);
    }
    transport.request(OpenSandboxHttpRequest {
        method: method.into(),
        url: join_url(&config.lifecycle_base_url, path),
        headers,
        body,
    })
}

pub(crate) fn resolve_data_endpoint<T: OpenSandboxTransport>(
    config: &OpenSandboxConfig,
    transport: &T,
    allocation: &ProviderAllocation,
    port: u16,
) -> Result<DataEndpoint> {
    if allocation.provider_ref != config.provider_ref || allocation.material_ref.trim().is_empty() {
        return Err(WorkcellError::OperationFailed(
            "OpenSandbox material endpoint requested for a foreign/empty allocation".into(),
        ));
    }
    let path = format!(
        "/sandboxes/{}/endpoints/{port}?use_server_proxy={}",
        allocation.material_ref, config.use_server_proxy
    );
    let response = lifecycle_request(config, transport, "GET", &path, Vec::new())?;
    require_success_safe(&response, "resolve data-plane endpoint")?;
    let value: Value = serde_json::from_slice(&response.body).map_err(|error| {
        WorkcellError::OperationFailed(format!(
            "decode OpenSandbox endpoint response: {error}"
        ))
    })?;
    let object = value.as_object().ok_or_else(|| {
        WorkcellError::OperationFailed("OpenSandbox endpoint response must be an object".into())
    })?;
    let endpoint = object
        .get("endpoint")
        .and_then(Value::as_str)
        .filter(|value| !value.trim().is_empty())
        .ok_or_else(|| {
            WorkcellError::OperationFailed(
                "OpenSandbox endpoint response requires a non-empty endpoint".into(),
            )
        })?;
    let headers = match object.get("headers") {
        None | Some(Value::Null) => BTreeMap::new(),
        Some(Value::Object(values)) => values
            .iter()
            .map(|(name, value)| {
                value
                    .as_str()
                    .map(|value| (name.clone(), value.to_owned()))
                    .ok_or_else(|| {
                        WorkcellError::OperationFailed(
                            "OpenSandbox endpoint header values must be strings".into(),
                        )
                    })
            })
            .collect::<Result<BTreeMap<_, _>>>()?,
        Some(_) => {
            return Err(WorkcellError::OperationFailed(
                "OpenSandbox endpoint headers must be an object".into(),
            ))
        }
    };
    Ok(DataEndpoint {
        base_url: normalize_endpoint_url(endpoint),
        headers,
    })
}

pub(crate) fn data_request<T: OpenSandboxTransport>(
    transport: &T,
    endpoint: &DataEndpoint,
    method: &str,
    path: &str,
    mut headers: BTreeMap<String, String>,
    body: Vec<u8>,
) -> Result<OpenSandboxHttpResponse> {
    for (name, value) in &endpoint.headers {
        headers.entry(name.clone()).or_insert_with(|| value.clone());
    }
    transport.request(OpenSandboxHttpRequest {
        method: method.into(),
        url: join_url(&endpoint.base_url, path),
        headers,
        body,
    })
}

pub(crate) fn require_success_safe(
    response: &OpenSandboxHttpResponse,
    operation: &str,
) -> Result<()> {
    if (200..300).contains(&response.status) {
        return Ok(());
    }
    let message = format!(
        "OpenSandbox `{operation}` returned HTTP {}",
        response.status
    );
    match response.status {
        404 => Err(WorkcellError::NotFound(message)),
        409 | 429 => Err(WorkcellError::Unavailable(message)),
        _ => Err(WorkcellError::OperationFailed(message)),
    }
}

pub(crate) fn join_url(base: &str, path: &str) -> String {
    format!(
        "{}/{}",
        base.trim_end_matches('/'),
        path.trim_start_matches('/')
    )
}

fn normalize_endpoint_url(endpoint: &str) -> String {
    if endpoint.starts_with("http://") || endpoint.starts_with("https://") {
        endpoint.trim_end_matches('/').into()
    } else {
        format!("http://{}", endpoint.trim_end_matches('/'))
    }
}
