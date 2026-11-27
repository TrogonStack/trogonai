use std::sync::Arc;

use async_nats::{Client, Subject};
use bytes::Bytes;
use serde::Serialize;
use tracing::warn;
use trogon_sandbox_protocol::nats_transfer;
use trogon_sandbox_ssh_gateway::SshStreamBridge;

use crate::publish_reply::publish_reply;
use crate::{K8sTerminals, K8sWorkerProvider};

/// What the verb handlers need to act on a lease and answer for it.
pub(crate) struct VerbContext {
    pub(crate) client: Client,
    pub(crate) provider: Arc<K8sWorkerProvider>,
    pub(crate) terminals: Arc<K8sTerminals>,
    pub(crate) bridge: Arc<SshStreamBridge>,
}

impl VerbContext {
    /// The largest message the bus accepts.
    pub(crate) fn max_payload(&self) -> usize {
        self.client.server_info().max_payload
    }

    /// Answers a request with one message.
    pub(crate) async fn reply(&self, to: Subject, body: &impl Serialize) {
        publish_reply(&self.client, Some(to), body).await;
    }

    /// Answers a request with a transfer that may span several messages.
    pub(crate) async fn reply_transfer(&self, to: &Subject, body: serde_json::Result<Vec<u8>>) {
        match body {
            Ok(body) => {
                nats_transfer::respond(&self.client, to.as_str(), Bytes::from(body))
                    .await
                    .inspect_err(|error| warn!(%error, "publishing the reply failed"))
                    .ok();
            }
            Err(error) => warn!(%error, "failed to encode reply"),
        }
    }
}
