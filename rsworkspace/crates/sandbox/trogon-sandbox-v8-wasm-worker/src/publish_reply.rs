use async_nats::{Client, Subject};
use bytes::Bytes;
use serde::Serialize;
use tracing::warn;

/// Publishes an already encoded reply, doing nothing when the request carried no reply subject.
pub(crate) async fn publish_encoded(client: &Client, reply: Option<Subject>, body: serde_json::Result<Vec<u8>>) {
    let encoded = body.inspect_err(|error| warn!(%error, "failed to encode reply"));
    if let (Some(reply), Ok(body)) = (reply, encoded) {
        client.publish(reply, Bytes::from(body)).await.ok();
    }
}

/// Encodes `body` as JSON and publishes it to the request's reply subject, if any.
pub(crate) async fn publish_reply(client: &Client, reply: Option<Subject>, body: &impl Serialize) {
    publish_encoded(client, reply, serde_json::to_vec(body)).await;
}
