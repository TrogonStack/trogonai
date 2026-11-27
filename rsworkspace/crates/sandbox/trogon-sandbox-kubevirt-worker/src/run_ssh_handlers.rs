use std::sync::Arc;

use async_nats::{Client, Message, Subject, SubscribeError};
use futures::StreamExt;
use serde::Serialize;
use tokio::task::JoinSet;
use tracing::warn;
use trogon_sandbox_protocol::subscribe_verified;
use trogon_sandbox_ssh_gateway::{
    SSH_ATTACH_FILTER, SSH_DETACH_FILTER, SshAttachRequest, SshDetachRequest, SshStreamBridge,
};

use crate::SshSessionKeys;

/// Serves SSH attach and detach requests until the client stops delivering them. The stream bridge owns each
/// session and answers only for leases this worker holds; the session's key is also installed in the guest, so a
/// direct `sshd` login with the session key keeps working.
pub async fn run_ssh_handlers(
    client: Client,
    keys: Arc<SshSessionKeys>,
    bridge: Arc<SshStreamBridge>,
) -> Result<(), SubscribeError> {
    let mut attaches = subscribe_verified(&client, SSH_ATTACH_FILTER).await?;
    let mut detaches = subscribe_verified(&client, SSH_DETACH_FILTER).await?;
    let mut requests = JoinSet::new();
    loop {
        tokio::select! {
            message = attaches.next() => {
                let Some(message) = message else { break };
                requests.spawn(serve_attach(client.clone(), Arc::clone(&keys), Arc::clone(&bridge), message));
            }
            message = detaches.next() => {
                let Some(message) = message else { break };
                requests.spawn(serve_detach(client.clone(), Arc::clone(&keys), Arc::clone(&bridge), message));
            }
            Some(_) = requests.join_next(), if !requests.is_empty() => {}
        }
    }
    while requests.join_next().await.is_some() {}
    Ok(())
}

async fn serve_attach(client: Client, keys: Arc<SshSessionKeys>, bridge: Arc<SshStreamBridge>, message: Message) {
    let Some(reply) = message.reply else { return };
    let request: SshAttachRequest = match serde_json::from_slice(&message.payload) {
        Ok(request) => request,
        Err(error) => {
            warn!(%error, "invalid ssh attach request");
            return;
        }
    };
    let Some(response) = bridge.attach(&request).await else {
        return;
    };
    if response.ok {
        let installed = keys.attach(&request).await;
        if !installed.ok {
            warn!(
                message = ?installed.message,
                "ssh session key was not installed; the stream bridge still serves the session"
            );
        }
    }
    publish(&client, reply, &response).await;
}

async fn serve_detach(client: Client, keys: Arc<SshSessionKeys>, bridge: Arc<SshStreamBridge>, message: Message) {
    let Some(reply) = message.reply else { return };
    let request: SshDetachRequest = match serde_json::from_slice(&message.payload) {
        Ok(request) => request,
        Err(error) => {
            warn!(%error, "invalid ssh detach request");
            return;
        }
    };
    let Some(response) = bridge.detach(&request).await else {
        return;
    };
    if response.ok {
        let removed = keys.detach(&request).await;
        if !removed.ok {
            warn!(message = ?removed.message, "ssh session key was not removed");
        }
    }
    publish(&client, reply, &response).await;
}

async fn publish(client: &Client, reply: Subject, response: &impl Serialize) {
    let Ok(body) = serde_json::to_vec(response) else { return };
    let _ = client.publish(reply, body.into()).await;
}
