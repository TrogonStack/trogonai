use async_nats::{Client, Message, Subject};
use serde::de::DeserializeOwned;
use tracing::warn;
use trogon_sandbox_protocol::nats_transfer;

use crate::K8sVerb;

/// A decoded verb request together with the subject to answer it on.
pub(crate) struct Inbound<T> {
    pub(crate) reply: Subject,
    pub(crate) request: T,
}

impl<T: DeserializeOwned> Inbound<T> {
    /// Decodes a request carried by one message. A request that wants no answer, or does not decode, is dropped.
    pub(crate) fn decode(msg: &Message, verb: K8sVerb) -> Option<Self> {
        let reply = msg.reply.clone()?;
        let request = serde_json::from_slice(&msg.payload)
            .inspect_err(|error| warn!(%error, verb = verb.token(), "invalid request"))
            .ok()?;
        Some(Self { reply, request })
    }

    /// Decodes a request that may span several bus messages.
    pub(crate) async fn receive(client: &Client, msg: &Message, verb: K8sVerb) -> Option<Self> {
        let reply = msg.reply.clone()?;
        let payload = nats_transfer::receive(client, msg)
            .await
            .inspect_err(|error| warn!(%error, verb = verb.token(), "receiving the request failed"))
            .ok()?;
        let request = serde_json::from_slice(&payload)
            .inspect_err(|error| warn!(%error, verb = verb.token(), "invalid request"))
            .ok()?;
        Some(Self { reply, request })
    }
}
