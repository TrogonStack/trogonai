//! A2A spec request/response header negotiation.
//!
//! Implements the protocol-level header surface from
//! <https://a2a-protocol.org/latest/specification/>:
//! - `A2A-Version` request parsing + response emission + version negotiation.
//! - `A2A-Extensions` request parsing + response echo + required-extension gating.
//! - `application/a2a+json` IANA media type accept + emit on JSON responses.
//!
//! Failures map to the spec-mandated JSON-RPC error codes
//! ([`VERSION_NOT_SUPPORTED`], [`EXTENSION_SUPPORT_REQUIRED`]) on `HTTP 400`
//! responses.

use std::collections::BTreeSet;
use std::sync::Arc;

use a2a_nats::error::{EXTENSION_SUPPORT_REQUIRED, VERSION_NOT_SUPPORTED};
use axum::extract::{Request, State};
use axum::http::{HeaderName, HeaderValue, StatusCode, header};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use serde_json::json;

pub const A2A_VERSION_HEADER: HeaderName = HeaderName::from_static("a2a-version");
pub const A2A_EXTENSIONS_HEADER: HeaderName = HeaderName::from_static("a2a-extensions");
pub const A2A_MEDIA_TYPE: &str = "application/a2a+json";

/// Default A2A protocol version this server speaks when the client omits the header.
pub const DEFAULT_A2A_VERSION: &str = "0.3.0";

#[derive(Clone, Debug)]
pub struct SpecNegotiationConfig {
    pub supported_versions: BTreeSet<String>,
    pub default_version: String,
    pub supported_extensions: BTreeSet<String>,
}

impl Default for SpecNegotiationConfig {
    fn default() -> Self {
        let mut supported_versions = BTreeSet::new();
        supported_versions.insert(DEFAULT_A2A_VERSION.to_string());
        Self {
            supported_versions,
            default_version: DEFAULT_A2A_VERSION.to_string(),
            supported_extensions: BTreeSet::new(),
        }
    }
}

impl SpecNegotiationConfig {
    pub fn new(default_version: impl Into<String>) -> Self {
        let default_version = default_version.into();
        let mut supported_versions = BTreeSet::new();
        supported_versions.insert(default_version.clone());
        Self {
            supported_versions,
            default_version,
            supported_extensions: BTreeSet::new(),
        }
    }

    pub fn with_version(mut self, version: impl Into<String>) -> Self {
        self.supported_versions.insert(version.into());
        self
    }

    pub fn with_extension(mut self, ext: impl Into<String>) -> Self {
        self.supported_extensions.insert(ext.into());
        self
    }
}

/// One requested extension, parsed from a single comma-delimited entry in `A2A-Extensions`.
#[derive(Clone, Debug, PartialEq, Eq)]
struct RequestedExtension {
    uri: String,
    required: bool,
}

impl RequestedExtension {
    fn parse(raw: &str) -> Option<Self> {
        let trimmed = raw.trim();
        if trimmed.is_empty() {
            return None;
        }
        // RFC-style optional marker: a trailing `;q=0` is treated as "optional".
        // We also accept a `?` prefix as an alternative optional marker that
        // is easy to author by hand.
        // Skip tokens whose URI segment ends up empty after stripping the
        // optional `?` prefix or `;q=…` params — e.g. a bare `"?"` or a `;q=0`
        // would otherwise be implicitly negotiated as an empty-URI extension.
        if let Some(stripped) = trimmed.strip_prefix('?') {
            let uri = stripped.trim();
            if uri.is_empty() {
                return None;
            }
            Some(Self {
                uri: uri.to_string(),
                required: false,
            })
        } else if let Some((uri, params)) = trimmed.split_once(';') {
            let uri = uri.trim();
            if uri.is_empty() {
                return None;
            }
            let optional = params.split(';').any(|p| {
                let p = p.trim().to_ascii_lowercase();
                p == "q=0" || p == "q=0.0" || p == "optional"
            });
            Some(Self {
                uri: uri.to_string(),
                required: !optional,
            })
        } else {
            Some(Self {
                uri: trimmed.to_string(),
                required: true,
            })
        }
    }
}

fn parse_extensions(raw: &str) -> Vec<RequestedExtension> {
    raw.split(',').filter_map(RequestedExtension::parse).collect()
}

/// Result of header negotiation that downstream handlers can read off the request extensions.
#[derive(Clone, Debug, Default)]
pub struct NegotiatedSpec {
    pub version: String,
    pub activated_extensions: Vec<String>,
}

