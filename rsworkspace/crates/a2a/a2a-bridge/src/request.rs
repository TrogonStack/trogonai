use axum::http::{HeaderMap, Method, Uri};
use bytes::Bytes;

/// An HTTP request as it reached the bridge: the parts an AAuth signature
/// covers alongside the headers and body the JSON-RPC handler reads, so both
/// see the same request.
pub struct InboundRequest {
    method: Method,
    uri: Uri,
    headers: HeaderMap,
    body: Bytes,
}

impl InboundRequest {
    #[must_use]
    pub fn new(method: Method, uri: Uri, headers: HeaderMap, body: Bytes) -> Self {
        Self {
            method,
            uri,
            headers,
            body,
        }
    }

    #[must_use]
    pub fn method(&self) -> &Method {
        &self.method
    }

    /// The request path without its query, as RFC 9421 `@path` covers it.
    #[must_use]
    pub fn path(&self) -> &str {
        self.uri.path()
    }

    #[must_use]
    pub fn headers(&self) -> &HeaderMap {
        &self.headers
    }

    #[must_use]
    pub fn body(&self) -> &Bytes {
        &self.body
    }
}
