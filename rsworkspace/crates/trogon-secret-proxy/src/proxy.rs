//! Axum HTTP proxy server.
//!
//! Receives HTTP calls from services (with `Authorization: Bearer tok_...`),
//! wraps them as [`OutboundHttpRequest`] messages, publishes to JetStream,
//! and waits on a Core NATS reply subject for the worker's response.

use std::sync::Arc;
use std::time::Duration;

use async_nats::jetstream;
use axum::body::Body;
use axum::extract::{Path, Request, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::Response;
use axum::routing::any;
use axum::Router;
use bytes::Bytes;
use futures_util::StreamExt;
use uuid::Uuid;

use crate::messages::{OutboundHttpRequest, OutboundHttpResponse};
use crate::provider;
use crate::subjects;
use trogon_nats::headers_with_trace_context;

#[derive(Clone)]
pub struct ProxyState {
    pub nats: async_nats::Client,
    pub jetstream: Arc<jetstream::Context>,
    pub prefix: String,
    pub outbound_subject: String,
    pub worker_timeout: Duration,
    /// Overrides the AI-provider base URL for all requests.
    /// `None` in production; set to a mock server URL in integration tests.
    pub base_url_override: Option<String>,
}

/// Build the axum router for the HTTP proxy.
pub fn router(state: ProxyState) -> Router {
    Router::new()
        .route("/{provider}/{*path}", any(handle_request))
        .with_state(state)
}

async fn handle_request(
    State(state): State<ProxyState>,
    Path((provider, path)): Path<(String, String)>,
    req: Request,
) -> Result<Response<Body>, ProxyError> {
    // An empty path (e.g. from `GET /anthropic/`) has no meaningful endpoint to
    // forward to.  Reject immediately with 400 rather than publishing to
    // JetStream and burning a worker slot on a request that will always fail.
    if path.is_empty() {
        return Err(ProxyError::EmptyPath);
    }

    let base: String = match &state.base_url_override {
        Some(override_url) => override_url.clone(),
        None => provider::base_url(&provider)
            .ok_or_else(|| ProxyError::UnknownProvider(provider.clone()))?
            .to_string(),
    };
    // Strip any trailing slash so `format!("{}/{}", base, path)` never produces
    // a double slash regardless of how base_url_override is configured.
    let base = base.trim_end_matches('/');

    let query = req
        .uri()
        .query()
        .map(|q| format!("?{}", q))
        .unwrap_or_default();
    let url = format!("{}/{}{}", base, path, query);

    let method = req.method().to_string();
    let req_headers = req.headers().clone();
    let body_bytes: Bytes = axum::body::to_bytes(req.into_body(), usize::MAX)
        .await
        .map_err(|e| ProxyError::ReadBody(e.to_string()))?;

    // Hop-by-hop headers must not be forwarded to the upstream AI provider.
    // RFC 7230 §6.1 — these headers are meaningful only for a single transport
    // hop and must be stripped by any intermediary (proxy).
    const HOP_BY_HOP: &[&str] = &[
        "connection",
        "keep-alive",
        "proxy-authenticate",
        "proxy-authorization",
        "te",
        "trailers",
        "transfer-encoding",
        "upgrade",
    ];

    let headers: Vec<(String, String)> = req_headers
        .iter()
        .filter_map(|(k, v)| {
            let key = k.as_str();
            if HOP_BY_HOP.contains(&key) {
                return None;
            }
            v.to_str()
                .ok()
                .map(|v_str| (key.to_string(), v_str.to_string()))
        })
        .collect();

    let correlation_id = Uuid::new_v4().to_string();
    let reply_subject = subjects::reply(&state.prefix, &correlation_id);

    let message = OutboundHttpRequest {
        method,
        url,
        headers,
        body: body_bytes.to_vec(),
        reply_to: reply_subject.clone(),
        idempotency_key: correlation_id.clone(),
    };

    // Subscribe to reply subject on Core NATS before publishing.
    let mut reply_sub = state
        .nats
        .subscribe(reply_subject.clone())
        .await
        .map_err(|e| ProxyError::NatsSubscribe(e.to_string()))?;

    // Publish OutboundHttpRequest to JetStream, injecting the current trace context
    // into NATS headers so the worker can continue the distributed trace.
    //
    // `publish_with_headers` returns a `PubAckFuture` that must be awaited to
    // confirm the JetStream server has durably stored the message in the stream.
    // Without this second `.await` the durability guarantee of JetStream is lost:
    // the message may be silently dropped if no stream covers the subject, and
    // the proxy would then wait forever for a worker reply that will never come.
    let payload = serde_json::to_vec(&message)
        .map_err(|e| ProxyError::Serialize(e.to_string()))?;

    let mut nats_headers = headers_with_trace_context();
    nats_headers.insert("Reply-To", reply_subject.as_str());
    state
        .jetstream
        .publish_with_headers(state.outbound_subject.clone(), nats_headers, payload.into())
        .await
        .map_err(|e| ProxyError::NatsPublish(e.to_string()))?
        .await
        .map_err(|e| ProxyError::NatsPublish(e.to_string()))?;

    tracing::debug!(
        correlation_id = %correlation_id,
        provider = %provider,
        "Published outbound request to JetStream, awaiting reply"
    );

    // Wait for worker reply via Core NATS.
    let reply_msg = tokio::time::timeout(state.worker_timeout, reply_sub.next())
        .await
        .map_err(|_| ProxyError::Timeout {
            correlation_id: correlation_id.clone(),
        })?
        .ok_or_else(|| ProxyError::ReplyChannelClosed)?;

    let proxy_response: OutboundHttpResponse = serde_json::from_slice(&reply_msg.payload)
        .map_err(|e| ProxyError::Deserialize(e.to_string()))?;

    let status = StatusCode::from_u16(proxy_response.status)
        .unwrap_or(StatusCode::INTERNAL_SERVER_ERROR);

    if let Some(err) = proxy_response.error {
        // Use the worker's status if it is already an error code (4xx/5xx),
        // otherwise fall back to 502 to avoid leaking a spurious 2xx.
        let error_status = if status.is_client_error() || status.is_server_error() {
            status
        } else {
            StatusCode::BAD_GATEWAY
        };
        tracing::warn!(
            correlation_id = %correlation_id,
            status = %error_status,
            error = %err,
            "Worker reported an error"
        );
        return Ok(Response::builder()
            .status(error_status)
            .body(Body::from(err))
            .unwrap());
    }

    let mut response_headers = HeaderMap::new();
    for (k, v) in &proxy_response.headers {
        if let (Ok(name), Ok(value)) = (
            k.parse::<axum::http::HeaderName>(),
            v.parse::<axum::http::HeaderValue>(),
        ) {
            // Use append (not insert) to preserve multiple values for the same
            // header name — e.g. multiple Set-Cookie headers from the provider.
            response_headers.append(name, value);
        }
    }

    let mut resp = Response::builder().status(status);
    if let Some(headers) = resp.headers_mut() {
        *headers = response_headers;
    }

    Ok(resp.body(Body::from(proxy_response.body)).unwrap())
}

/// Errors the proxy handler can produce (converted to HTTP 4xx/5xx responses).
#[derive(Debug)]
pub enum ProxyError {
    UnknownProvider(String),
    EmptyPath,
    ReadBody(String),
    Serialize(String),
    NatsSubscribe(String),
    NatsPublish(String),
    Deserialize(String),
    Timeout { correlation_id: String },
    ReplyChannelClosed,
}

impl std::fmt::Display for ProxyError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::UnknownProvider(p) => write!(f, "Unknown AI provider: {}", p),
            Self::EmptyPath => write!(f, "Request path must not be empty"),
            Self::ReadBody(e) => write!(f, "Failed to read request body: {}", e),
            Self::Serialize(e) => write!(f, "Failed to serialize message: {}", e),
            Self::NatsSubscribe(e) => write!(f, "Failed to subscribe to NATS subject: {}", e),
            Self::NatsPublish(e) => write!(f, "Failed to publish to JetStream: {}", e),
            Self::Deserialize(e) => write!(f, "Failed to deserialize worker reply: {}", e),
            Self::Timeout { correlation_id } => {
                write!(f, "Worker timed out for request {}", correlation_id)
            }
            Self::ReplyChannelClosed => write!(f, "NATS reply subscription was closed"),
        }
    }
}