fn json_rpc_error(code: i32, message: &str) -> Response {
    let body = json!({
        "jsonrpc": "2.0",
        "id": serde_json::Value::Null,
        "error": { "code": code, "message": message },
    });
    let mut response = (StatusCode::BAD_REQUEST, axum::Json(body)).into_response();
    response
        .headers_mut()
        .insert(header::CONTENT_TYPE, HeaderValue::from_static(A2A_MEDIA_TYPE));
    response
}

/// Axum middleware that negotiates A2A spec headers + media type on every request/response.
pub async fn negotiate(State(config): State<Arc<SpecNegotiationConfig>>, mut request: Request, next: Next) -> Response {
    let headers = request.headers();

    let requested_version = headers
        .get(&A2A_VERSION_HEADER)
        .and_then(|v| v.to_str().ok())
        .map(|s| s.trim().to_string());

    let served_version = match requested_version {
        Some(v) if v.is_empty() => config.default_version.clone(),
        Some(v) => {
            if config.supported_versions.contains(&v) {
                v
            } else {
                return json_rpc_error(
                    VERSION_NOT_SUPPORTED,
                    &format!("A2A protocol version `{v}` is not supported by this agent"),
                );
            }
        }
        None => config.default_version.clone(),
    };

    let requested_extensions = headers
        .get(&A2A_EXTENSIONS_HEADER)
        .and_then(|v| v.to_str().ok())
        .map(parse_extensions)
        .unwrap_or_default();

    let mut activated = Vec::new();
    for ext in &requested_extensions {
        if config.supported_extensions.contains(&ext.uri) {
            activated.push(ext.uri.clone());
        } else if ext.required {
            return json_rpc_error(
                EXTENSION_SUPPORT_REQUIRED,
                &format!("required A2A extension `{}` is not supported by this agent", ext.uri),
            );
        }
    }

    request.extensions_mut().insert(NegotiatedSpec {
        version: served_version.clone(),
        activated_extensions: activated.clone(),
    });

    let mut response = next.run(request).await;

    if let Ok(value) = HeaderValue::from_str(&served_version) {
        response.headers_mut().insert(&A2A_VERSION_HEADER, value);
    }
    if !activated.is_empty()
        && let Ok(value) = HeaderValue::from_str(&activated.join(", "))
    {
        response.headers_mut().insert(&A2A_EXTENSIONS_HEADER, value);
    }

    let should_set_media_type = response
        .headers()
        .get(header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .map(|s| s.starts_with("application/json") || s.starts_with(A2A_MEDIA_TYPE))
        .unwrap_or(false);
    if should_set_media_type {
        response
            .headers_mut()
            .insert(header::CONTENT_TYPE, HeaderValue::from_static(A2A_MEDIA_TYPE));
    }

    response
}

/// Helper that returns a default-built shared config wrapped in an Arc.
pub fn default_config() -> Arc<SpecNegotiationConfig> {
    Arc::new(SpecNegotiationConfig::default())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_optional_extension_with_question_prefix() {
        let ext = RequestedExtension::parse("?https://example.com/ext/foo").unwrap();
        assert_eq!(ext.uri, "https://example.com/ext/foo");
        assert!(!ext.required);
    }

    #[test]
    fn parses_required_extension_default() {
        let ext = RequestedExtension::parse("https://example.com/ext/bar").unwrap();
        assert_eq!(ext.uri, "https://example.com/ext/bar");
        assert!(ext.required);
    }

    #[test]
    fn parses_q_zero_as_optional() {
        let ext = RequestedExtension::parse("https://example.com/ext/baz;q=0").unwrap();
        assert_eq!(ext.uri, "https://example.com/ext/baz");
        assert!(!ext.required);
    }

    #[test]
    fn parse_extensions_splits_on_comma_and_ignores_empty() {
        let exts = parse_extensions("https://a/, ,?https://b/");
        assert_eq!(exts.len(), 2);
        assert_eq!(exts[0].uri, "https://a/");
        assert!(exts[0].required);
        assert_eq!(exts[1].uri, "https://b/");
        assert!(!exts[1].required);
    }

    #[test]
    fn default_config_has_one_version() {
        let cfg = SpecNegotiationConfig::default();
        assert!(cfg.supported_versions.contains(DEFAULT_A2A_VERSION));
        assert_eq!(cfg.default_version, DEFAULT_A2A_VERSION);
        assert!(cfg.supported_extensions.is_empty());
    }

    #[test]
    fn with_version_appends_to_supported_set() {
        let cfg = SpecNegotiationConfig::new("0.3.0").with_version("0.2.0");
        assert!(cfg.supported_versions.contains("0.3.0"));
        assert!(cfg.supported_versions.contains("0.2.0"));
    }

    #[test]
    fn with_extension_appends() {
        let cfg = SpecNegotiationConfig::default().with_extension("https://example.com/ext/x");
        assert!(cfg.supported_extensions.contains("https://example.com/ext/x"));
    }
}
