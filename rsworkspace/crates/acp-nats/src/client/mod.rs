pub(crate) mod session_update;

use crate::agent::Bridge;
use crate::in_flight_slot_guard::InFlightSlotGuard;
use crate::nats::{
    ClientMethod, FlushClient, PublishClient, RequestClient, SubscribeClient, client,
    parse_client_subject,
};
use agent_client_protocol::Client;
use async_nats::Message;
use bytes::Bytes;
use futures::StreamExt;
use std::cell::Cell;
use std::rc::Rc;
use tracing::{Span, error, info, instrument, warn};
use trogon_std::time::GetElapsed;

/// Runs the client proxy, subscribing to client subjects and dispatching to handlers.
///
/// # Panics / Runtime requirement
/// This function uses [`tokio::task::spawn_local`] internally and **must** be called from within
/// a [`tokio::task::LocalSet`] (or any executor that supports `!Send` tasks). Calling it outside
/// a `LocalSet` will panic at runtime when the first message is dispatched.
pub async fn run<
    N: SubscribeClient + RequestClient + PublishClient + FlushClient,
    Cl: Client + 'static,
    C: GetElapsed + 'static,
>(
    nats: N,
    client: Rc<Cl>,
    bridge: Rc<Bridge<N, C>>,
) {
    let wildcard = client::wildcards::all(bridge.config.acp_prefix());
    info!("Starting client proxy - subscribing to {}", wildcard);

    // TODO: change `run` to return `Result` and propagate this error once there is a caller.
    let mut subscriber = match nats.subscribe(wildcard).await {
        Ok(sub) => sub,
        Err(e) => {
            error!(error = %e, "Failed to subscribe to client subjects");
            return;
        }
    };

    let in_flight = Rc::new(Cell::new(0usize));
    let max_concurrent = bridge.config.max_concurrent_client_tasks();

    while let Some(msg) = subscriber.next().await {
        process_message(
            msg,
            &nats,
            client.clone(),
            bridge.clone(),
            &in_flight,
            max_concurrent,
        )
        .await;
    }

    info!("Client proxy subscriber ended");
}

async fn process_message<
    N: SubscribeClient + RequestClient + PublishClient + FlushClient,
    Cl: Client + 'static,
    C: GetElapsed + 'static,
>(
    msg: Message,
    nats: &N,
    client: Rc<Cl>,
    bridge: Rc<Bridge<N, C>>,
    in_flight: &Rc<Cell<usize>>,
    max_concurrent: usize,
) {
    let subject = msg.subject.to_string();

    // Validate subject before backpressure so unrecognised methods always
    // get InvalidParams, not a misleading "Bridge overloaded" error.
    let parsed = match parse_client_subject(&subject) {
        Some(parsed) => parsed,
        None => {
            warn!(subject = %subject, "Failed to parse client subject");
            return;
        }
    };

    let current_in_flight = in_flight.get();
    if current_in_flight >= max_concurrent {
        warn!(
            in_flight = current_in_flight,
            method = ?parsed.method,
            subject = %subject,
            "Client task backpressure — rejecting message"
        );
        bridge
            .metrics
            .record_error("client", "client_backpressure_rejected");

        return;
    }

    let payload = msg.payload.clone();
    let nats = nats.clone();

    let bridge_clone = bridge.clone();
    let in_flight_guard = InFlightSlotGuard::new(in_flight.clone());
    tokio::task::spawn_local(async move {
        let _in_flight_guard = in_flight_guard;
        dispatch_client_method(
            &subject,
            parsed,
            payload,
            &nats,
            client.as_ref(),
            bridge_clone.as_ref(),
        )
        .await;
    });
}

#[instrument(skip(payload, _nats, client, _bridge), fields(subject = %subject, session_id = tracing::field::Empty))]
async fn dispatch_client_method<
    N: SubscribeClient + RequestClient + PublishClient + FlushClient,
    Cl: Client,
    C: GetElapsed,
