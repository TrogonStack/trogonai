use std::sync::Arc;

use async_nats::{Client, Message};
use bytes::Bytes;
use futures::StreamExt;
use tracing::warn;
use trogon_sandbox_contract::TerminalRequest;

use crate::{TerminalHub, TunnelError};

/// Serves `subject`, the provider's broadcast terminal subject. Every worker of the provider receives each request;
/// only the one holding the lease acts and replies.
pub async fn run_terminal_handler(client: Client, hub: Arc<TerminalHub>, subject: &str) -> Result<(), TunnelError> {
    let mut subscription = client.subscribe(subject.to_string()).await?;
    while let Some(message) = subscription.next().await {
        let (client, hub) = (client.clone(), Arc::clone(&hub));
        tokio::spawn(async move {
            if let Err(error) = handle_terminal_message(message, client, hub).await {
                warn!(%error, "failed to handle terminal request");
            }
        });
    }
    Ok(())
}

async fn handle_terminal_message(message: Message, client: Client, hub: Arc<TerminalHub>) -> Result<(), TunnelError> {
    let Some(reply) = message.reply.clone() else {
        return Ok(());
    };
    let request: TerminalRequest = match serde_json::from_slice(&message.payload) {
        Ok(request) => request,
        Err(error) => {
            warn!(%error, "invalid terminal request");
            return Ok(());
        }
    };
    if !hub.holds(&request.lease_id).await {
        return Ok(());
    }
    let response = hub.handle(&request).await;
    let body = serde_json::to_vec(&response).map_err(TunnelError::Encode)?;
    client.publish(reply, Bytes::from(body)).await?;
    Ok(())
}
