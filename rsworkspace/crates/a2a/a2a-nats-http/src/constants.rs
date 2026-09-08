use axum::http::HeaderName;

pub const A2A_VERSION_HEADER: HeaderName = HeaderName::from_static("a2a-version");
pub const A2A_EXTENSIONS_HEADER: HeaderName = HeaderName::from_static("a2a-extensions");
pub const A2A_MEDIA_TYPE: &str = "application/a2a+json";

/// Default A2A protocol version this server speaks when the client omits the header.
pub const DEFAULT_A2A_VERSION: &str = "0.3.0";

pub(crate) const DEFAULT_BIND: &str = "0.0.0.0:8080";
pub(crate) const ENV_HTTP_BIND: &str = "A2A_HTTP_BIND";
pub(crate) const ENV_AGENT_ID: &str = "A2A_AGENT_ID";
pub(crate) const ENV_USE_GATEWAY: &str = "A2A_USE_GATEWAY";
pub(crate) const ENV_GATEWAY_CALLER_JWT: &str = "A2A_GATEWAY_CALLER_JWT";
