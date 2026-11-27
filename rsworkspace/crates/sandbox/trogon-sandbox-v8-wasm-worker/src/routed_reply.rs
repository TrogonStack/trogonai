use std::sync::Arc;

use async_nats::{Client, Message};
use bytes::Bytes;
use futures::future::OptionFuture;
use tracing::warn;
use trogon_sandbox_contract::{LeaseId, RunCommandRequest};
use trogon_sandbox_protocol::nats_transfer;

use crate::publish_reply::publish_encoded;
use crate::{RoutedVerb, V8WasmProvider};

/// Answers a routed verb for a lease this worker holds. Anything for another lease is left to the
/// worker that owns it.
pub(crate) async fn handle_verb(client: Client, v8: Arc<V8WasmProvider>, verb: RoutedVerb, msg: Message) {
    let lease = msg.subject.split('.').nth(4).map(LeaseId::new);
    let known = OptionFuture::from(lease.as_ref().map(|lease| v8.knows_lease(lease))).await;
    if !known.unwrap_or(false) {
        return;
    }
    if verb == RoutedVerb::RunCommand && reply_from_persistent_runtime(&client, &msg, &v8).await {
        return;
    }
    publish_encoded(&client, msg.reply, verb.unsupported_reply()).await;
}

/// Answers a `run_command` from the lease's persistent runtime. False when the lease holds none,
/// so the caller sends the generic unsupported reply.
async fn reply_from_persistent_runtime(client: &Client, msg: &Message, v8: &V8WasmProvider) -> bool {
    let request = nats_transfer::receive(client, msg)
        .await
        .ok()
        .and_then(|payload| serde_json::from_slice::<RunCommandRequest>(&payload).ok());
    let response = OptionFuture::from(request.map(|request| async move { v8.run_session_command(&request).await }))
        .await
        .flatten();
    let (Some(reply), Some(response)) = (msg.reply.as_ref(), response) else {
        return false;
    };
    let body = serde_json::to_vec(&response).inspect_err(|error| warn!(%error, "failed to encode run_command reply"));
    let sending = body
        .ok()
        .map(|body| nats_transfer::respond(client, reply, Bytes::from(body)));
    let sent = OptionFuture::from(sending).await;
    sent.transpose()
        .inspect_err(|error| warn!(%error, "publishing the reply failed"))
        .ok();
    true
}
