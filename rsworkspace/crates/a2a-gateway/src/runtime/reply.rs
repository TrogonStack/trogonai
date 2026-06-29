//! Fire-and-forget error replies on JSON-RPC ingress subjects.
//!
//! The dispatch path calls this from denial / mapping branches that
//! still need to send a JSON-RPC error envelope back to the caller.
//! A publish failure here is logged but does not cascade: the caller
//! has already gone (or will time out) and the gateway must keep
//! servicing other ingress requests.

use async_nats::{Client as NatsAsyncClient, HeaderMap, Subject};
use bytes::Bytes;
use tracing::warn;

/// Publish a JSON-RPC error reply onto `reply` with the supplied
/// `headers` and `body`. Best-effort: a NATS publish failure surfaces
/// only as a warning, since the dispatch path has nothing actionable
/// to do with it.
pub async fn reply_error(client: &NatsAsyncClient, reply: Subject, headers: HeaderMap, body: Bytes) {
    if let Err(error) = client.publish_with_headers(reply, headers, body).await {
        warn!(error = %error, routing_outcome = "error_reply_publish_failed");
    }
}