>(
    subject: &str,
    parsed: crate::nats::ParsedClientSubject,
    payload: Bytes,
    _nats: &N,
    client: &Cl,
    _bridge: &Bridge<N, C>,
) {
    Span::current().record("session_id", parsed.session_id.as_str());

    match parsed.method {
        ClientMethod::SessionUpdate => {
            session_update::handle(&payload, client, &parsed.session_id).await;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::session_id::AcpSessionId;
    use agent_client_protocol::{
        ContentBlock, ContentChunk, RequestPermissionRequest, RequestPermissionResponse,
        SessionNotification, SessionUpdate,
    };
    use async_trait::async_trait;
    use std::cell::RefCell;
    use trogon_nats::MockNatsClient;
    use trogon_std::time::SystemClock;

    struct MockClient {
        notifications: RefCell<Vec<String>>,
    }

    impl MockClient {
        fn new() -> Self {
            Self {
                notifications: RefCell::new(Vec::new()),
            }
        }
    }

    #[async_trait(?Send)]
    impl Client for MockClient {
        async fn session_notification(
            &self,
            n: agent_client_protocol::SessionNotification,
        ) -> agent_client_protocol::Result<()> {
            self.notifications.borrow_mut().push(format!("{:?}", n));
            Ok(())
        }

        async fn request_permission(
            &self,
            _: RequestPermissionRequest,
        ) -> agent_client_protocol::Result<RequestPermissionResponse> {
            Err(agent_client_protocol::Error::new(
                -32603,
                "not implemented in test mock",
            ))
        }
    }

    fn make_msg(subject: &str, payload: &[u8], reply: Option<&str>) -> async_nats::Message {
        async_nats::Message {
            subject: subject.into(),
            reply: reply.map(|r| r.into()),
            payload: payload.to_vec().into(),
            headers: None,
            length: payload.len(),
            status: None,
            description: None,
        }
    }

    fn make_bridge(nats: MockNatsClient) -> Rc<Bridge<MockNatsClient, SystemClock>> {
        Rc::new(Bridge::new(
            nats,
            SystemClock,
            &opentelemetry::global::meter("acp-nats-test"),
            crate::config::Config::for_test("acp"),
        ))
    }

    #[tokio::test]
    async fn mock_client_request_permission_returns_err() {
        let client = MockClient::new();
        let req: RequestPermissionRequest = serde_json::from_value(serde_json::json!({
            "sessionId": "sess-1",
            "toolCall": {
                "toolCallId": "call-1"
            },
            "options": []
        }))
        .unwrap();
        let result = client.request_permission(req).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn run_returns_early_when_subscribe_fails() {
        let nats = MockNatsClient::new();
        let bridge = make_bridge(nats.clone());
        let client = Rc::new(MockClient::new());

        run(nats, client, bridge).await;
    }

    #[tokio::test]
    async fn run_processes_messages_then_exits_when_stream_ends() {
        let local = tokio::task::LocalSet::new();
        local
            .run_until(async {
                let nats = MockNatsClient::new();
                let bridge = make_bridge(nats.clone());
                let client = Rc::new(MockClient::new());

                let notification = SessionNotification::new(
                    "sess1",
                    SessionUpdate::AgentMessageChunk(ContentChunk::new(ContentBlock::from("hi"))),
                );
                let payload = serde_json::to_vec(&notification).unwrap();
                let msg = make_msg("acp.sess1.client.session.update", &payload, None);

                let tx = nats.inject_messages();
                tx.unbounded_send(msg).unwrap();
                drop(tx);

                run(nats, client.clone(), bridge).await;

                tokio::task::yield_now().await;
                assert_eq!(client.notifications.borrow().len(), 1);
            })
            .await;
    }

    #[tokio::test]
    async fn dispatch_client_method_dispatches_session_update() {
        let nats = MockNatsClient::new();
        let bridge = Bridge::new(
            nats.clone(),
            SystemClock,
            &opentelemetry::global::meter("acp-nats-test"),
            crate::config::Config::for_test("acp"),
        );
        let client = MockClient::new();
        let session_id = AcpSessionId::new("sess-1").unwrap();

        let notification = SessionNotification::new(
            "sess-1",
            SessionUpdate::AgentMessageChunk(ContentChunk::new(ContentBlock::from("hi"))),
        );
        let payload = bytes::Bytes::from(serde_json::to_vec(&notification).unwrap());

        let parsed = crate::nats::ParsedClientSubject {
            session_id,
            method: ClientMethod::SessionUpdate,
        };

        dispatch_client_method(
            "acp.sess-1.client.session.update",
            parsed,
            payload,
            &nats,
            &client,
            &bridge,
        )
        .await;

        assert_eq!(client.notifications.borrow().len(), 1);
    }

    #[tokio::test]
    async fn process_message_invalid_subject_no_reply_does_not_publish() {
        let nats = MockNatsClient::new();
        let bridge = make_bridge(nats.clone());
        let client = Rc::new(MockClient::new());
        let in_flight = Rc::new(Cell::new(0usize));

        let msg = make_msg("acp.sess.unknown.method", b"{}", None);
        process_message(msg, &nats, client, bridge, &in_flight, 256).await;

        assert!(nats.published_messages().is_empty());
    }

    #[tokio::test]
    async fn process_message_invalid_subject_with_reply_is_ignored() {
        let nats = MockNatsClient::new();
        let bridge = make_bridge(nats.clone());
        let client = Rc::new(MockClient::new());
        let in_flight = Rc::new(Cell::new(0usize));

        let msg = make_msg("acp.sess.unknown.method", b"{}", Some("_INBOX.reply"));
        process_message(msg, &nats, client, bridge, &in_flight, 256).await;

        assert!(nats.published_messages().is_empty());
    }

    #[tokio::test]
    async fn process_message_backpressure_no_reply_does_not_publish() {
        let nats = MockNatsClient::new();
        let bridge = make_bridge(nats.clone());
        let client = Rc::new(MockClient::new());
        let in_flight = Rc::new(Cell::new(1usize));

        let msg = make_msg("acp.sess1.client.session.update", b"{}", None);
        process_message(msg, &nats, client, bridge, &in_flight, 1).await;

        assert!(nats.published_messages().is_empty());
    }

    #[tokio::test]
    async fn process_message_valid_dispatch_spawns_task() {
        let local = tokio::task::LocalSet::new();
        local
            .run_until(async {
                let nats = MockNatsClient::new();
                let bridge = make_bridge(nats.clone());
                let client = Rc::new(MockClient::new());
                let in_flight = Rc::new(Cell::new(0usize));

                let notification = SessionNotification::new(
                    "sess1",
                    SessionUpdate::AgentMessageChunk(ContentChunk::new(ContentBlock::from("hi"))),
                );
                let payload = serde_json::to_vec(&notification).unwrap();

                let msg = make_msg("acp.sess1.client.session.update", &payload, None);
                process_message(msg, &nats, client.clone(), bridge, &in_flight, 256).await;

                // Yield to allow the spawned local task to run.
                tokio::task::yield_now().await;

                assert_eq!(client.notifications.borrow().len(), 1);
            })
            .await;
    }
}
