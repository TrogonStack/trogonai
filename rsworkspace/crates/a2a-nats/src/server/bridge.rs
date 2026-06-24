//! Agent-side bridge that subscribes to the wildcard agent subject and dispatches
//! inbound JSON-RPC requests to an [`A2aExecutor`] implementation.
//!
//! Audit emission, push-notification dispatch, and cancellation tracking are
//! deferred to follow-up PRs so this slice stays focused on the wildcard
//! subscribe + per-op routing path the binaries need to come up.

use std::sync::Arc;

use futures::StreamExt;
use tokio_util::sync::CancellationToken;
use tracing::{info, warn};
use trogon_nats::jetstream::JetStreamPublisher;
use trogon_nats::{PublishClient, SubscribeClient};

use crate::a2a_prefix::A2aPrefix;
use crate::agent_id::A2aAgentId;
use crate::config::Config;
use crate::nats::subjects::subscriptions::agent_all::AgentAllSubject;
use crate::server::dispatch::A2aMethod;
use crate::server::handler::A2aExecutor;
use crate::server::{
    agent_card, message_send, message_stream, push_delete, push_get, push_list, push_set, tasks_cancel, tasks_get,
    tasks_list, tasks_resubscribe,
};

#[derive(Debug, thiserror::Error)]
pub enum BridgeError {
    /// Subscribing to the agent wildcard subject failed. The source error from
    /// the NATS client implementation flows through `Error::source` so callers
    /// can downcast without us flattening it into a string.
    #[error("subscribe failed")]
    Subscribe(#[source] Box<dyn std::error::Error + Send + Sync>),
}

pub struct Bridge<H, N, J> {
    config: Config,
    handler: Arc<H>,
    nats: N,
    js: J,
}

impl<H, N, J> Bridge<H, N, J>
where
    H: A2aExecutor,
    N: SubscribeClient + PublishClient,
    J: JetStreamPublisher,
{
    pub fn new(config: Config, handler: H, nats: N, js: J) -> Self {
        Self {
            config,
            handler: Arc::new(handler),
            nats,
            js,
        }
    }

    /// Subscribe to `{prefix}.agents.{agent_id}.>` and dispatch each inbound
    /// request to the matching per-op handler. Returns when `shutdown` is
    /// cancelled or the subscription terminates.
    pub async fn run_with_agent_id(
        self,
        agent_id: &A2aAgentId,
        shutdown: CancellationToken,
    ) -> Result<(), BridgeError> {
        let prefix = self.config.a2a_prefix_ref().clone();
        let subject = AgentAllSubject::new(&prefix, agent_id);
        let prefix_len = format!("{}.agents.{}", prefix.as_str(), agent_id.as_str()).len();

        let mut sub = self
            .nats
            .subscribe(subject)
            .await
            .map_err(|e| BridgeError::Subscribe(Box::new(e)))?;

        let subscribed_wildcard = format!("{}.agents.{}.>", prefix.as_str(), agent_id.as_str());
        info!("a2a-nats bridge subscribed to {subscribed_wildcard}");

        loop {
            tokio::select! {
                _ = shutdown.cancelled() => {
                    info!("bridge shutdown signal received");
                    return Ok(());
                }
                next = sub.next() => match next {
                    None => {
                        info!("bridge subscription closed");
                        return Ok(());
                    }
                    Some(msg) => {
                        dispatch_message(&prefix, &self.handler, &self.nats, &self.js, msg, prefix_len).await;
                    }
                },
            }
        }
    }
}

async fn dispatch_message<H, N, J>(
    prefix: &A2aPrefix,
    handler: &Arc<H>,
    nats: &N,
    js: &J,
    msg: async_nats::Message,
    prefix_len: usize,
) where
    H: A2aExecutor,
    N: PublishClient,
    J: JetStreamPublisher,
{
    let subject = msg.subject.to_string();
    let reply = msg.reply.map(|s| s.to_string());
    let payload = msg.payload;

    let Some(method) = A2aMethod::from_subject(&subject, prefix_len) else {
        warn!(subject = %subject, "received message on unknown subject suffix; dropping");
        return;
    };

    match method {
        A2aMethod::MessageSend => message_send::handle(handler.as_ref(), &payload, reply, nats).await,
        A2aMethod::MessageStream => {
            message_stream::handle(handler.as_ref(), &payload, reply, nats, js, prefix).await;
        }
        A2aMethod::TasksGet => tasks_get::handle(handler.as_ref(), &payload, reply, nats).await,
        A2aMethod::TasksList => tasks_list::handle(handler.as_ref(), &payload, reply, nats).await,
        A2aMethod::TasksCancel => tasks_cancel::handle(handler.as_ref(), &payload, reply, nats).await,
        A2aMethod::TasksResubscribe => tasks_resubscribe::handle(handler.as_ref(), &payload, reply, nats).await,
        A2aMethod::PushNotificationSet => push_set::handle(handler.as_ref(), &payload, reply, nats).await,
        A2aMethod::PushNotificationGet => push_get::handle(handler.as_ref(), &payload, reply, nats).await,
        A2aMethod::PushNotificationList => push_list::handle(handler.as_ref(), &payload, reply, nats).await,
        A2aMethod::PushNotificationDelete => push_delete::handle(handler.as_ref(), &payload, reply, nats).await,
        A2aMethod::AgentCard => agent_card::handle(handler.as_ref(), &payload, reply, nats).await,
    }
}

#[cfg(test)]
mod tests;
