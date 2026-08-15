//! Reading task events an older release wrote.
//!
//! Task events on `A2A_EVENTS` are JSON-RPC success responses. They used to be
//! bare [`StreamResponse`] bodies, and a reader on this release still meets those:
//! the stream retains by limits for a whole `max_age` window, so `tasks/resubscribe`
//! replays them, and a rolling upgrade runs both agent releases at once, so a live
//! subscription sees them too. Refusing them would end a stream mid-task on every
//! deploy.
//!
//! This module can go once no deployment can still hold pre-envelope events: one
//! retention window after the last agent is upgraded.

use a2a::event::StreamResponse;
use jsonrpc_nats::JSONRPC_VERSION;
use serde::Serialize;
use serde_json::Value;

/// Decodes a task event that predates the JSON-RPC envelope, or `None` if the body
/// is not one.
///
/// The decision is a full parse rather than a shape guess, so a body that is merely
/// malformed stays the decode error it is instead of arriving as a plausible event.
/// A current envelope carries none of the four variant keys, so it never lands here.
pub fn decode_legacy_event(body: &[u8]) -> Option<StreamResponse> {
    serde_json::from_slice::<StreamResponse>(body).ok()
}

/// The same event lifted into the envelope this release's readers expect, for the
/// hops that forward event bytes without typing them.
pub fn legacy_event_as_response(body: &[u8], id: &Value) -> Option<Value> {
    let event = decode_legacy_event(body)?;
    let envelope = LegacyEventResponse {
        jsonrpc: JSONRPC_VERSION,
        id,
        result: event,
    };
    serde_json::to_value(envelope).ok()
}

/// The envelope a legacy event is lifted into: a JSON-RPC success response
/// carrying the reader's id, since the event bytes never held one.
#[derive(Serialize)]
struct LegacyEventResponse<'a> {
    jsonrpc: &'static str,
    id: &'a Value,
    result: StreamResponse,
}

#[cfg(test)]
mod tests;
