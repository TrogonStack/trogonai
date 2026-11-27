use axum::http::{HeaderMap, HeaderName, HeaderValue};
use trogon_sandbox_contract::PortAccess;

/// Where requests for a published port go: the worker's `access` URL when it supplied one, otherwise plain HTTP to
/// the bare endpoint.
pub(crate) fn upstream_base(endpoint: &str, access: Option<&PortAccess>) -> String {
    match access {
        Some(access) => access.base_url.trim_end_matches('/').to_string(),
        None => format!("http://{endpoint}"),
    }
}

/// The headers the worker requires on every request to the port. Entries that are not valid headers are skipped.
pub(crate) fn access_headers(access: Option<&PortAccess>) -> HeaderMap {
    let mut out = HeaderMap::new();
    for (name, value) in access.iter().flat_map(|access| access.headers.iter()) {
        if let (Ok(name), Ok(value)) = (HeaderName::from_bytes(name.as_bytes()), HeaderValue::from_str(value)) {
            out.insert(name, value);
        }
    }
    out
}
