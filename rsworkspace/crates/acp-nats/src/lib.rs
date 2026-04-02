pub mod acp_prefix;
pub mod agent;
pub mod client;
pub mod client_proxy;
pub mod config;
pub mod constants;
pub mod error;
pub mod ext_method_name;
pub(crate) mod in_flight_slot_guard;
pub mod jetstream;
pub(crate) mod jsonrpc;
pub mod nats;
pub(crate) mod pending_prompt_waiters;
pub mod req_id;
pub mod session_id;
pub(crate) mod telemetry;

pub use acp_prefix::{AcpPrefix, AcpPrefixError};
pub use agent::Bridge;
pub use agent::REQ_ID_HEADER;
pub use client_proxy::NatsClientProxy;
pub use config::{
    Config, DEFAULT_ACP_PREFIX, ENV_ACP_PREFIX, apply_timeout_overrides, nats_connect_timeout,
};
pub use error::AGENT_UNAVAILABLE;
pub use ext_method_name::ExtMethodName;
pub use nats::session::agent::{PromptResponseSubject, ResponseSubject, UpdateSubject};
pub use nats::{FlushClient, PublishClient, RequestClient, SubscribeClient};
pub use req_id::ReqId;
pub use session_id::AcpSessionId;
#[cfg(not(coverage))]
pub use trogon_nats::jetstream::NatsJetStreamClient;
pub use trogon_nats::jetstream::{JetStreamGetStream, JetStreamPublisher};
pub use trogon_nats::{NatsAuth, NatsConfig};
pub use trogon_std::StdJsonSerialize;

pub fn spawn_notification_forwarder(
    client: impl agent_client_protocol::Client + 'static,
    mut rx: tokio::sync::mpsc::Receiver<agent_client_protocol::SessionNotification>,
) {
    tokio::task::spawn_local(async move {
        while let Some(notif) = rx.recv().await {
            if client.session_notification(notif).await.is_err() {
                break;
            }
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use agent_client_protocol::{SessionNotification, SessionUpdate};
    use std::cell::RefCell;
    use std::rc::Rc;

    struct MockClient {
        received: Rc<RefCell<Vec<SessionNotification>>>,
        fail_after: Option<usize>,
    }

    impl MockClient {
        fn new(fail_after: Option<usize>) -> Self {
            Self {
                received: Rc::new(RefCell::new(Vec::new())),
                fail_after,
            }
        }
    }

    #[async_trait::async_trait(?Send)]
    impl agent_client_protocol::Client for MockClient {
        async fn request_permission(
            &self,
            _args: agent_client_protocol::RequestPermissionRequest,
        ) -> agent_client_protocol::Result<agent_client_protocol::RequestPermissionResponse>
        {
            Err(agent_client_protocol::Error::internal_error())
        }

        async fn session_notification(
            &self,
            args: SessionNotification,
        ) -> agent_client_protocol::Result<()> {
            let count = self.received.borrow().len();
            if let Some(limit) = self.fail_after
                && count >= limit
            {
                return Err(agent_client_protocol::Error::internal_error());
            }
            self.received.borrow_mut().push(args);
            Ok(())
        }
    }

    #[tokio::test]
    async fn spawn_notification_forwarder_delivers_notifications() {
        let client = MockClient::new(None);
        let received = client.received.clone();
        let (tx, rx) = tokio::sync::mpsc::channel(16);

        let local = tokio::task::LocalSet::new();
        local
            .run_until(async {
                spawn_notification_forwarder(client, rx);

                let notif = SessionNotification::new(
                    "s1",
                    SessionUpdate::AgentMessageChunk(agent_client_protocol::ContentChunk::new(
                        agent_client_protocol::ContentBlock::Text(
                            agent_client_protocol::TextContent::new("hello"),
                        ),
                    )),
                );
                tx.send(notif).await.unwrap();
                drop(tx);

                tokio::task::yield_now().await;
                tokio::task::yield_now().await;
            })
            .await;

        assert_eq!(received.borrow().len(), 1);
    }

    #[tokio::test]
    async fn mock_client_request_permission_returns_error() {
        use agent_client_protocol::{
            Client, RequestPermissionRequest, ToolCallUpdate, ToolCallUpdateFields,
        };
        let client = MockClient::new(None);
        let tool_call = ToolCallUpdate::new("id", ToolCallUpdateFields::new());
        let result = client
            .request_permission(RequestPermissionRequest::new("s1", tool_call, vec![]))
            .await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn spawn_notification_forwarder_stops_on_client_error() {
        let client = MockClient::new(Some(0));
        let received = client.received.clone();
        let (tx, rx) = tokio::sync::mpsc::channel(16);

        let local = tokio::task::LocalSet::new();
        local
            .run_until(async {
                spawn_notification_forwarder(client, rx);

                let notif = SessionNotification::new(
                    "s1",
                    SessionUpdate::AgentMessageChunk(agent_client_protocol::ContentChunk::new(
                        agent_client_protocol::ContentBlock::Text(
                            agent_client_protocol::TextContent::new("hello"),
                        ),
                    )),
                );
                let _ = tx.send(notif).await;
                drop(tx);

                tokio::task::yield_now().await;
                tokio::task::yield_now().await;
            })
            .await;

        assert_eq!(received.borrow().len(), 0);
    }
}
