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

use std::collections::{HashMap, VecDeque};
use std::net::IpAddr;
use std::sync::{Arc, Mutex, PoisonError};

use axum::body::Body;
use axum::extract::Request;
use axum::http::header::{CONTENT_LENGTH, CONTENT_TYPE, HOST, ORIGIN};
use axum::http::uri::{Authority, Uri};
use axum::http::{HeaderValue, StatusCode};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use tracing::warn;

use crate::constants::{
    ACP_CONNECTION_ID_HEADER, ACP_PROTOCOL_VERSION_HEADER, MAX_INSPECTED_BODY, MAX_TRACKED_CONNECTIONS,
    X_ACCEL_BUFFERING_HEADER,
};

/// Bounded insertion-ordered map of connection id to negotiated version.
#[derive(Default)]
struct VersionTable {
    versions: HashMap<String, u16>,
    oldest_first: VecDeque<String>,
}

/// Protocol version each connection negotiated, keyed by connection id.
///
/// Upstream mints the connection id and keeps the negotiated version private, so
/// the only place both appear together is the `initialize` response: the id in a
/// header, the version in the body. This records that pairing as it passes.
#[derive(Clone, Default)]
pub struct NegotiatedVersions(Arc<Mutex<VersionTable>>);

impl NegotiatedVersions {
    pub fn new() -> Self {
        Self::default()
    }

    fn record(&self, connection_id: String, version: u16) {
        let mut table = self.0.lock().unwrap_or_else(PoisonError::into_inner);

        // Re-initializing a known connection updates in place: pushing again would
        // let one id occupy several eviction slots and evict live connections early.
        if table.versions.insert(connection_id.clone(), version).is_some() {
            return;
        }

        table.oldest_first.push_back(connection_id);

        while table.oldest_first.len() > MAX_TRACKED_CONNECTIONS {
            if let Some(evicted) = table.oldest_first.pop_front() {
                table.versions.remove(&evicted);
            }
        }
    }

    fn get(&self, connection_id: &str) -> Option<u16> {
        self.0
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .versions
            .get(connection_id)
            .copied()
    }

    fn forget(&self, connection_id: &str) {
        let mut table = self.0.lock().unwrap_or_else(PoisonError::into_inner);
        if table.versions.remove(connection_id).is_some() {
            table.oldest_first.retain(|id| id != connection_id);
        }
    }

    #[cfg(test)]
    fn tracked(&self) -> usize {
        let table = self.0.lock().unwrap_or_else(PoisonError::into_inner);
        debug_assert_eq!(
            table.versions.len(),
            table.oldest_first.len(),
            "the eviction queue must mirror the map"
        );
        table.versions.len()
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
        unbracket(&origin_host).eq_ignore_ascii_case(&bind_host.to_string()) || matches_request_host()
    };

    if allowed { None } else { Some(forbidden()) }
}

fn is_loopback_host(host: &str) -> bool {
    matches!(unbracket(host), "localhost" | "127.0.0.1" | "::1")
}

/// Strips the brackets `Uri::host()` keeps around an IPv6 literal.
///
/// `IpAddr::to_string()` produces none, so a bracketed `[2001:db8::1]` would never
/// equal the bind address it names.
fn unbracket(host: &str) -> &str {
    host.strip_prefix('[')
        .and_then(|host| host.strip_suffix(']'))
        .unwrap_or(host)
}

/// Validates `Acp-Protocol-Version` and learns each connection's negotiated version.
///
/// Request side: a header that is present but not a number, or that disagrees with
/// what the connection negotiated, is a client bug and is answered with `400`
/// rather than silently served. A well-formed header on a connection we have not
/// seen initialize is allowed through, since upstream owns connection lifetime and
/// may know ids this layer does not.
///
/// Response side: the `initialize` reply is the one message carrying both the
/// connection id and the negotiated version, so it is inspected to populate the
/// map. `DELETE` drops the entry; every other kind of close is invisible here, so
/// the table is bounded by eviction rather than by clients behaving well.
pub async fn track_protocol_version(versions: NegotiatedVersions, request: Request, next: Next) -> Response {
    // Absent and malformed are different answers. Parsing straight to an `Option`
    // would conflate them and let a garbage header through as though the client had
    // sent nothing, which is exactly the validation this layer exists to restore.
    let provided = match request.headers().get(ACP_PROTOCOL_VERSION_HEADER) {
        None => None,
        Some(value) => {
            let Some(version) = value.to_str().ok().and_then(|value| value.trim().parse::<u16>().ok()) else {
                warn!("Rejected request with a malformed Acp-Protocol-Version header");
                return (StatusCode::BAD_REQUEST, "invalid Acp-Protocol-Version header").into_response();
            };
            Some(version)
        }
    };

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

    // Check the declared length before touching the body. `to_bytes` is destructive,
    // so discovering the cap by failing it would leave nothing to forward.
    let declared_too_large = response
        .headers()
        .get(CONTENT_LENGTH)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.parse::<usize>().ok())
        .is_some_and(|length| length > MAX_INSPECTED_BODY);
    if declared_too_large {
        return response;
    }

    let (parts, body) = response.into_parts();
    let Ok(bytes) = axum::body::to_bytes(body, MAX_INSPECTED_BODY).await else {
        // The body is consumed by now, so it cannot be forwarded. Returning the
        // original parts with an empty body would hand the client a response whose
        // `Content-Length` disagrees with its payload: a truncated reply that still
        // looks successful. An explicit failure is the lesser harm.
        warn!("Could not read an initialize response body; failing rather than truncating it");
        return StatusCode::BAD_GATEWAY.into_response();
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
