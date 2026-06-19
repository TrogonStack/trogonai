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

#[derive(Debug)]
pub enum BridgeError {
    Subscribe(String),
}

impl std::fmt::Display for BridgeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Subscribe(msg) => write!(f, "subscribe failed: {msg}"),
        }
    }
}

impl std::error::Error for BridgeError {}

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
            .map_err(|e| BridgeError::Subscribe(e.to_string()))?;

        info!(
            prefix = prefix.as_str(),
            agent_id = agent_id.as_str(),
            "a2a-nats bridge subscribed"
        );

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
mod tests {
    use bytes::Bytes;

    use super::*;
    use crate::server::test_support::stub;

    fn prefix() -> A2aPrefix {
        A2aPrefix::new("a2a").unwrap()
    }

    fn agent() -> A2aAgentId {
        A2aAgentId::new("bot").unwrap()
    }

    fn msg(subject: &str, reply: Option<&str>, payload: &[u8]) -> async_nats::Message {
        async_nats::Message {
            subject: subject.into(),
            reply: reply.map(|r| r.into()),
            payload: Bytes::copy_from_slice(payload),
            headers: None,
            status: None,
            description: None,
            length: payload.len(),
        }
    }

    #[tokio::test]
    async fn dispatch_routes_agent_card_to_handler() {
        use crate::server::handler::A2aError;
        use trogon_nats::AdvancedMockNatsClient;
        use trogon_nats::jetstream::mocks::MockJetStreamPublisher;

        let nats = AdvancedMockNatsClient::new();
        let js = MockJetStreamPublisher::new();
        let handler = Arc::new(stub());
        handler.lock().unwrap().agent_card_result = Some(Err(A2aError::unsupported_operation("stub")));

        let payload = serde_json::to_vec(&serde_json::json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "agent/getAuthenticatedExtendedCard",
            "params": {}
        }))
        .unwrap();
        let prefix_len = format!("{}.agents.{}", prefix().as_str(), agent().as_str()).len();
        dispatch_message(
            &prefix(),
            &handler,
            &nats,
            &js,
            msg("a2a.agents.bot.card", Some("r"), &payload),
            prefix_len,
        )
        .await;
        let body: serde_json::Value = serde_json::from_slice(&nats.published_payloads()[0]).unwrap();
        assert_eq!(
            body["error"]["code"].as_i64(),
            Some(i64::from(crate::error::UNSUPPORTED_OPERATION))
        );
    }

    #[tokio::test]
    async fn dispatch_drops_unknown_subject_suffix() {
        use trogon_nats::AdvancedMockNatsClient;
        use trogon_nats::jetstream::mocks::MockJetStreamPublisher;

        let nats = AdvancedMockNatsClient::new();
        let js = MockJetStreamPublisher::new();
        let handler = Arc::new(stub());
        let prefix_len = format!("{}.agents.{}", prefix().as_str(), agent().as_str()).len();
        dispatch_message(
            &prefix(),
            &handler,
            &nats,
            &js,
            msg("a2a.agents.bot.unknown.method", Some("r"), b"{}"),
            prefix_len,
        )
        .await;
        assert!(nats.published_messages().is_empty());
    }

    #[test]
    fn bridge_error_display() {
        let e = BridgeError::Subscribe("denied".to_string());
        assert!(e.to_string().contains("denied"));
    }
}
