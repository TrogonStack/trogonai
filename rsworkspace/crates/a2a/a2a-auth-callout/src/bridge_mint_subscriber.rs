use std::sync::Arc;

use futures::StreamExt as _;
use tracing::{error, info, warn};
use trogon_std::env::ReadEnv;

use crate::bridge_mint_responder::{BridgeMintResponder, bridge_mint_subject};
use crate::dispatcher::AuthDispatcher;
use crate::error::AuthCalloutError;
use crate::subscriber::queue_group;

/// Serves the bridge mint subject on the callout's connection. Replicas share
/// the callout's queue group, so each request reaches one of them.
pub struct BridgeMintSubscriber<D> {
    client: async_nats::Client,
    responder: Arc<BridgeMintResponder<D>>,
    subscription: async_nats::Subscriber,
    subject: String,
}

impl<D: AuthDispatcher> BridgeMintSubscriber<D> {
    /// Installs the subscription. Call it before the server callout subscriber
    /// flushes its own, so one flush covers both before the process reports
    /// ready.
    pub async fn subscribe(
        client: async_nats::Client,
        responder: BridgeMintResponder<D>,
        env: &impl ReadEnv,
    ) -> Result<Self, AuthCalloutError> {
        let subject = bridge_mint_subject(env);
        let subscription = client
            .queue_subscribe(subject.clone(), queue_group(env))
            .await
            .map_err(AuthCalloutError::Subscribe)?;
        Ok(Self {
            client,
            responder: Arc::new(responder),
            subscription,
            subject,
        })
    }

    pub fn subject(&self) -> &str {
        &self.subject
    }

    /// Answers requests until the subscription closes.
    pub async fn serve(mut self) {
        let subject = self.subject.clone();
        info!(subject = %subject, "bridge mint subscriber started");

        while let Some(message) = self.subscription.next().await {
            let Some(reply) = message.reply.clone() else {
                warn!("bridge mint request without reply subject; dropping");
                continue;
            };
            let client = self.client.clone();
            let responder = Arc::clone(&self.responder);

            tokio::spawn(async move {
                let payload = responder.respond(&message.payload).await.to_payload();
                if let Err(publish_error) = client.publish(reply, payload.into()).await {
                    error!(error = %publish_error, "failed to publish bridge mint reply");
                }
            });
        }

        warn!("bridge mint NATS subscription closed");
    }
}

#[cfg(test)]
mod tests;