impl std::error::Error for ProxyError {}

impl axum::response::IntoResponse for ProxyError {
    fn into_response(self) -> Response {
        let (status, body) = match &self {
            Self::UnknownProvider(_) => (StatusCode::BAD_GATEWAY, self.to_string()),
            Self::EmptyPath => (StatusCode::BAD_REQUEST, self.to_string()),
            Self::Timeout { .. } => (StatusCode::GATEWAY_TIMEOUT, self.to_string()),
            _ => (StatusCode::INTERNAL_SERVER_ERROR, self.to_string()),
        };

        tracing::error!(error = %self, "Proxy error");

        Response::builder()
            .status(status)
            .body(Body::from(body))
            .unwrap()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::response::IntoResponse;

    #[test]
    fn unknown_provider_maps_to_502() {
        let resp = ProxyError::UnknownProvider("fakeai".to_string()).into_response();
        assert_eq!(resp.status(), StatusCode::BAD_GATEWAY);
    }

    #[test]
    fn empty_path_maps_to_400() {
        let resp = ProxyError::EmptyPath.into_response();
        assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
    }

    #[test]
    fn timeout_maps_to_504() {
        let resp = ProxyError::Timeout {
            correlation_id: "abc-123".to_string(),
        }
        .into_response();
        assert_eq!(resp.status(), StatusCode::GATEWAY_TIMEOUT);
    }

    #[test]
    fn internal_errors_map_to_500() {
        let cases: Vec<ProxyError> = vec![
            ProxyError::ReadBody("x".to_string()),
            ProxyError::Serialize("x".to_string()),
            ProxyError::NatsSubscribe("x".to_string()),
            ProxyError::NatsPublish("x".to_string()),
            ProxyError::Deserialize("x".to_string()),
            ProxyError::ReplyChannelClosed,
        ];
        for err in cases {
            assert_eq!(err.into_response().status(), StatusCode::INTERNAL_SERVER_ERROR);
        }
    }

    #[test]
    fn error_display_includes_context() {
        assert!(ProxyError::UnknownProvider("fakeai".to_string())
            .to_string()
            .contains("fakeai"));
        assert!(ProxyError::Timeout {
            correlation_id: "req-1".to_string()
        }
        .to_string()
        .contains("req-1"));
        assert!(ProxyError::ReadBody("boom".to_string())
            .to_string()
            .contains("boom"));
        assert!(!ProxyError::ReplyChannelClosed.to_string().is_empty());
    }
}
