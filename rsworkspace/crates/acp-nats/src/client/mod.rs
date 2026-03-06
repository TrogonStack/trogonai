pub(crate) mod fs_read_text_file;
pub(crate) mod session_update;

use crate::agent::Bridge;
use crate::error::AGENT_UNAVAILABLE;
use crate::in_flight_slot_guard::InFlightSlotGuard;
use crate::jsonrpc::extract_request_id;
use crate::nats::{
    ClientMethod, FlushClient, PublishClient, RequestClient, SubscribeClient, client,
    headers_with_trace_context, parse_client_subject,
};
use agent_client_protocol::{Client, Error, ErrorCode, Response};
use async_nats::Message;
use bytes::Bytes;
use futures::StreamExt;
use std::cell::Cell;
use std::rc::Rc;
use tracing::{Span, error, info, instrument, warn};
use trogon_std::JsonSerialize;
use trogon_std::time::GetElapsed;

const CONTENT_TYPE_JSON: &str = "application/json";
const CONTENT_TYPE_PLAIN: &str = "text/plain";

async fn publish_backpressure_error_reply<N: PublishClient + FlushClient, S: JsonSerialize>(
    nats: &N,
    payload: &[u8],
    reply_to: &str,
    serializer: &S,
) {
    let request_id = extract_request_id(payload);
    let response = Response::<()>::Error {
        id: request_id,
        error: Error::new(
            i32::from(ErrorCode::Other(AGENT_UNAVAILABLE)),
            "Client proxy overloaded; retry with backoff",
        ),
    };
    let (bytes, content_type) = serializer
        .to_vec(&response)
        .or_else(|e| {
            warn!(error = %e, "JSON serialization of backpressure error failed, using fallback");
            serializer.to_vec(&Response::<()>::Error {
                id: agent_client_protocol::RequestId::Null,
                error: Error::new(-32603, "Internal error"),
            })
        })
        .map(|v| (Bytes::from(v), CONTENT_TYPE_JSON))
        .unwrap_or_else(|e| {
            warn!(error = %e, "Fallback JSON serialization failed, response may not be valid JSON-RPC");
            (Bytes::from("Internal error"), CONTENT_TYPE_PLAIN)
        });
    let mut headers = headers_with_trace_context();
    headers.insert("Content-Type", content_type);
    if let Err(e) = nats
        .publish_with_headers(reply_to.to_string(), headers, bytes)
        .await
    {
        warn!(error = %e, "Failed to publish backpressure error reply");
    }
    if let Err(e) = nats.flush().await {
        warn!(error = %e, "Failed to flush backpressure error reply");
    }
}

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
    S: Clone + JsonSerialize + 'static,
>(
    nats: N,
    client: Rc<Cl>,
    bridge: Rc<Bridge<N, C>>,
    serializer: S,
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
            &serializer,
        )
        .await;
    }

    info!("Client proxy subscriber ended");
}

async fn process_message<
    N: SubscribeClient + RequestClient + PublishClient + FlushClient,
    Cl: Client + 'static,
    C: GetElapsed + 'static,
    S: Clone + JsonSerialize + 'static,
>(
    msg: Message,
    nats: &N,
    client: Rc<Cl>,
    bridge: Rc<Bridge<N, C>>,
    in_flight: &Rc<Cell<usize>>,
    max_concurrent: usize,
    serializer: &S,
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

    let payload = msg.payload.clone();
    let reply = msg.reply.as_ref().map(|r| r.to_string());

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

        if let Some(reply_to) = &reply {
            publish_backpressure_error_reply(nats, &payload, reply_to, serializer).await;
        }
        return;
    }
    let nats = nats.clone();
    let serializer = serializer.clone();
    let in_flight_guard = InFlightSlotGuard::new(in_flight.clone());
    tokio::task::spawn_local(async move {
        let _in_flight_guard = in_flight_guard;
        let ctx = DispatchContext {
            nats: &nats,
            client: client.as_ref(),
            serializer: &serializer,
        };
        dispatch_client_method(&subject, parsed, payload, reply, &ctx).await;
    });
}

struct DispatchContext<'a, N, Cl, S>
where
    N: RequestClient + PublishClient + FlushClient,
{
    nats: &'a N,
    client: &'a Cl,
    serializer: &'a S,
}

#[instrument(skip(payload, ctx), fields(subject = %subject, session_id = tracing::field::Empty))]
async fn dispatch_client_method<
    N: SubscribeClient + RequestClient + PublishClient + FlushClient,
    Cl: Client,
    S: JsonSerialize,
