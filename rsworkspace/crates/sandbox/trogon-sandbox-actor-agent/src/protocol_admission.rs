use std::sync::Arc;

use axum::Json;
use axum::extract::{Request, State};
use axum::http::{HeaderValue, StatusCode};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};

use crate::{
    AGENT_MIN_PROTOCOL_VERSION_HEADER, AGENT_PROTOCOL_VERSION_HEADER, AgentState, GUEST_PROTOCOL_VERSION, READYZ_PATH,
    RESERVED_PATH_PREFIX, WORKER_PROTOCOL_VERSION_HEADER, admit_protocol_version, parse_protocol_version,
};

/// Version admission for the agent's own paths.
///
/// A worker below the minimum is refused with a typed [`crate::GuestProtocolError`]; a worker that sends no header is
/// version 0. Proxied tenant traffic is never touched. Every answer on an agent path, refusals included, carries the
/// agent's own versions so a worker can run the same check the other way.
pub(crate) async fn admit_protocol(State(state): State<Arc<AgentState>>, request: Request, next: Next) -> Response {
    let path = request.uri().path();
    let readiness = path == READYZ_PATH;
    if !readiness && !path.starts_with(RESERVED_PATH_PREFIX) {
        return next.run(request).await;
    }
    let minimum = state.min_worker_protocol_version;
    let header = request
        .headers()
        .get(WORKER_PROTOCOL_VERSION_HEADER)
        .and_then(|value| value.to_str().ok());
    let admitted = parse_protocol_version(header).and_then(|peer| admit_protocol_version(peer, minimum));
    // The readiness probe stays unconditional: the runtime's own probe sends no version and must keep gating resume.
    let refusal = admitted.err().filter(|_| !readiness);
    let mut response = match refusal {
        Some(error) => (StatusCode::UPGRADE_REQUIRED, Json(error)).into_response(),
        None => next.run(request).await,
    };
    let headers = response.headers_mut();
    headers.insert(AGENT_PROTOCOL_VERSION_HEADER, HeaderValue::from(GUEST_PROTOCOL_VERSION));
    headers.insert(AGENT_MIN_PROTOCOL_VERSION_HEADER, HeaderValue::from(minimum));
    response
}
