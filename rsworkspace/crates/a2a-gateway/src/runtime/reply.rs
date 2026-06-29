//! Fire-and-forget error replies on JSON-RPC ingress subjects.
//!
//! The dispatch path calls this from denial / mapping branches that
//! still need to send a JSON-RPC error envelope back to the caller.
//! A publish failure here is logged but does not cascade: the caller
//! has already gone (or will time out) and the gateway must keep
//! servicing other ingress requests.

use async_nats::{HeaderMap, Subject};
use bytes::Bytes;
use tracing::warn;
use trogon_nats::PublishClient;

/// Publish a JSON-RPC error reply onto `reply` with the supplied
/// `headers` and `body`. Best-effort: a NATS publish failure surfaces
/// only as a warning, since the dispatch path has nothing actionable
/// to do with it.
///
/// Generic over [`PublishClient`] so the same helper drives the live
/// async-nats client at dispatch time and `MockNatsClient` in tests
/// without a hidden indirection.
pub async fn reply_error<C: PublishClient>(client: &C, reply: Subject, headers: HeaderMap, body: Bytes) {
    if let Err(error) = client.publish_with_headers(reply, headers, body).await {
        warn!(error = %error, routing_outcome = "error_reply_publish_failed");
    }
}

#[cfg(test)]
mod tests;
