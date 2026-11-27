use async_nats::{Client, Subject};
use bytes::Bytes;
use serde::Serialize;
use tracing::warn;

/// Publishes `value` as JSON on `reply`. A reply that cannot be encoded or published is logged and dropped, because
/// the requester is gone or will time out either way.
pub(crate) async fn publish_json<T: Serialize>(client: &Client, reply: Subject, value: &T, what: &'static str) {
    match serde_json::to_vec(value) {
        Ok(body) => publish_body(client, reply, body.into(), what).await,
        Err(error) => warn!(what, %error, "failed to encode a reply"),
    }
}

/// Publishes an encoded reply, logging a failure instead of returning it.
pub(crate) async fn publish_body(client: &Client, reply: Subject, body: Bytes, what: &'static str) {
    if let Err(error) = client.publish(reply, body).await {
        warn!(what, %error, "failed to publish a reply");
    }
}

#[cfg(test)]
mod tests;
