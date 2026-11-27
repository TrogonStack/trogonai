use async_nats::Subject;
use bytes::Bytes;
use serde::Serialize;
use tracing::warn;
use trogon_sandbox_protocol::nats_transfer;

/// Encodes `response` as JSON, or logs why it could not.
fn encode<Resp: Serialize>(response: &Resp) -> Option<Bytes> {
    match serde_json::to_vec(response) {
        Ok(payload) => Some(Bytes::from(payload)),
        Err(error) => {
            warn!(error = %error, "failed to encode reply");
            None
        }
    }
}

/// Publishes `response` as a plain JSON reply.
pub(crate) async fn publish_json<Resp: Serialize>(nats: &async_nats::Client, reply: Subject, response: &Resp) {
    let Some(payload) = encode(response) else {
        return;
    };
    if let Err(error) = nats.publish(reply, payload).await {
        warn!(error = %error, "failed to publish reply");
    }
}

/// Sends `response` through the chunked transfer protocol, for replies that may be large.
pub(crate) async fn respond_json<Resp: Serialize>(nats: &async_nats::Client, reply: &str, response: &Resp) {
    let Some(payload) = encode(response) else {
        return;
    };
    if let Err(error) = nats_transfer::respond(nats, reply, payload).await {
        warn!(error = %error, "failed to send transfer reply");
    }
}
