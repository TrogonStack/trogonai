//! Behaviors the SDK's HTTP transport does not provide, layered over its router.
//!
//! [`AcpHttpServer`](agent_client_protocol_http::AcpHttpServer) owns the remote
//! transport, but it keeps `ConnectionRegistry` private, so anything needing
//! per-connection state cannot be added from inside it. These layers sit outside
//! `into_router()` and reconstruct only what the hand-rolled transport did and
//! upstream does not:
//!
//! - `Origin` enforcement on every verb. Upstream checks it server-side only on
//!   the WebSocket upgrade and otherwise relies on the browser honoring CORS,
//!   which does not constrain a non-browser client.
//! - `Acp-Protocol-Version` validation against the version the connection
//!   actually negotiated. The transport spec says clients SHOULD send this
//!   header; upstream never reads it.
//! - `X-Accel-Buffering: no` on the SSE stream, so a reverse proxy does not
//!   buffer a long-lived response.

use std::collections::HashMap;
use std::net::IpAddr;
use std::sync::{Arc, Mutex, PoisonError};

use axum::body::Body;
use axum::extract::Request;
use axum::http::header::{CONTENT_TYPE, HOST, ORIGIN};
use axum::http::uri::{Authority, Uri};
use axum::http::{HeaderValue, StatusCode};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use tracing::warn;

use crate::constants::{ACP_CONNECTION_ID_HEADER, ACP_PROTOCOL_VERSION_HEADER, X_ACCEL_BUFFERING_HEADER};

/// Largest `initialize` response body inspected for the negotiated version.
///
/// Only the JSON `initialize` reply is ever buffered, and that payload is
/// capability metadata, so this is generous. Anything larger passes through
/// unread rather than being rejected: failing a valid response to enforce a
/// SHOULD-level header check would be the worse trade.
const MAX_INSPECTED_BODY: usize = 1024 * 1024;

/// Protocol version each connection negotiated, keyed by connection id.
///
/// Upstream mints the connection id and keeps the negotiated version private, so
/// the only place both appear together is the `initialize` response: the id in a
/// header, the version in the body. This records that pairing as it passes.
#[derive(Clone, Default)]
pub struct NegotiatedVersions(Arc<Mutex<HashMap<String, u16>>>);

impl NegotiatedVersions {
    pub fn new() -> Self {
        Self::default()
    }

    fn record(&self, connection_id: String, version: u16) {
        self.0
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .insert(connection_id, version);
    }

    fn get(&self, connection_id: &str) -> Option<u16> {
        self.0
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .get(connection_id)
            .copied()
    }

    fn forget(&self, connection_id: &str) {
        self.0
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .remove(connection_id);
    }
}

/// Wraps the SDK's router in every behavior this module restores.
///
/// Both `main` and the tests go through here so the served stack and the tested
/// stack cannot drift: a layer added for production but missing from the harness
/// would otherwise look like upstream misbehaving.
pub fn apply_layers(router: axum::Router, bind_host: IpAddr) -> axum::Router {
    let versions = NegotiatedVersions::new();

    router
        .layer(axum::middleware::from_fn(disable_sse_buffering))
        .layer(axum::middleware::from_fn(move |request, next| {
            track_protocol_version(versions.clone(), request, next)
        }))
        .layer(axum::middleware::from_fn(move |request, next| {
            enforce_origin(bind_host, request, next)
        }))
}

/// Rejects a browser origin that does not match the address the server is bound to.
///
/// Ported from the hand-rolled transport so the check keeps applying to `POST`,
/// `GET`, and `DELETE` and not only the WebSocket upgrade.
pub async fn enforce_origin(bind_host: IpAddr, request: Request, next: Next) -> Response {
    if let Some(rejection) = origin_rejection(bind_host, &request) {
        return rejection;
    }
    next.run(request).await
}