>(
    subject: &str,
    parsed: crate::nats::ParsedClientSubject,
    payload: Bytes,
    reply: Option<String>,
    ctx: &DispatchContext<'_, N, Cl, S>,
) {
    Span::current().record("session_id", parsed.session_id.as_str());

    match parsed.method {
        ClientMethod::FsReadTextFile => {
            fs_read_text_file::handle(
                &payload,
                ctx.client,
                reply.as_deref(),
                ctx.nats,
                parsed.session_id.as_str(),
                ctx.serializer,
            )
            .await;
        }
        ClientMethod::SessionUpdate => {
            session_update::handle(&payload, ctx.client, &parsed.session_id).await;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::session_id::AcpSessionId;
    use agent_client_protocol::{
        ContentBlock, ContentChunk, ReadTextFileRequest, ReadTextFileResponse, Request, RequestId,
        RequestPermissionRequest, RequestPermissionResponse, SessionNotification, SessionUpdate,
    };
    use async_trait::async_trait;
    use std::cell::RefCell;
    use trogon_nats::{AdvancedMockNatsClient, MockNatsClient};
    use trogon_std::time::SystemClock;
    use trogon_std::{FailNextSerialize, StdJsonSerialize};

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

        async fn read_text_file(
            &self,
            _: ReadTextFileRequest,
        ) -> agent_client_protocol::Result<ReadTextFileResponse> {
            Ok(ReadTextFileResponse::new("mock file content".to_string()))
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

    fn make_bridge_advanced(
        nats: AdvancedMockNatsClient,
    ) -> Rc<Bridge<AdvancedMockNatsClient, SystemClock>> {
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

        run(nats, client, bridge, StdJsonSerialize).await;
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

                run(nats, client.clone(), bridge, StdJsonSerialize).await;

                tokio::task::yield_now().await;
                assert_eq!(client.notifications.borrow().len(), 1);
            })
            .await;
    }

    #[tokio::test]
    async fn dispatch_client_method_dispatches_session_update() {
        let nats = MockNatsClient::new();
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

        let ctx = DispatchContext {
            nats: &nats,
            client: &client,
            serializer: &StdJsonSerialize,
        };
        dispatch_client_method(
            "acp.sess-1.client.session.update",
            parsed,
            payload,
            None,
            &ctx,
        )
        .await;

        assert_eq!(client.notifications.borrow().len(), 1);
    }

    #[tokio::test]
    async fn dispatch_client_method_dispatches_fs_read_text_file() {
        let nats = MockNatsClient::new();
        let client = MockClient::new();
        let session_id = AcpSessionId::new("sess-1").unwrap();

        let envelope = Request {
            id: RequestId::Number(1),
            method: std::sync::Arc::from("fs/read_text_file"),
            params: Some(ReadTextFileRequest::new(
                agent_client_protocol::SessionId::from("sess-1"),
                "/tmp/foo.txt".to_string(),
            )),
        };
        let payload = bytes::Bytes::from(serde_json::to_vec(&envelope).unwrap());

        let parsed = crate::nats::ParsedClientSubject {
            session_id,
            method: ClientMethod::FsReadTextFile,
        };

        let ctx = DispatchContext {
            nats: &nats,
            client: &client,
            serializer: &StdJsonSerialize,
        };
        dispatch_client_method(
            "acp.sess-1.client.fs.read_text_file",
            parsed,
            payload,
            Some("_INBOX.reply".to_string()),
            &ctx,
        )
        .await;

        assert_eq!(nats.published_messages(), vec!["_INBOX.reply"]);
    }

    #[tokio::test]
    async fn dispatch_client_method_dispatches_fs_read_text_file_with_advanced_mock() {
        let nats = AdvancedMockNatsClient::new();
        let client = MockClient::new();
        let session_id = AcpSessionId::new("sess-1").unwrap();

        let envelope = Request {
            id: RequestId::Number(1),
            method: std::sync::Arc::from("fs/read_text_file"),
            params: Some(ReadTextFileRequest::new(
                agent_client_protocol::SessionId::from("sess-1"),
                "/tmp/foo.txt".to_string(),
            )),
        };
        let payload = bytes::Bytes::from(serde_json::to_vec(&envelope).unwrap());

        let parsed = crate::nats::ParsedClientSubject {
            session_id,
            method: ClientMethod::FsReadTextFile,
        };

        let ctx = DispatchContext {
            nats: &nats,
            client: &client,
            serializer: &StdJsonSerialize,
        };
        dispatch_client_method(
            "acp.sess-1.client.fs.read_text_file",
            parsed,
            payload,
            Some("_INBOX.reply".to_string()),
            &ctx,
        )
        .await;

        assert_eq!(nats.published_messages(), vec!["_INBOX.reply"]);
    }

    #[tokio::test]
    async fn dispatch_client_method_dispatches_fs_read_text_file_serialization_fallback() {
        let nats = MockNatsClient::new();
        let client = MockClient::new();
        let session_id = AcpSessionId::new("sess-1").unwrap();
        let serializer = FailNextSerialize::new(1);

        let envelope = Request {
            id: RequestId::Number(1),
            method: std::sync::Arc::from("fs/read_text_file"),
            params: Some(ReadTextFileRequest::new(
                agent_client_protocol::SessionId::from("sess-1"),
                "/tmp/foo.txt".to_string(),
            )),
        };
        let payload = bytes::Bytes::from(serde_json::to_vec(&envelope).unwrap());

        let parsed = crate::nats::ParsedClientSubject {
            session_id,
            method: ClientMethod::FsReadTextFile,
        };

        let ctx = DispatchContext {
            nats: &nats,
            client: &client,
            serializer: &serializer,
        };
        dispatch_client_method(
            "acp.sess-1.client.fs.read_text_file",
            parsed,
            payload,
            Some("_INBOX.reply".to_string()),
            &ctx,
        )
        .await;

        assert_eq!(nats.published_messages(), vec!["_INBOX.reply"]);
    }

    #[tokio::test]
    async fn process_message_invalid_subject_no_reply_does_not_publish() {
        let nats = MockNatsClient::new();
        let bridge = make_bridge(nats.clone());
        let client = Rc::new(MockClient::new());
        let in_flight = Rc::new(Cell::new(0usize));

        let msg = make_msg("acp.sess.unknown.method", b"{}", None);
        process_message(
            msg,
            &nats,
            client,
            bridge,
            &in_flight,
            256,
            &StdJsonSerialize,
        )
        .await;

        assert!(nats.published_messages().is_empty());
    }

    #[tokio::test]
    async fn process_message_invalid_subject_with_reply_is_ignored() {
        let nats = MockNatsClient::new();
        let bridge = make_bridge(nats.clone());
        let client = Rc::new(MockClient::new());
        let in_flight = Rc::new(Cell::new(0usize));

        let msg = make_msg("acp.sess.unknown.method", b"{}", Some("_INBOX.reply"));
        process_message(
            msg,
            &nats,
            client,
            bridge,
            &in_flight,
            256,
            &StdJsonSerialize,
        )
        .await;

        assert!(nats.published_messages().is_empty());
    }

    #[tokio::test]
    async fn process_message_backpressure_no_reply_does_not_publish() {
        let nats = MockNatsClient::new();
        let bridge = make_bridge(nats.clone());
        let client = Rc::new(MockClient::new());
        let in_flight = Rc::new(Cell::new(1usize));

        let msg = make_msg("acp.sess1.client.session.update", b"{}", None);
        process_message(msg, &nats, client, bridge, &in_flight, 1, &StdJsonSerialize).await;

        assert!(nats.published_messages().is_empty());
    }

    #[tokio::test]
    async fn process_message_backpressure_with_reply_publishes_error() {
        let nats = MockNatsClient::new();
        let bridge = make_bridge(nats.clone());
        let client = Rc::new(MockClient::new());
        let in_flight = Rc::new(Cell::new(1usize));

        let envelope = Request {
            id: RequestId::Number(1),
            method: std::sync::Arc::from("fs/read_text_file"),
            params: Some(ReadTextFileRequest::new(
                agent_client_protocol::SessionId::from("sess1"),
                "/tmp/foo.txt".to_string(),
            )),
        };
        let payload = serde_json::to_vec(&envelope).unwrap();
        let msg = make_msg(
            "acp.sess1.client.fs.read_text_file",
            &payload,
            Some("_INBOX.reply"),
        );
        process_message(msg, &nats, client, bridge, &in_flight, 1, &StdJsonSerialize).await;

        assert_eq!(nats.published_messages(), vec!["_INBOX.reply"]);
    }

    #[tokio::test]
    async fn process_message_backpressure_with_reply_flush_failure_exercises_warn_path() {
        let nats = AdvancedMockNatsClient::new();
        nats.fail_next_flush();
        let bridge = make_bridge_advanced(nats.clone());
        let client = Rc::new(MockClient::new());
        let in_flight = Rc::new(Cell::new(1usize));

        let envelope = Request {
            id: RequestId::Number(1),
            method: std::sync::Arc::from("fs/read_text_file"),
            params: Some(ReadTextFileRequest::new(
                agent_client_protocol::SessionId::from("sess1"),
                "/tmp/foo.txt".to_string(),
            )),
        };
        let payload = serde_json::to_vec(&envelope).unwrap();
        let msg = make_msg(
            "acp.sess1.client.fs.read_text_file",
            &payload,
            Some("_INBOX.reply"),
        );
        process_message(msg, &nats, client, bridge, &in_flight, 1, &StdJsonSerialize).await;

        assert_eq!(nats.published_messages(), vec!["_INBOX.reply"]);
    }

    #[tokio::test]
    async fn process_message_backpressure_with_reply_publish_failure_exercises_error_path() {
        let nats = AdvancedMockNatsClient::new();
        nats.fail_next_publish();
        let bridge = make_bridge_advanced(nats.clone());
        let client = Rc::new(MockClient::new());
        let in_flight = Rc::new(Cell::new(1usize));

        let envelope = Request {
            id: RequestId::Number(1),
            method: std::sync::Arc::from("fs/read_text_file"),
            params: Some(ReadTextFileRequest::new(
                agent_client_protocol::SessionId::from("sess1"),
                "/tmp/foo.txt".to_string(),
            )),
        };
        let payload = serde_json::to_vec(&envelope).unwrap();
        let msg = make_msg(
            "acp.sess1.client.fs.read_text_file",
            &payload,
            Some("_INBOX.reply"),
        );
        process_message(msg, &nats, client, bridge, &in_flight, 1, &StdJsonSerialize).await;

        assert!(nats.published_messages().is_empty());
    }

    #[tokio::test]
    async fn process_message_backpressure_first_serialize_fails_uses_fallback() {
        let nats = MockNatsClient::new();
        let bridge = make_bridge(nats.clone());
        let client = Rc::new(MockClient::new());
        let in_flight = Rc::new(Cell::new(1usize));
        let serializer = FailNextSerialize::new(1);

        let envelope = Request {
            id: RequestId::Number(1),
            method: std::sync::Arc::from("fs/read_text_file"),
            params: Some(ReadTextFileRequest::new(
                agent_client_protocol::SessionId::from("sess1"),
                "/tmp/foo.txt".to_string(),
            )),
        };
        let payload = serde_json::to_vec(&envelope).unwrap();
        let msg = make_msg(
            "acp.sess1.client.fs.read_text_file",
            &payload,
            Some("_INBOX.reply"),
        );
        process_message(msg, &nats, client, bridge, &in_flight, 1, &serializer).await;

        assert_eq!(nats.published_messages(), vec!["_INBOX.reply"]);
    }

    #[tokio::test]
    async fn process_message_backpressure_serialization_fallback_uses_plain_text() {
        let nats = MockNatsClient::new();
        let bridge = make_bridge(nats.clone());
        let client = Rc::new(MockClient::new());
        let in_flight = Rc::new(Cell::new(1usize));
        let serializer = FailNextSerialize::new(2);

        let envelope = Request {
            id: RequestId::Number(1),
            method: std::sync::Arc::from("fs/read_text_file"),
            params: Some(ReadTextFileRequest::new(
                agent_client_protocol::SessionId::from("sess1"),
                "/tmp/foo.txt".to_string(),
            )),
        };
        let payload = serde_json::to_vec(&envelope).unwrap();
        let msg = make_msg(
            "acp.sess1.client.fs.read_text_file",
            &payload,
            Some("_INBOX.reply"),
        );
        process_message(msg, &nats, client, bridge, &in_flight, 1, &serializer).await;

        assert_eq!(nats.published_messages(), vec!["_INBOX.reply"]);
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
                process_message(
                    msg,
                    &nats,
                    client.clone(),
                    bridge,
                    &in_flight,
                    256,
                    &StdJsonSerialize,
                )
                .await;

                // Yield to allow the spawned local task to run.
                tokio::task::yield_now().await;

                assert_eq!(client.notifications.borrow().len(), 1);
            })
            .await;
    }
}
