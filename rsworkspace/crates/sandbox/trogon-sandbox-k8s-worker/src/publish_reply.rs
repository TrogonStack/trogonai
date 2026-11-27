use async_nats::{Client, Subject};
use bytes::Bytes;
use serde::Serialize;
use tracing::warn;

/// Encodes `body` as JSON and publishes it to the request's reply subject, if any.
pub(crate) async fn publish_reply(client: &Client, reply: Option<Subject>, body: &impl Serialize) {
    let encoded = serde_json::to_vec(body).inspect_err(|error| warn!(%error, "failed to encode reply"));
    publish_bytes(client, reply, encoded.ok()).await;
}

/// Publishes an already encoded reply to the request's reply subject, if any.
pub(crate) async fn publish_bytes(client: &Client, reply: Option<Subject>, body: Option<Vec<u8>>) {
    if let (Some(reply), Some(body)) = (reply, body) {
        client
            .publish(reply, Bytes::from(body))
            .await
            .inspect_err(|error| warn!(%error, "publishing the reply failed"))
            .ok();
    }
}