/// Returns the rejection to send, or `None` when the origin is acceptable.
///
/// Phrased as an `Option` rather than a `Result` because a `Response` is a large
/// error variant, and "allowed" carries no value worth returning.
fn origin_rejection(bind_host: IpAddr, request: &Request) -> Option<Response> {
    let headers = request.headers();
    // No Origin means a non-browser client, which this check cannot constrain and
    // is not meant to: it exists to stop a hostile page from reaching a local server.
    let origin = headers.get(ORIGIN)?;

    let forbidden = || {
        warn!("Rejected request with a disallowed Origin");
        (StatusCode::FORBIDDEN, "Origin is not allowed").into_response()
    };

    let Some(origin_host) = origin
        .to_str()
        .ok()
        .and_then(|value| value.parse::<Uri>().ok())
        .and_then(|uri| uri.host().map(str::to_owned))
    else {
        return Some(forbidden());
    };

    let request_host = headers
        .get(HOST)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.parse::<Authority>().ok())
        .map(|authority| authority.host().to_owned());

    let matches_request_host = || {
        request_host
            .as_deref()
            .is_some_and(|host| host.eq_ignore_ascii_case(&origin_host))
    };

    let allowed = if bind_host.is_loopback() {
        is_loopback_host(&origin_host)
    } else if bind_host.is_unspecified() {
        is_loopback_host(&origin_host) || matches_request_host()
    } else {
        origin_host.eq_ignore_ascii_case(&bind_host.to_string()) || matches_request_host()
    };

    if allowed { None } else { Some(forbidden()) }
}

fn is_loopback_host(host: &str) -> bool {
    matches!(host, "localhost" | "127.0.0.1" | "::1" | "[::1]")
}

/// Validates `Acp-Protocol-Version` and learns each connection's negotiated version.
///
/// Request side: a header that disagrees with what the connection negotiated is a
/// client bug, answered with `400` rather than silently served. A header on a
/// connection we have not seen initialize is allowed through, since upstream owns
/// connection lifetime and may know ids this layer does not.
///
/// Response side: the `initialize` reply is the one message carrying both the
/// connection id and the negotiated version, so it is inspected to populate the
/// map. `DELETE` drops the entry so ids cannot accumulate.
pub async fn track_protocol_version(versions: NegotiatedVersions, request: Request, next: Next) -> Response {
    let provided = request
        .headers()
        .get(ACP_PROTOCOL_VERSION_HEADER)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.trim().parse::<u16>().ok());

    let connection_id = request
        .headers()
        .get(ACP_CONNECTION_ID_HEADER)
        .and_then(|value| value.to_str().ok())
        .map(str::to_owned);

    if let (Some(provided), Some(connection_id)) = (provided, connection_id.as_deref())
        && let Some(negotiated) = versions.get(connection_id)
        && provided != negotiated
    {
        warn!(
            provided,
            negotiated, "Rejected request whose Acp-Protocol-Version does not match the connection"
        );
        return (
            StatusCode::BAD_REQUEST,
            "Acp-Protocol-Version header does not match initialized protocol version",
        )
            .into_response();
    }

    let is_delete = request.method() == axum::http::Method::DELETE;
    let response = next.run(request).await;

    if is_delete {
        if let Some(connection_id) = connection_id.as_deref() {
            versions.forget(connection_id);
        }
        return response;
    }

    learn_negotiated_version(&versions, response).await
}

/// Reads `result.protocolVersion` out of an `initialize` reply, passing the body on.
async fn learn_negotiated_version(versions: &NegotiatedVersions, response: Response) -> Response {
    // Only the initialize reply carries a connection id alongside a JSON body.
    // Everything else, notably the SSE stream, must stream untouched.
    let Some(connection_id) = response
        .headers()
        .get(ACP_CONNECTION_ID_HEADER)
        .and_then(|value| value.to_str().ok())
        .map(str::to_owned)
    else {
        return response;
    };

    let is_json = response
        .headers()
        .get(CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .is_some_and(|value| value.starts_with("application/json"));
    if !is_json {
        return response;
    }

    let (parts, body) = response.into_parts();
    let Ok(bytes) = axum::body::to_bytes(body, MAX_INSPECTED_BODY).await else {
        // Oversized or broken bodies are forwarded as-is rather than failed: this
        // layer is an observer, and an unread version simply skips validation.
        return (parts, Body::empty()).into_response();
    };

    if let Some(version) = serde_json::from_slice::<serde_json::Value>(&bytes)
        .ok()
        .and_then(|value| value["result"]["protocolVersion"].as_u64())
        .and_then(|version| u16::try_from(version).ok())
    {
        versions.record(connection_id, version);
    }

    (parts, Body::from(bytes)).into_response()
}

/// Tells reverse proxies not to buffer the SSE stream.
pub async fn disable_sse_buffering(request: Request, next: Next) -> Response {
    let mut response = next.run(request).await;

    let is_event_stream = response
        .headers()
        .get(CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .is_some_and(|value| value.starts_with("text/event-stream"));

    if is_event_stream {
        response
            .headers_mut()
            .insert(X_ACCEL_BUFFERING_HEADER, HeaderValue::from_static("no"));
    }

    response
}

#[cfg(test)]
mod tests;
