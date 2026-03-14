pub(crate) mod ext_session_prompt_response;
pub(crate) mod fs_read_text_file;
pub(crate) mod fs_write_text_file;
pub(crate) mod request_permission;
pub(crate) mod rpc_reply;
pub(crate) mod session_update;
pub(crate) mod terminal_create;
pub(crate) mod terminal_kill;
pub(crate) mod terminal_output;
pub(crate) mod terminal_release;
pub(crate) mod terminal_wait_for_exit;

use crate::agent::Bridge;
use crate::error::AGENT_UNAVAILABLE;
use crate::in_flight_slot_guard::InFlightSlotGuard;
use crate::jsonrpc::extract_request_id;
use crate::nats::{
    ClientMethod, FlushClient, PublishClient, RequestClient, SubscribeClient, client,
    parse_client_subject,
};
use agent_client_protocol::{Client, ErrorCode};
use async_nats::Message;
use bytes::Bytes;
use futures::StreamExt;
use std::cell::Cell;
use std::rc::Rc;
use tracing::{Span, error, info, instrument, warn};
use trogon_std::JsonSerialize;
use trogon_std::time::GetElapsed;

async fn publish_backpressure_error_reply<N: PublishClient + FlushClient, S: JsonSerialize>(
    nats: &N,
    payload: &[u8],
    reply_to: &str,
    serializer: &S,
) {
    let request_id = extract_request_id(payload);
    let (bytes, content_type) = rpc_reply::error_response_bytes(
        serializer,
        request_id,
        ErrorCode::Other(AGENT_UNAVAILABLE),
        "Client proxy overloaded; retry with backoff",
    );
    rpc_reply::publish_reply(
        nats,
        reply_to,
        bytes,
        content_type,
        "backpressure error reply",
    )
    .await;
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
            bridge: bridge.as_ref(),
            serializer: &serializer,
        };
        dispatch_client_method(&subject, parsed, payload, reply, &ctx).await;
    });
}

struct DispatchContext<'a, N, Cl, C, S>
where
    N: RequestClient + PublishClient + FlushClient,
    C: GetElapsed + 'static,
{
    nats: &'a N,
    client: &'a Cl,
    bridge: &'a Bridge<N, C>,
    serializer: &'a S,
}

#[instrument(skip(payload, ctx), fields(subject = %subject, session_id = tracing::field::Empty))]
async fn dispatch_client_method<
    N: SubscribeClient + RequestClient + PublishClient + FlushClient,
    Cl: Client,
    C: GetElapsed + 'static,
    S: JsonSerialize,
>(
    subject: &str,
    parsed: crate::nats::ParsedClientSubject,
    payload: Bytes,
    reply: Option<String>,
    ctx: &DispatchContext<'_, N, Cl, C, S>,
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
        ClientMethod::FsWriteTextFile => {
            fs_write_text_file::handle(
                &payload,
                ctx.client,
                reply.as_deref(),
                ctx.nats,
                parsed.session_id.as_str(),
                ctx.serializer,
            )
            .await;
        }
        ClientMethod::SessionRequestPermission => {
            request_permission::handle(
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
            session_update::handle(&payload, ctx.client, reply.is_some()).await;
        }
        ClientMethod::ExtSessionPromptResponse => {
            ext_session_prompt_response::handle(
                parsed.session_id.as_str(),
                &payload,
                reply.as_deref(),
                ctx.bridge,
            )
            .await;
        }
        ClientMethod::TerminalCreate => {
            terminal_create::handle(
                &payload,
                ctx.client,
                reply.as_deref(),
                ctx.nats,
                parsed.session_id.as_str(),
                ctx.serializer,
            )
            .await;
        }
        ClientMethod::TerminalKill => {
            terminal_kill::handle(
                &payload,
                ctx.client,
                reply.as_deref(),
                ctx.nats,
                parsed.session_id.as_str(),
                ctx.serializer,
            )
            .await;
        }
        ClientMethod::TerminalOutput => {
            terminal_output::handle(
                &payload,
                ctx.client,
                reply.as_deref(),
                ctx.nats,
                parsed.session_id.as_str(),
                ctx.serializer,
            )
            .await;
        }
        ClientMethod::TerminalRelease => {
            terminal_release::handle(
                &payload,
                ctx.client,
                reply.as_deref(),
                ctx.nats,
                parsed.session_id.as_str(),
                ctx.serializer,
            )
            .await;
        }
        ClientMethod::TerminalWaitForExit => {
            terminal_wait_for_exit::handle(
                &payload,
                ctx.client,
                reply.as_deref(),
                ctx.nats,
                parsed.session_id.as_str(),
                ctx.bridge.config.operation_timeout(),
                ctx.serializer,
            )
            .await;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::session_id::AcpSessionId;
    use agent_client_protocol::{
        ContentBlock, ContentChunk, CreateTerminalRequest, CreateTerminalResponse,
        KillTerminalCommandRequest, KillTerminalCommandResponse, ReadTextFileRequest,
        ReadTextFileResponse, ReleaseTerminalRequest, ReleaseTerminalResponse, Request, RequestId,
        RequestPermissionOutcome, RequestPermissionRequest, RequestPermissionResponse,
        SessionNotification, SessionUpdate, TerminalExitStatus, TerminalOutputRequest,
        TerminalOutputResponse, ToolCallUpdate, ToolCallUpdateFields, WaitForTerminalExitRequest,
        WaitForTerminalExitResponse, WriteTextFileRequest, WriteTextFileResponse,
    };
    use async_trait::async_trait;
    use std::cell::RefCell;
    use trogon_nats::{AdvancedMockNatsClient, MockNatsClient};
    use trogon_std::time::SystemClock;
    use trogon_std::{FailNextSerialize, StdJsonSerialize};

    pub(super) struct MockClient {
        notifications: RefCell<Vec<String>>,
        kill_terminal_calls: RefCell<usize>,
        terminal_output_calls: RefCell<usize>,
        terminal_release_calls: RefCell<usize>,
        wait_for_terminal_exit_calls: RefCell<usize>,
    }

    impl MockClient {
        pub(super) fn new() -> Self {
            Self {
                notifications: RefCell::new(Vec::new()),
                kill_terminal_calls: RefCell::new(0),
                terminal_output_calls: RefCell::new(0),
                terminal_release_calls: RefCell::new(0),
                wait_for_terminal_exit_calls: RefCell::new(0),
            }
        }

        pub(super) fn kill_terminal_call_count(&self) -> usize {
            *self.kill_terminal_calls.borrow()
        }

        pub(super) fn terminal_output_call_count(&self) -> usize {
            *self.terminal_output_calls.borrow()
        }

        pub(super) fn terminal_release_call_count(&self) -> usize {
            *self.terminal_release_calls.borrow()
        }

        pub(super) fn wait_for_terminal_exit_call_count(&self) -> usize {
            *self.wait_for_terminal_exit_calls.borrow()
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

        async fn write_text_file(
            &self,
            _: WriteTextFileRequest,
        ) -> agent_client_protocol::Result<WriteTextFileResponse> {
            Ok(WriteTextFileResponse::new())
        }

        async fn create_terminal(
            &self,
            _: CreateTerminalRequest,
        ) -> agent_client_protocol::Result<CreateTerminalResponse> {
            Ok(CreateTerminalResponse::new("term-001"))
        }

        async fn kill_terminal_command(
            &self,
            _: KillTerminalCommandRequest,
        ) -> agent_client_protocol::Result<KillTerminalCommandResponse> {
            *self.kill_terminal_calls.borrow_mut() += 1;
            Ok(KillTerminalCommandResponse::new())
        }

        async fn terminal_output(
            &self,
            _: TerminalOutputRequest,
        ) -> agent_client_protocol::Result<TerminalOutputResponse> {
            *self.terminal_output_calls.borrow_mut() += 1;
            Ok(TerminalOutputResponse::new(
                "mock output".to_string(),
                false,
            ))
        }

        async fn release_terminal(
            &self,
            _: ReleaseTerminalRequest,
        ) -> agent_client_protocol::Result<ReleaseTerminalResponse> {
            *self.terminal_release_calls.borrow_mut() += 1;
            Ok(ReleaseTerminalResponse::new())
        }

        async fn wait_for_terminal_exit(
            &self,
            _: WaitForTerminalExitRequest,
        ) -> agent_client_protocol::Result<WaitForTerminalExitResponse> {
            *self.wait_for_terminal_exit_calls.borrow_mut() += 1;
            Ok(WaitForTerminalExitResponse::new(
                TerminalExitStatus::new().exit_code(0u32),
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

    fn make_bridge_with_operation_timeout(
        nats: MockNatsClient,
        operation_timeout: std::time::Duration,
    ) -> Rc<Bridge<MockNatsClient, SystemClock>> {
        Rc::new(Bridge::new(
            nats,
            SystemClock,
            &opentelemetry::global::meter("acp-nats-test"),
            crate::config::Config::for_test("acp").with_operation_timeout(operation_timeout),
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

        let bridge = make_bridge(nats.clone());
        let ctx = DispatchContext {
            nats: &nats,
            client: &client,
            bridge: &bridge,
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

        let bridge = make_bridge(nats.clone());
        let ctx = DispatchContext {
            nats: &nats,
            client: &client,
            bridge: &bridge,
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
    async fn dispatch_client_method_dispatches_fs_write_text_file() {
        let nats = MockNatsClient::new();
        let client = MockClient::new();
        let session_id = AcpSessionId::new("sess-1").unwrap();

        let envelope = Request {
            id: RequestId::Number(42),
            method: std::sync::Arc::from("fs/write_text_file"),
            params: Some(WriteTextFileRequest::new(
                agent_client_protocol::SessionId::from("sess-1"),
                "/tmp/test.txt".to_string(),
                "content".to_string(),
            )),
        };
        let payload = bytes::Bytes::from(serde_json::to_vec(&envelope).unwrap());

        let parsed = crate::nats::ParsedClientSubject {
            session_id,
            method: ClientMethod::FsWriteTextFile,
        };

        let bridge = make_bridge(nats.clone());
        let ctx = DispatchContext {
            nats: &nats,
            client: &client,
            bridge: &bridge,
            serializer: &StdJsonSerialize,
        };
        dispatch_client_method(
            "acp.sess-1.client.fs.write_text_file",
            parsed,
            payload,
            Some("_INBOX.reply".to_string()),
            &ctx,
        )
        .await;

        assert_eq!(nats.published_messages(), vec!["_INBOX.reply"]);
        let payloads = nats.published_payloads();
        assert_eq!(payloads.len(), 1);
        let response: serde_json::Value =
            serde_json::from_slice(payloads[0].as_ref()).expect("response should be valid JSON");
        assert_eq!(
            response["id"], 42,
            "response must be JSON-RPC envelope with matching id"
        );
        assert!(
            response.get("result").is_some(),
            "success response must have result field"
        );
    }

    #[tokio::test]
    async fn fs_write_text_file_round_trip() {
        let nats = MockNatsClient::new();
        let client = MockClient::new();
        let envelope = Request {
            id: RequestId::Number(1),
            method: std::sync::Arc::from("fs/write_text_file"),
            params: Some(WriteTextFileRequest::new(
                agent_client_protocol::SessionId::from("session-001"),
                "/tmp/test.txt".to_string(),
                "content".to_string(),
            )),
        };
        let payload = serde_json::to_vec(&envelope).unwrap();

        fs_write_text_file::handle(
            &payload,
            &client,
            Some("_INBOX.reply"),
            &nats,
            "session-001",
            &StdJsonSerialize,
        )
        .await;

        assert_eq!(nats.published_messages(), vec!["_INBOX.reply"]);
        let payloads = nats.published_payloads();
        assert_eq!(payloads.len(), 1);
        let response: serde_json::Value = serde_json::from_slice(payloads[0].as_ref()).unwrap();
        assert_eq!(response["id"], 1);
        assert!(response.get("result").is_some());
    }

    #[tokio::test]
    async fn dispatch_client_method_dispatches_terminal_create() {
        let nats = MockNatsClient::new();
        let client = MockClient::new();
        let session_id = AcpSessionId::new("sess-1").unwrap();

        let envelope = Request {
            id: RequestId::Number(1),
            method: std::sync::Arc::from("terminal/create"),
            params: Some(CreateTerminalRequest::new("sess-1", "echo hello")),
        };
        let payload = bytes::Bytes::from(serde_json::to_vec(&envelope).unwrap());

        let parsed = crate::nats::ParsedClientSubject {
            session_id,
            method: ClientMethod::TerminalCreate,
        };

        let bridge = make_bridge(nats.clone());
        let ctx = DispatchContext {
            nats: &nats,
            client: &client,
            bridge: &bridge,
            serializer: &StdJsonSerialize,
        };
        dispatch_client_method(
            "acp.sess-1.client.terminal.create",
            parsed,
            payload,
            Some("_INBOX.reply".to_string()),
            &ctx,
        )
        .await;

        assert_eq!(nats.published_messages(), vec!["_INBOX.reply"]);
    }

    #[tokio::test]
    async fn dispatch_client_method_dispatches_terminal_create_session_id_mismatch_publishes_error_reply()
     {
        let nats = MockNatsClient::new();
        let client = MockClient::new();
        let session_id = AcpSessionId::new("sess-a").unwrap();

        let envelope = Request {
            id: RequestId::Number(1),
            method: std::sync::Arc::from("terminal/create"),
            params: Some(CreateTerminalRequest::new("sess-b", "echo hello")),
        };
        let payload = bytes::Bytes::from(serde_json::to_vec(&envelope).unwrap());

        let parsed = crate::nats::ParsedClientSubject {
            session_id,
            method: ClientMethod::TerminalCreate,
        };

        let bridge = make_bridge(nats.clone());
        let ctx = DispatchContext {
            nats: &nats,
            client: &client,
            bridge: &bridge,
            serializer: &StdJsonSerialize,
        };
        dispatch_client_method(
            "acp.sess-a.client.terminal.create",
            parsed,
            payload,
            Some("_INBOX.reply".to_string()),
            &ctx,
        )
        .await;

        assert_eq!(nats.published_messages(), vec!["_INBOX.reply"]);
    }

    #[tokio::test]
    async fn dispatch_client_method_dispatches_terminal_kill() {
        let nats = MockNatsClient::new();
        let client = MockClient::new();
        let session_id = AcpSessionId::new("sess-1").unwrap();

        let envelope = Request {
            id: RequestId::Number(1),
            method: std::sync::Arc::from("terminal/kill_command"),
            params: Some(KillTerminalCommandRequest::new(
                agent_client_protocol::SessionId::from("sess-1"),
                "term-001".to_string(),
            )),
        };
        let payload = bytes::Bytes::from(serde_json::to_vec(&envelope).unwrap());

        let parsed = crate::nats::ParsedClientSubject {
            session_id,
            method: ClientMethod::TerminalKill,
        };

        let bridge = make_bridge(nats.clone());
        let ctx = DispatchContext {
            nats: &nats,
            client: &client,
            bridge: &bridge,
            serializer: &StdJsonSerialize,
        };
        dispatch_client_method(
            "acp.sess-1.client.terminal.kill",
            parsed,
            payload,
            Some("_INBOX.reply".to_string()),
            &ctx,
        )
        .await;

        assert_eq!(nats.published_messages(), vec!["_INBOX.reply"]);
        let payloads = nats.published_payloads();
        assert_eq!(payloads.len(), 1);
        let response: serde_json::Value = serde_json::from_slice(payloads[0].as_ref()).unwrap();
        assert_eq!(response.get("id"), Some(&serde_json::Value::from(1)));
        assert!(response.get("result").is_some());
        assert!(response.get("error").is_none());
    }

    #[tokio::test]
    async fn dispatch_client_method_dispatches_terminal_output() {
        let nats = MockNatsClient::new();
        let client = MockClient::new();
        let session_id = AcpSessionId::new("sess-1").unwrap();

        let envelope = Request {
            id: RequestId::Number(1),
            method: std::sync::Arc::from("terminal/output"),
            params: Some(TerminalOutputRequest::new("sess-1", "term-001")),
        };
        let payload = bytes::Bytes::from(serde_json::to_vec(&envelope).unwrap());

        let parsed = crate::nats::ParsedClientSubject {
            session_id,
            method: ClientMethod::TerminalOutput,
        };

        let bridge = make_bridge(nats.clone());
        let ctx = DispatchContext {
            nats: &nats,
            client: &client,
            bridge: &bridge,
            serializer: &StdJsonSerialize,
        };
        dispatch_client_method(
            "acp.sess-1.client.terminal.output",
            parsed,
            payload,
            Some("_INBOX.reply".to_string()),
            &ctx,
        )
        .await;

        assert_eq!(nats.published_messages(), vec!["_INBOX.reply"]);
        let payloads = nats.published_payloads();
        assert_eq!(payloads.len(), 1);
        let response: serde_json::Value = serde_json::from_slice(payloads[0].as_ref()).unwrap();
        assert_eq!(response.get("id"), Some(&serde_json::Value::from(1)));
        assert!(response.get("result").is_some());
        assert!(response.get("error").is_none());
        assert_eq!(
            client.terminal_output_call_count(),
            1,
            "terminal_output handler must run"
        );
        assert_eq!(
            client.kill_terminal_call_count(),
            0,
            "kill handler must not run"
        );
    }

    #[tokio::test]
    async fn dispatch_client_method_dispatches_terminal_release() {
        let nats = MockNatsClient::new();
        let client = MockClient::new();
        let session_id = AcpSessionId::new("sess-1").unwrap();

        let envelope = Request {
            id: RequestId::Number(1),
            method: std::sync::Arc::from("terminal/release"),
            params: Some(ReleaseTerminalRequest::new("sess-1", "term-001")),
        };
        let payload = bytes::Bytes::from(serde_json::to_vec(&envelope).unwrap());

        let parsed = crate::nats::ParsedClientSubject {
            session_id,
            method: ClientMethod::TerminalRelease,
        };

        let bridge = make_bridge(nats.clone());
        let ctx = DispatchContext {
            nats: &nats,
            client: &client,
            bridge: &bridge,
            serializer: &StdJsonSerialize,
        };
        dispatch_client_method(
            "acp.sess-1.client.terminal.release",
            parsed,
            payload,
            Some("_INBOX.reply".to_string()),
            &ctx,
        )
        .await;

        assert_eq!(nats.published_messages(), vec!["_INBOX.reply"]);
        let payloads = nats.published_payloads();
        assert_eq!(payloads.len(), 1);
        let response: serde_json::Value = serde_json::from_slice(payloads[0].as_ref()).unwrap();
        assert_eq!(response.get("id"), Some(&serde_json::Value::from(1)));
        assert!(response.get("result").is_some());
        assert!(response.get("error").is_none());
        assert_eq!(
            client.terminal_release_call_count(),
            1,
            "terminal_release handler must run"
        );
    }

    #[tokio::test]
    async fn dispatch_client_method_terminal_wait_for_exit_reply_none_skips_handler() {
        let nats = MockNatsClient::new();
        let client = MockClient::new();
        let session_id = AcpSessionId::new("sess-1").unwrap();

        let envelope = Request {
            id: RequestId::Number(1),
            method: std::sync::Arc::from("terminal/wait_for_exit"),
            params: Some(WaitForTerminalExitRequest::new("sess-1", "term-001")),
        };
        let payload = bytes::Bytes::from(serde_json::to_vec(&envelope).unwrap());

        let parsed = crate::nats::ParsedClientSubject {
            session_id,
            method: ClientMethod::TerminalWaitForExit,
        };

        let bridge = make_bridge(nats.clone());
        let ctx = DispatchContext {
            nats: &nats,
            client: &client,
            bridge: &bridge,
            serializer: &StdJsonSerialize,
        };
        dispatch_client_method(
            "acp.sess-1.client.terminal.wait_for_exit",
            parsed,
            payload,
            None,
            &ctx,
        )
        .await;

        assert!(nats.published_messages().is_empty());
        assert_eq!(client.wait_for_terminal_exit_call_count(), 0);
    }

    #[tokio::test]
    async fn dispatch_client_method_dispatches_terminal_wait_for_exit() {
        let nats = MockNatsClient::new();
        let client = MockClient::new();
        let session_id = AcpSessionId::new("sess-1").unwrap();

        let envelope = Request {
            id: RequestId::Number(1),
            method: std::sync::Arc::from("terminal/wait_for_exit"),
            params: Some(WaitForTerminalExitRequest::new("sess-1", "term-001")),
        };
        let payload = bytes::Bytes::from(serde_json::to_vec(&envelope).unwrap());

        let parsed = crate::nats::ParsedClientSubject {
            session_id,
            method: ClientMethod::TerminalWaitForExit,
        };

        let bridge = make_bridge(nats.clone());
        let ctx = DispatchContext {
            nats: &nats,
            client: &client,
            bridge: &bridge,
            serializer: &StdJsonSerialize,
        };
        dispatch_client_method(
            "acp.sess-1.client.terminal.wait_for_exit",
            parsed,
            payload,
            Some("_INBOX.reply".to_string()),
            &ctx,
        )
        .await;

        assert_eq!(nats.published_messages(), vec!["_INBOX.reply"]);
        let payloads = nats.published_payloads();
        assert_eq!(payloads.len(), 1);
        let response: serde_json::Value = serde_json::from_slice(payloads[0].as_ref()).unwrap();
        assert_eq!(response.get("id"), Some(&serde_json::Value::from(1)));
        assert!(response.get("result").is_some());
        assert!(response.get("error").is_none());
    }

    #[tokio::test]
    async fn dispatch_client_method_dispatches_terminal_wait_for_exit_client_error_publishes_error_reply()
     {
        let nats = MockNatsClient::new();
        let client = TerminalWaitForExitFailingClient;
        let session_id = AcpSessionId::new("sess-1").unwrap();

        let envelope = Request {
            id: RequestId::Number(1),
            method: std::sync::Arc::from("terminal/wait_for_exit"),
            params: Some(WaitForTerminalExitRequest::new("sess-1", "term-001")),
        };
        let payload = bytes::Bytes::from(serde_json::to_vec(&envelope).unwrap());

        let parsed = crate::nats::ParsedClientSubject {
            session_id,
            method: ClientMethod::TerminalWaitForExit,
        };

        let bridge = make_bridge(nats.clone());
        let ctx = DispatchContext {
            nats: &nats,
            client: &client,
            bridge: &bridge,
            serializer: &StdJsonSerialize,
        };
        dispatch_client_method(
            "acp.sess-1.client.terminal.wait_for_exit",
            parsed,
            payload,
            Some("_INBOX.reply".to_string()),
            &ctx,
        )
        .await;

        assert_eq!(nats.published_messages(), vec!["_INBOX.reply"]);
        let payloads = nats.published_payloads();
        assert_eq!(payloads.len(), 1);
        let response: serde_json::Value = serde_json::from_slice(payloads[0].as_ref()).unwrap();
        assert!(response.get("error").is_some());
        assert_eq!(
            response.get("error").and_then(|e| e.get("code")),
            Some(&serde_json::Value::from(-32603))
        );
        assert_eq!(
            response.get("error").and_then(|e| e.get("message")),
            Some(&serde_json::Value::from(
                "mock wait_for_terminal_exit failure"
            ))
        );
    }

    #[tokio::test]
    async fn dispatch_client_method_terminal_wait_for_exit_client_error_serialization_fallback() {
        let nats = MockNatsClient::new();
        let client = TerminalWaitForExitFailingClient;
        let serializer = FailNextSerialize::new(1);
        let session_id = AcpSessionId::new("sess-1").unwrap();

        let envelope = Request {
            id: RequestId::Number(42),
            method: std::sync::Arc::from("terminal/wait_for_exit"),
            params: Some(WaitForTerminalExitRequest::new("sess-1", "term-001")),
        };
        let payload = bytes::Bytes::from(serde_json::to_vec(&envelope).unwrap());

        let parsed = crate::nats::ParsedClientSubject {
            session_id,
            method: ClientMethod::TerminalWaitForExit,
        };

        let bridge = make_bridge(nats.clone());
        let ctx = DispatchContext {
            nats: &nats,
            client: &client,
            bridge: &bridge,
            serializer: &serializer,
        };
        dispatch_client_method(
            "acp.sess-1.client.terminal.wait_for_exit",
            parsed,
            payload,
            Some("_INBOX.reply".to_string()),
            &ctx,
        )
        .await;

        assert_eq!(nats.published_messages(), vec!["_INBOX.reply"]);
        let payloads = nats.published_payloads();
        assert_eq!(payloads.len(), 1);
        let response: serde_json::Value = serde_json::from_slice(payloads[0].as_ref()).unwrap();
        assert_eq!(response.get("id"), Some(&serde_json::Value::Null));
        assert_eq!(
            response.get("error").and_then(|e| e.get("code")),
            Some(&serde_json::Value::from(-32603))
        );
    }

    #[tokio::test]
    async fn dispatch_client_method_terminal_wait_for_exit_timeout_publishes_error_reply() {
        let nats = MockNatsClient::new();
        let client = TerminalWaitForExitTimeoutClient;
        let session_id = AcpSessionId::new("sess-1").unwrap();

        let envelope = Request {
            id: RequestId::Number(1),
            method: std::sync::Arc::from("terminal/wait_for_exit"),
            params: Some(WaitForTerminalExitRequest::new("sess-1", "term-001")),
        };
        let payload = bytes::Bytes::from(serde_json::to_vec(&envelope).unwrap());

        let parsed = crate::nats::ParsedClientSubject {
            session_id,
            method: ClientMethod::TerminalWaitForExit,
        };

        let bridge =
            make_bridge_with_operation_timeout(nats.clone(), std::time::Duration::from_millis(10));
        let ctx = DispatchContext {
            nats: &nats,
            client: &client,
            bridge: &bridge,
            serializer: &StdJsonSerialize,
        };
        dispatch_client_method(
            "acp.sess-1.client.terminal.wait_for_exit",
            parsed,
            payload,
            Some("_INBOX.reply".to_string()),
            &ctx,
        )
        .await;

        assert_eq!(nats.published_messages(), vec!["_INBOX.reply"]);
        let payloads = nats.published_payloads();
        assert_eq!(payloads.len(), 1);
        let response: serde_json::Value = serde_json::from_slice(payloads[0].as_ref()).unwrap();
        assert!(response.get("error").is_some());
        assert_eq!(
            response.get("error").and_then(|e| e.get("code")),
            Some(&serde_json::Value::from(-32603))
        );
    }

    #[tokio::test]
    async fn dispatch_client_method_terminal_wait_for_exit_success_publish_failure_exercises_error_path()
     {
        let nats = AdvancedMockNatsClient::new();
        nats.fail_next_publish();
        let client = MockClient::new();
        let session_id = AcpSessionId::new("sess-1").unwrap();

        let envelope = Request {
            id: RequestId::Number(1),
            method: std::sync::Arc::from("terminal/wait_for_exit"),
            params: Some(WaitForTerminalExitRequest::new("sess-1", "term-001")),
        };
        let payload = bytes::Bytes::from(serde_json::to_vec(&envelope).unwrap());

        let parsed = crate::nats::ParsedClientSubject {
            session_id,
            method: ClientMethod::TerminalWaitForExit,
        };

        let bridge = make_bridge_advanced(nats.clone());
        let ctx = DispatchContext {
            nats: &nats,
            client: &client,
            bridge: &bridge,
            serializer: &StdJsonSerialize,
        };
        dispatch_client_method(
            "acp.sess-1.client.terminal.wait_for_exit",
            parsed,
            payload,
            Some("_INBOX.reply".to_string()),
            &ctx,
        )
        .await;

        assert!(nats.published_messages().is_empty());
    }

    #[tokio::test]
    async fn dispatch_client_method_terminal_wait_for_exit_success_flush_failure_exercises_warn_path()
     {
        let nats = AdvancedMockNatsClient::new();
        nats.fail_next_flush();
        let client = MockClient::new();
        let session_id = AcpSessionId::new("sess-1").unwrap();

        let envelope = Request {
            id: RequestId::Number(1),
            method: std::sync::Arc::from("terminal/wait_for_exit"),
            params: Some(WaitForTerminalExitRequest::new("sess-1", "term-001")),
        };
        let payload = bytes::Bytes::from(serde_json::to_vec(&envelope).unwrap());

        let parsed = crate::nats::ParsedClientSubject {
            session_id,
            method: ClientMethod::TerminalWaitForExit,
        };

        let bridge = make_bridge_advanced(nats.clone());
        let ctx = DispatchContext {
            nats: &nats,
            client: &client,
            bridge: &bridge,
            serializer: &StdJsonSerialize,
        };
        dispatch_client_method(
            "acp.sess-1.client.terminal.wait_for_exit",
            parsed,
            payload,
            Some("_INBOX.reply".to_string()),
            &ctx,
        )
        .await;

        assert_eq!(nats.published_messages(), vec!["_INBOX.reply"]);
    }

    #[tokio::test]
    async fn dispatch_client_method_terminal_wait_for_exit_client_error_publish_failure_exercises_error_path()
     {
        let nats = AdvancedMockNatsClient::new();
        nats.fail_next_publish();
        let client = TerminalWaitForExitFailingClient;
        let session_id = AcpSessionId::new("sess-1").unwrap();

        let envelope = Request {
            id: RequestId::Number(1),
            method: std::sync::Arc::from("terminal/wait_for_exit"),
            params: Some(WaitForTerminalExitRequest::new("sess-1", "term-001")),
        };
        let payload = bytes::Bytes::from(serde_json::to_vec(&envelope).unwrap());

        let parsed = crate::nats::ParsedClientSubject {
            session_id,
            method: ClientMethod::TerminalWaitForExit,
        };

        let bridge = make_bridge_advanced(nats.clone());
        let ctx = DispatchContext {
            nats: &nats,
            client: &client,
            bridge: &bridge,
            serializer: &StdJsonSerialize,
        };
        dispatch_client_method(
            "acp.sess-1.client.terminal.wait_for_exit",
            parsed,
            payload,
            Some("_INBOX.reply".to_string()),
            &ctx,
        )
        .await;

        assert!(nats.published_messages().is_empty());
    }

    #[tokio::test]
    async fn dispatch_client_method_terminal_wait_for_exit_client_error_flush_failure_exercises_warn_path()
     {
        let nats = AdvancedMockNatsClient::new();
        nats.fail_next_flush();
        let client = TerminalWaitForExitFailingClient;
        let session_id = AcpSessionId::new("sess-1").unwrap();

        let envelope = Request {
            id: RequestId::Number(1),
            method: std::sync::Arc::from("terminal/wait_for_exit"),
            params: Some(WaitForTerminalExitRequest::new("sess-1", "term-001")),
        };
        let payload = bytes::Bytes::from(serde_json::to_vec(&envelope).unwrap());

        let parsed = crate::nats::ParsedClientSubject {
            session_id,
            method: ClientMethod::TerminalWaitForExit,
        };

        let bridge = make_bridge_advanced(nats.clone());
        let ctx = DispatchContext {
            nats: &nats,
            client: &client,
            bridge: &bridge,
            serializer: &StdJsonSerialize,
        };
        dispatch_client_method(
            "acp.sess-1.client.terminal.wait_for_exit",
            parsed,
            payload,
            Some("_INBOX.reply".to_string()),
            &ctx,
        )
        .await;

        assert_eq!(nats.published_messages(), vec!["_INBOX.reply"]);
    }

    #[tokio::test]
    async fn dispatch_client_method_dispatches_terminal_wait_for_exit_with_terminal_release_failing_client()
     {
        let nats = MockNatsClient::new();
        let client = TerminalReleaseFailingClient;
        let envelope = Request {
            id: RequestId::Number(1),
            method: std::sync::Arc::from("terminal/wait_for_exit"),
            params: Some(WaitForTerminalExitRequest::new("sess-1", "term-001")),
        };
        let payload = bytes::Bytes::from(serde_json::to_vec(&envelope).unwrap());
        let parsed = crate::nats::ParsedClientSubject {
            session_id: AcpSessionId::new("sess-1").unwrap(),
            method: ClientMethod::TerminalWaitForExit,
        };
        let bridge = make_bridge(nats.clone());
        let ctx = DispatchContext {
            nats: &nats,
            client: &client,
            bridge: &bridge,
            serializer: &StdJsonSerialize,
        };
        dispatch_client_method(
            "acp.sess-1.client.terminal.wait_for_exit",
            parsed,
            payload,
            Some("_INBOX.reply".to_string()),
            &ctx,
        )
        .await;
        assert_eq!(nats.published_messages(), vec!["_INBOX.reply"]);
    }

    #[tokio::test]
    async fn dispatch_client_method_terminal_wait_for_exit_failing_client_session_update_covers_stubs()
     {
        let nats = MockNatsClient::new();
        let client = TerminalWaitForExitFailingClient;
        let notification = SessionNotification::new(
            "sess-1",
            SessionUpdate::AgentMessageChunk(ContentChunk::new(ContentBlock::from("hi"))),
        );
        let payload = bytes::Bytes::from(serde_json::to_vec(&notification).unwrap());
        let parsed = crate::nats::ParsedClientSubject {
            session_id: AcpSessionId::new("sess-1").unwrap(),
            method: ClientMethod::SessionUpdate,
        };
        let bridge = make_bridge(nats.clone());
        let ctx = DispatchContext {
            nats: &nats,
            client: &client,
            bridge: &bridge,
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
    }

    #[tokio::test]
    async fn dispatch_client_method_terminal_wait_for_exit_timeout_client_session_update_covers_stubs()
     {
        let nats = MockNatsClient::new();
        let client = TerminalWaitForExitTimeoutClient;
        let notification = SessionNotification::new(
            "sess-1",
            SessionUpdate::AgentMessageChunk(ContentChunk::new(ContentBlock::from("hi"))),
        );
        let payload = bytes::Bytes::from(serde_json::to_vec(&notification).unwrap());
        let parsed = crate::nats::ParsedClientSubject {
            session_id: AcpSessionId::new("sess-1").unwrap(),
            method: ClientMethod::SessionUpdate,
        };
        let bridge = make_bridge(nats.clone());
        let ctx = DispatchContext {
            nats: &nats,
            client: &client,
            bridge: &bridge,
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
    }

    #[tokio::test]
    async fn dispatch_client_method_terminal_wait_for_exit_failing_client_request_permission_covers_stubs()
     {
        let nats = MockNatsClient::new();
        let client = TerminalWaitForExitFailingClient;
        let tool_call = ToolCallUpdate::new("call-1", ToolCallUpdateFields::new());
        let request = RequestPermissionRequest::new("sess-1", tool_call, vec![]);
        let envelope = Request {
            id: RequestId::Number(1),
            method: std::sync::Arc::from("session/request_permission"),
            params: Some(request),
        };
        let payload = bytes::Bytes::from(serde_json::to_vec(&envelope).unwrap());
        let parsed = crate::nats::ParsedClientSubject {
            session_id: AcpSessionId::new("sess-1").unwrap(),
            method: ClientMethod::SessionRequestPermission,
        };
        let bridge = make_bridge(nats.clone());
        let ctx = DispatchContext {
            nats: &nats,
            client: &client,
            bridge: &bridge,
            serializer: &StdJsonSerialize,
        };
        dispatch_client_method(
            "acp.sess-1.client.session.request_permission",
            parsed,
            payload,
            Some("_INBOX.reply".to_string()),
            &ctx,
        )
        .await;
        assert_eq!(nats.published_messages(), vec!["_INBOX.reply"]);
    }

    #[tokio::test]
    async fn dispatch_client_method_terminal_wait_for_exit_timeout_client_request_permission_covers_stubs()
     {
        let nats = MockNatsClient::new();
        let client = TerminalWaitForExitTimeoutClient;
        let tool_call = ToolCallUpdate::new("call-1", ToolCallUpdateFields::new());
        let request = RequestPermissionRequest::new("sess-1", tool_call, vec![]);
        let envelope = Request {
            id: RequestId::Number(1),
            method: std::sync::Arc::from("session/request_permission"),
            params: Some(request),
        };
        let payload = bytes::Bytes::from(serde_json::to_vec(&envelope).unwrap());
        let parsed = crate::nats::ParsedClientSubject {
            session_id: AcpSessionId::new("sess-1").unwrap(),
            method: ClientMethod::SessionRequestPermission,
        };
        let bridge = make_bridge(nats.clone());
        let ctx = DispatchContext {
            nats: &nats,
            client: &client,
            bridge: &bridge,
            serializer: &StdJsonSerialize,
        };
        dispatch_client_method(
            "acp.sess-1.client.session.request_permission",
            parsed,
            payload,
            Some("_INBOX.reply".to_string()),
            &ctx,
        )
        .await;
        assert_eq!(nats.published_messages(), vec!["_INBOX.reply"]);
    }

    #[tokio::test]
    async fn dispatch_client_method_terminal_wait_for_exit_failing_client_read_text_file_covers_stubs()
     {
        let nats = MockNatsClient::new();
        let client = TerminalWaitForExitFailingClient;
        let envelope = Request {
            id: RequestId::Number(1),
            method: std::sync::Arc::from("fs/read_text_file"),
            params: Some(ReadTextFileRequest::new("sess-1", "/tmp/foo")),
        };
        let payload = bytes::Bytes::from(serde_json::to_vec(&envelope).unwrap());
        let parsed = crate::nats::ParsedClientSubject {
            session_id: AcpSessionId::new("sess-1").unwrap(),
            method: ClientMethod::FsReadTextFile,
        };
        let bridge = make_bridge(nats.clone());
        let ctx = DispatchContext {
            nats: &nats,
            client: &client,
            bridge: &bridge,
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
    async fn dispatch_client_method_terminal_wait_for_exit_failing_client_terminal_create_covers_stubs()
     {
        let nats = MockNatsClient::new();
        let client = TerminalWaitForExitFailingClient;
        let envelope = Request {
            id: RequestId::Number(1),
            method: std::sync::Arc::from("terminal/create"),
            params: Some(CreateTerminalRequest::new("sess-1", "echo hi")),
        };
        let payload = bytes::Bytes::from(serde_json::to_vec(&envelope).unwrap());
        let parsed = crate::nats::ParsedClientSubject {
            session_id: AcpSessionId::new("sess-1").unwrap(),
            method: ClientMethod::TerminalCreate,
        };
        let bridge = make_bridge(nats.clone());
        let ctx = DispatchContext {
            nats: &nats,
            client: &client,
            bridge: &bridge,
            serializer: &StdJsonSerialize,
        };
        dispatch_client_method(
            "acp.sess-1.client.terminal.create",
            parsed,
            payload,
            Some("_INBOX.reply".to_string()),
            &ctx,
        )
        .await;
        assert_eq!(nats.published_messages(), vec!["_INBOX.reply"]);
    }

    #[tokio::test]
    async fn dispatch_client_method_terminal_wait_for_exit_failing_client_terminal_kill_covers_stubs()
     {
        let nats = MockNatsClient::new();
        let client = TerminalWaitForExitFailingClient;
        let envelope = Request {
            id: RequestId::Number(1),
            method: std::sync::Arc::from("terminal/kill_command"),
            params: Some(KillTerminalCommandRequest::new(
                agent_client_protocol::SessionId::from("sess-1"),
                "term-001".to_string(),
            )),
        };
        let payload = bytes::Bytes::from(serde_json::to_vec(&envelope).unwrap());
        let parsed = crate::nats::ParsedClientSubject {
            session_id: AcpSessionId::new("sess-1").unwrap(),
            method: ClientMethod::TerminalKill,
        };
        let bridge = make_bridge(nats.clone());
        let ctx = DispatchContext {
            nats: &nats,
            client: &client,
            bridge: &bridge,
            serializer: &StdJsonSerialize,
        };
        dispatch_client_method(
            "acp.sess-1.client.terminal.kill",
            parsed,
            payload,
            Some("_INBOX.reply".to_string()),
            &ctx,
        )
        .await;
        assert_eq!(nats.published_messages(), vec!["_INBOX.reply"]);
    }

    #[tokio::test]
    async fn dispatch_client_method_terminal_wait_for_exit_failing_client_terminal_output_covers_stubs()
     {
        let nats = MockNatsClient::new();
        let client = TerminalWaitForExitFailingClient;
        let envelope = Request {
            id: RequestId::Number(1),
            method: std::sync::Arc::from("terminal/output"),
            params: Some(TerminalOutputRequest::new("sess-1", "term-001")),
        };
        let payload = bytes::Bytes::from(serde_json::to_vec(&envelope).unwrap());
        let parsed = crate::nats::ParsedClientSubject {
            session_id: AcpSessionId::new("sess-1").unwrap(),
            method: ClientMethod::TerminalOutput,
        };
        let bridge = make_bridge(nats.clone());
        let ctx = DispatchContext {
            nats: &nats,
            client: &client,
            bridge: &bridge,
            serializer: &StdJsonSerialize,
        };
        dispatch_client_method(
            "acp.sess-1.client.terminal.output",
            parsed,
            payload,
            Some("_INBOX.reply".to_string()),
            &ctx,
        )
        .await;
        assert_eq!(nats.published_messages(), vec!["_INBOX.reply"]);
    }

    #[tokio::test]
    async fn dispatch_client_method_terminal_wait_for_exit_failing_client_terminal_release_covers_stubs()
     {
        let nats = MockNatsClient::new();
        let client = TerminalWaitForExitFailingClient;
        let envelope = Request {
            id: RequestId::Number(1),
            method: std::sync::Arc::from("terminal/release"),
            params: Some(ReleaseTerminalRequest::new("sess-1", "term-001")),
        };
        let payload = bytes::Bytes::from(serde_json::to_vec(&envelope).unwrap());
        let parsed = crate::nats::ParsedClientSubject {
            session_id: AcpSessionId::new("sess-1").unwrap(),
            method: ClientMethod::TerminalRelease,
        };
        let bridge = make_bridge(nats.clone());
        let ctx = DispatchContext {
            nats: &nats,
            client: &client,
            bridge: &bridge,
            serializer: &StdJsonSerialize,
        };
        dispatch_client_method(
            "acp.sess-1.client.terminal.release",
            parsed,
            payload,
            Some("_INBOX.reply".to_string()),
            &ctx,
        )
        .await;
        assert_eq!(nats.published_messages(), vec!["_INBOX.reply"]);
    }

    #[tokio::test]
    async fn dispatch_client_method_terminal_wait_for_exit_failing_client_write_text_file_covers_stubs()
     {
        let nats = MockNatsClient::new();
        let client = TerminalWaitForExitFailingClient;
        let envelope = Request {
            id: RequestId::Number(1),
            method: std::sync::Arc::from("fs/write_text_file"),
            params: Some(WriteTextFileRequest::new(
                agent_client_protocol::SessionId::from("sess-1"),
                "/tmp/foo.txt".to_string(),
                "content".to_string(),
            )),
        };
        let payload = bytes::Bytes::from(serde_json::to_vec(&envelope).unwrap());
        let parsed = crate::nats::ParsedClientSubject {
            session_id: AcpSessionId::new("sess-1").unwrap(),
            method: ClientMethod::FsWriteTextFile,
        };
        let bridge = make_bridge(nats.clone());
        let ctx = DispatchContext {
            nats: &nats,
            client: &client,
            bridge: &bridge,
            serializer: &StdJsonSerialize,
        };
        dispatch_client_method(
            "acp.sess-1.client.fs.write_text_file",
            parsed,
            payload,
            Some("_INBOX.reply".to_string()),
            &ctx,
        )
        .await;
        assert_eq!(nats.published_messages(), vec!["_INBOX.reply"]);
    }

    #[tokio::test]
    async fn dispatch_client_method_terminal_wait_for_exit_timeout_client_read_text_file_covers_stubs()
     {
        let nats = MockNatsClient::new();
        let client = TerminalWaitForExitTimeoutClient;
        let envelope = Request {
            id: RequestId::Number(1),
            method: std::sync::Arc::from("fs/read_text_file"),
            params: Some(ReadTextFileRequest::new("sess-1", "/tmp/foo")),
        };
        let payload = bytes::Bytes::from(serde_json::to_vec(&envelope).unwrap());
        let parsed = crate::nats::ParsedClientSubject {
            session_id: AcpSessionId::new("sess-1").unwrap(),
            method: ClientMethod::FsReadTextFile,
        };
        let bridge = make_bridge(nats.clone());
        let ctx = DispatchContext {
            nats: &nats,
            client: &client,
            bridge: &bridge,
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
    async fn dispatch_client_method_terminal_wait_for_exit_timeout_client_terminal_create_covers_stubs()
     {
        let nats = MockNatsClient::new();
        let client = TerminalWaitForExitTimeoutClient;
        let envelope = Request {
            id: RequestId::Number(1),
            method: std::sync::Arc::from("terminal/create"),
            params: Some(CreateTerminalRequest::new("sess-1", "echo hi")),
        };
        let payload = bytes::Bytes::from(serde_json::to_vec(&envelope).unwrap());
        let parsed = crate::nats::ParsedClientSubject {
            session_id: AcpSessionId::new("sess-1").unwrap(),
            method: ClientMethod::TerminalCreate,
        };
        let bridge = make_bridge(nats.clone());
        let ctx = DispatchContext {
            nats: &nats,
            client: &client,
            bridge: &bridge,
            serializer: &StdJsonSerialize,
        };
        dispatch_client_method(
            "acp.sess-1.client.terminal.create",
            parsed,
            payload,
            Some("_INBOX.reply".to_string()),
            &ctx,
        )
        .await;
        assert_eq!(nats.published_messages(), vec!["_INBOX.reply"]);
    }

    #[tokio::test]
    async fn dispatch_client_method_terminal_wait_for_exit_timeout_client_terminal_kill_covers_stubs()
     {
        let nats = MockNatsClient::new();
        let client = TerminalWaitForExitTimeoutClient;
        let envelope = Request {
            id: RequestId::Number(1),
            method: std::sync::Arc::from("terminal/kill_command"),
            params: Some(KillTerminalCommandRequest::new(
                agent_client_protocol::SessionId::from("sess-1"),
                "term-001".to_string(),
            )),
        };
        let payload = bytes::Bytes::from(serde_json::to_vec(&envelope).unwrap());
        let parsed = crate::nats::ParsedClientSubject {
            session_id: AcpSessionId::new("sess-1").unwrap(),
            method: ClientMethod::TerminalKill,
        };
        let bridge = make_bridge(nats.clone());
        let ctx = DispatchContext {
            nats: &nats,
            client: &client,
            bridge: &bridge,
            serializer: &StdJsonSerialize,
        };
        dispatch_client_method(
            "acp.sess-1.client.terminal.kill",
            parsed,
            payload,
            Some("_INBOX.reply".to_string()),
            &ctx,
        )
        .await;
        assert_eq!(nats.published_messages(), vec!["_INBOX.reply"]);
    }

    #[tokio::test]
    async fn dispatch_client_method_terminal_wait_for_exit_timeout_client_terminal_output_covers_stubs()
     {
        let nats = MockNatsClient::new();
        let client = TerminalWaitForExitTimeoutClient;
        let envelope = Request {
            id: RequestId::Number(1),
            method: std::sync::Arc::from("terminal/output"),
            params: Some(TerminalOutputRequest::new("sess-1", "term-001")),
        };
        let payload = bytes::Bytes::from(serde_json::to_vec(&envelope).unwrap());
        let parsed = crate::nats::ParsedClientSubject {
            session_id: AcpSessionId::new("sess-1").unwrap(),
            method: ClientMethod::TerminalOutput,
        };
        let bridge = make_bridge(nats.clone());
        let ctx = DispatchContext {
            nats: &nats,
            client: &client,
            bridge: &bridge,
            serializer: &StdJsonSerialize,
        };
        dispatch_client_method(
            "acp.sess-1.client.terminal.output",
            parsed,
            payload,
            Some("_INBOX.reply".to_string()),
            &ctx,
        )
        .await;
        assert_eq!(nats.published_messages(), vec!["_INBOX.reply"]);
    }

    #[tokio::test]
    async fn dispatch_client_method_terminal_wait_for_exit_timeout_client_terminal_release_covers_stubs()
     {
        let nats = MockNatsClient::new();
        let client = TerminalWaitForExitTimeoutClient;
        let envelope = Request {
            id: RequestId::Number(1),
            method: std::sync::Arc::from("terminal/release"),
            params: Some(ReleaseTerminalRequest::new("sess-1", "term-001")),
        };
        let payload = bytes::Bytes::from(serde_json::to_vec(&envelope).unwrap());
        let parsed = crate::nats::ParsedClientSubject {
            session_id: AcpSessionId::new("sess-1").unwrap(),
            method: ClientMethod::TerminalRelease,
        };
        let bridge = make_bridge(nats.clone());
        let ctx = DispatchContext {
            nats: &nats,
            client: &client,
            bridge: &bridge,
            serializer: &StdJsonSerialize,
        };
        dispatch_client_method(
            "acp.sess-1.client.terminal.release",
            parsed,
            payload,
            Some("_INBOX.reply".to_string()),
            &ctx,
        )
        .await;
        assert_eq!(nats.published_messages(), vec!["_INBOX.reply"]);
    }

    #[tokio::test]
    async fn dispatch_client_method_terminal_wait_for_exit_timeout_client_write_text_file_covers_stubs()
     {
        let nats = MockNatsClient::new();
        let client = TerminalWaitForExitTimeoutClient;
        let envelope = Request {
            id: RequestId::Number(1),
            method: std::sync::Arc::from("fs/write_text_file"),
            params: Some(WriteTextFileRequest::new(
                agent_client_protocol::SessionId::from("sess-1"),
                "/tmp/foo.txt".to_string(),
                "content".to_string(),
            )),
        };
        let payload = bytes::Bytes::from(serde_json::to_vec(&envelope).unwrap());
        let parsed = crate::nats::ParsedClientSubject {
            session_id: AcpSessionId::new("sess-1").unwrap(),
            method: ClientMethod::FsWriteTextFile,
        };
        let bridge = make_bridge(nats.clone());
        let ctx = DispatchContext {
            nats: &nats,
            client: &client,
            bridge: &bridge,
            serializer: &StdJsonSerialize,
        };
        dispatch_client_method(
            "acp.sess-1.client.fs.write_text_file",
            parsed,
            payload,
            Some("_INBOX.reply".to_string()),
            &ctx,
        )
        .await;
        assert_eq!(nats.published_messages(), vec!["_INBOX.reply"]);
    }

    #[tokio::test]
    async fn dispatch_client_method_terminal_kill_failing_client_write_text_file_covers_stubs() {
        let nats = MockNatsClient::new();
        let client = TerminalKillFailingClient;
        let envelope = Request {
            id: RequestId::Number(1),
            method: std::sync::Arc::from("fs/write_text_file"),
            params: Some(WriteTextFileRequest::new(
                agent_client_protocol::SessionId::from("sess-1"),
                "/tmp/foo.txt".to_string(),
                "content".to_string(),
            )),
        };
        let payload = bytes::Bytes::from(serde_json::to_vec(&envelope).unwrap());
        let parsed = crate::nats::ParsedClientSubject {
            session_id: AcpSessionId::new("sess-1").unwrap(),
            method: ClientMethod::FsWriteTextFile,
        };
        let bridge = make_bridge(nats.clone());
        let ctx = DispatchContext {
            nats: &nats,
            client: &client,
            bridge: &bridge,
            serializer: &StdJsonSerialize,
        };
        dispatch_client_method(
            "acp.sess-1.client.fs.write_text_file",
            parsed,
            payload,
            Some("_INBOX.reply".to_string()),
            &ctx,
        )
        .await;
        assert_eq!(nats.published_messages(), vec!["_INBOX.reply"]);
    }

    #[tokio::test]
    async fn dispatch_client_method_terminal_release_failing_client_write_text_file_covers_stubs() {
        let nats = MockNatsClient::new();
        let client = TerminalReleaseFailingClient;
        let envelope = Request {
            id: RequestId::Number(1),
            method: std::sync::Arc::from("fs/write_text_file"),
            params: Some(WriteTextFileRequest::new(
                agent_client_protocol::SessionId::from("sess-1"),
                "/tmp/foo.txt".to_string(),
                "content".to_string(),
            )),
        };
        let payload = bytes::Bytes::from(serde_json::to_vec(&envelope).unwrap());
        let parsed = crate::nats::ParsedClientSubject {
            session_id: AcpSessionId::new("sess-1").unwrap(),
            method: ClientMethod::FsWriteTextFile,
        };
        let bridge = make_bridge(nats.clone());
        let ctx = DispatchContext {
            nats: &nats,
            client: &client,
            bridge: &bridge,
            serializer: &StdJsonSerialize,
        };
        dispatch_client_method(
            "acp.sess-1.client.fs.write_text_file",
            parsed,
            payload,
            Some("_INBOX.reply".to_string()),
            &ctx,
        )
        .await;
        assert_eq!(nats.published_messages(), vec!["_INBOX.reply"]);
    }

    #[tokio::test]
    async fn dispatch_client_method_dispatches_terminal_release_session_id_mismatch_publishes_error_reply()
     {
        let nats = MockNatsClient::new();
        let client = MockClient::new();
        let session_id = AcpSessionId::new("sess-a").unwrap();

        let envelope = Request {
            id: RequestId::Number(1),
            method: std::sync::Arc::from("terminal/release"),
            params: Some(ReleaseTerminalRequest::new("sess-b", "term-001")),
        };
        let payload = bytes::Bytes::from(serde_json::to_vec(&envelope).unwrap());

        let parsed = crate::nats::ParsedClientSubject {
            session_id,
            method: ClientMethod::TerminalRelease,
        };

        let bridge = make_bridge(nats.clone());
        let ctx = DispatchContext {
            nats: &nats,
            client: &client,
            bridge: &bridge,
            serializer: &StdJsonSerialize,
        };
        dispatch_client_method(
            "acp.sess-a.client.terminal.release",
            parsed,
            payload,
            Some("_INBOX.err".to_string()),
            &ctx,
        )
        .await;

        assert_eq!(nats.published_messages(), vec!["_INBOX.err"]);
        let payloads = nats.published_payloads();
        let response: serde_json::Value = serde_json::from_slice(payloads[0].as_ref()).unwrap();
        assert!(response.get("error").is_some());
        assert_eq!(client.terminal_release_call_count(), 0);
    }

    #[tokio::test]
    async fn dispatch_client_method_dispatches_terminal_release_client_error_publishes_error_reply()
    {
        let nats = MockNatsClient::new();
        let client = TerminalReleaseFailingClient;
        let session_id = AcpSessionId::new("sess-1").unwrap();

        let envelope = Request {
            id: RequestId::Number(1),
            method: std::sync::Arc::from("terminal/release"),
            params: Some(ReleaseTerminalRequest::new("sess-1", "term-001")),
        };
        let payload = bytes::Bytes::from(serde_json::to_vec(&envelope).unwrap());

        let parsed = crate::nats::ParsedClientSubject {
            session_id,
            method: ClientMethod::TerminalRelease,
        };

        let bridge = make_bridge(nats.clone());
        let ctx = DispatchContext {
            nats: &nats,
            client: &client,
            bridge: &bridge,
            serializer: &StdJsonSerialize,
        };
        dispatch_client_method(
            "acp.sess-1.client.terminal.release",
            parsed,
            payload,
            Some("_INBOX.err".to_string()),
            &ctx,
        )
        .await;

        assert_eq!(nats.published_messages(), vec!["_INBOX.err"]);
        let payloads = nats.published_payloads();
        let response: serde_json::Value = serde_json::from_slice(payloads[0].as_ref()).unwrap();
        assert!(response.get("error").is_some());
        assert_eq!(
            response.get("error").and_then(|e| e.get("code")),
            Some(&serde_json::Value::from(-32603))
        );
    }

    #[tokio::test]
    async fn dispatch_client_method_terminal_release_no_reply_does_not_call_client_or_publish() {
        let nats = MockNatsClient::new();
        let client = MockClient::new();
        let session_id = AcpSessionId::new("sess-1").unwrap();

        let envelope = Request {
            id: RequestId::Number(1),
            method: std::sync::Arc::from("terminal/release"),
            params: Some(ReleaseTerminalRequest::new("sess-1", "term-001")),
        };
        let payload = bytes::Bytes::from(serde_json::to_vec(&envelope).unwrap());

        let parsed = crate::nats::ParsedClientSubject {
            session_id,
            method: ClientMethod::TerminalRelease,
        };

        let bridge = make_bridge(nats.clone());
        let ctx = DispatchContext {
            nats: &nats,
            client: &client,
            bridge: &bridge,
            serializer: &StdJsonSerialize,
        };
        dispatch_client_method(
            "acp.sess-1.client.terminal.release",
            parsed,
            payload,
            None,
            &ctx,
        )
        .await;

        assert!(nats.published_messages().is_empty());
        assert_eq!(client.terminal_release_call_count(), 0);
    }

    #[tokio::test]
    async fn dispatch_client_method_dispatches_session_update_with_terminal_release_failing_client()
    {
        let nats = MockNatsClient::new();
        let client = TerminalReleaseFailingClient;
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

        let bridge = make_bridge(nats.clone());
        let ctx = DispatchContext {
            nats: &nats,
            client: &client,
            bridge: &bridge,
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
    }

    #[tokio::test]
    async fn dispatch_client_method_dispatches_fs_read_text_file_with_terminal_release_failing_client()
     {
        let nats = MockNatsClient::new();
        let client = TerminalReleaseFailingClient;
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

        let bridge = make_bridge(nats.clone());
        let ctx = DispatchContext {
            nats: &nats,
            client: &client,
            bridge: &bridge,
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
    async fn dispatch_client_method_dispatches_terminal_create_with_terminal_release_failing_client()
     {
        let nats = MockNatsClient::new();
        let client = TerminalReleaseFailingClient;
        let session_id = AcpSessionId::new("sess-1").unwrap();

        let envelope = Request {
            id: RequestId::Number(1),
            method: std::sync::Arc::from("terminal/create"),
            params: Some(CreateTerminalRequest::new("sess-1", "echo hi")),
        };
        let payload = bytes::Bytes::from(serde_json::to_vec(&envelope).unwrap());

        let parsed = crate::nats::ParsedClientSubject {
            session_id,
            method: ClientMethod::TerminalCreate,
        };

        let bridge = make_bridge(nats.clone());
        let ctx = DispatchContext {
            nats: &nats,
            client: &client,
            bridge: &bridge,
            serializer: &StdJsonSerialize,
        };
        dispatch_client_method(
            "acp.sess-1.client.terminal.create",
            parsed,
            payload,
            Some("_INBOX.reply".to_string()),
            &ctx,
        )
        .await;

        assert_eq!(nats.published_messages(), vec!["_INBOX.reply"]);
    }

    #[tokio::test]
    async fn dispatch_client_method_dispatches_terminal_kill_with_terminal_release_failing_client()
    {
        let nats = MockNatsClient::new();
        let client = TerminalReleaseFailingClient;
        let session_id = AcpSessionId::new("sess-1").unwrap();

        let envelope = Request {
            id: RequestId::Number(1),
            method: std::sync::Arc::from("terminal/kill"),
            params: Some(KillTerminalCommandRequest::new("sess-1", "term-001")),
        };
        let payload = bytes::Bytes::from(serde_json::to_vec(&envelope).unwrap());

        let parsed = crate::nats::ParsedClientSubject {
            session_id,
            method: ClientMethod::TerminalKill,
        };

        let bridge = make_bridge(nats.clone());
        let ctx = DispatchContext {
            nats: &nats,
            client: &client,
            bridge: &bridge,
            serializer: &StdJsonSerialize,
        };
        dispatch_client_method(
            "acp.sess-1.client.terminal.kill",
            parsed,
            payload,
            Some("_INBOX.reply".to_string()),
            &ctx,
        )
        .await;

        assert_eq!(nats.published_messages(), vec!["_INBOX.reply"]);
    }

    #[tokio::test]
    async fn dispatch_client_method_dispatches_terminal_output_with_terminal_release_failing_client()
     {
        let nats = MockNatsClient::new();
        let client = TerminalReleaseFailingClient;
        let session_id = AcpSessionId::new("sess-1").unwrap();

        let envelope = Request {
            id: RequestId::Number(1),
            method: std::sync::Arc::from("terminal/output"),
            params: Some(TerminalOutputRequest::new("sess-1", "term-001")),
        };
        let payload = bytes::Bytes::from(serde_json::to_vec(&envelope).unwrap());

        let parsed = crate::nats::ParsedClientSubject {
            session_id,
            method: ClientMethod::TerminalOutput,
        };

        let bridge = make_bridge(nats.clone());
        let ctx = DispatchContext {
            nats: &nats,
            client: &client,
            bridge: &bridge,
            serializer: &StdJsonSerialize,
        };
        dispatch_client_method(
            "acp.sess-1.client.terminal.output",
            parsed,
            payload,
            Some("_INBOX.reply".to_string()),
            &ctx,
        )
        .await;

        assert_eq!(nats.published_messages(), vec!["_INBOX.reply"]);
    }

    #[tokio::test]
    async fn dispatch_client_method_dispatches_terminal_release_with_terminal_kill_failing_client()
    {
        let nats = MockNatsClient::new();
        let client = TerminalKillFailingClient;
        let session_id = AcpSessionId::new("sess-1").unwrap();

        let envelope = Request {
            id: RequestId::Number(1),
            method: std::sync::Arc::from("terminal/release"),
            params: Some(ReleaseTerminalRequest::new("sess-1", "term-001")),
        };
        let payload = bytes::Bytes::from(serde_json::to_vec(&envelope).unwrap());

        let parsed = crate::nats::ParsedClientSubject {
            session_id,
            method: ClientMethod::TerminalRelease,
        };

        let bridge = make_bridge(nats.clone());
        let ctx = DispatchContext {
            nats: &nats,
            client: &client,
            bridge: &bridge,
            serializer: &StdJsonSerialize,
        };
        dispatch_client_method(
            "acp.sess-1.client.terminal.release",
            parsed,
            payload,
            Some("_INBOX.err".to_string()),
            &ctx,
        )
        .await;

        assert_eq!(nats.published_messages(), vec!["_INBOX.err"]);
        let payloads = nats.published_payloads();
        let response: serde_json::Value = serde_json::from_slice(payloads[0].as_ref()).unwrap();
        assert!(response.get("error").is_some());
    }

    #[tokio::test]
    async fn dispatch_client_method_dispatches_request_permission_with_terminal_release_failing_client()
     {
        let nats = MockNatsClient::new();
        let client = TerminalReleaseFailingClient;
        let session_id = AcpSessionId::new("sess-1").unwrap();

        let tool_call = ToolCallUpdate::new("call-1", ToolCallUpdateFields::new());
        let request = RequestPermissionRequest::new("sess-1", tool_call, vec![]);
        let envelope = Request {
            id: RequestId::Number(1),
            method: std::sync::Arc::from("session/request_permission"),
            params: Some(request),
        };
        let payload = bytes::Bytes::from(serde_json::to_vec(&envelope).unwrap());

        let parsed = crate::nats::ParsedClientSubject {
            session_id,
            method: ClientMethod::SessionRequestPermission,
        };

        let bridge = make_bridge(nats.clone());
        let ctx = DispatchContext {
            nats: &nats,
            client: &client,
            bridge: &bridge,
            serializer: &StdJsonSerialize,
        };
        dispatch_client_method(
            "acp.sess-1.client.session.request_permission",
            parsed,
            payload,
            Some("_INBOX.reply".to_string()),
            &ctx,
        )
        .await;

        assert_eq!(nats.published_messages(), vec!["_INBOX.reply"]);
    }

    #[tokio::test]
    async fn dispatch_client_method_dispatches_terminal_output_client_error_publishes_error_reply()
    {
        let nats = MockNatsClient::new();
        let client = TerminalKillFailingClient;
        let session_id = AcpSessionId::new("sess-1").unwrap();

        let envelope = Request {
            id: RequestId::Number(1),
            method: std::sync::Arc::from("terminal/output"),
            params: Some(TerminalOutputRequest::new("sess-1", "term-001")),
        };
        let payload = bytes::Bytes::from(serde_json::to_vec(&envelope).unwrap());

        let parsed = crate::nats::ParsedClientSubject {
            session_id,
            method: ClientMethod::TerminalOutput,
        };

        let bridge = make_bridge(nats.clone());
        let ctx = DispatchContext {
            nats: &nats,
            client: &client,
            bridge: &bridge,
            serializer: &StdJsonSerialize,
        };
        dispatch_client_method(
            "acp.sess-1.client.terminal.output",
            parsed,
            payload,
            Some("_INBOX.reply".to_string()),
            &ctx,
        )
        .await;

        assert_eq!(nats.published_messages(), vec!["_INBOX.reply"]);
        let payloads = nats.published_payloads();
        let response: serde_json::Value = serde_json::from_slice(payloads[0].as_ref()).unwrap();
        assert!(response.get("error").is_some());
        assert_eq!(
            response.get("error").and_then(|e| e.get("code")),
            Some(&serde_json::Value::from(-32603))
        );
    }

    #[tokio::test]
    async fn dispatch_client_method_dispatches_terminal_output_with_rpc_mock_client() {
        let nats = MockNatsClient::new();
        let client = RpcMockClient;
        let session_id = AcpSessionId::new("sess-1").unwrap();

        let envelope = Request {
            id: RequestId::Number(1),
            method: std::sync::Arc::from("terminal/output"),
            params: Some(TerminalOutputRequest::new("sess-1", "term-001")),
        };
        let payload = bytes::Bytes::from(serde_json::to_vec(&envelope).unwrap());

        let parsed = crate::nats::ParsedClientSubject {
            session_id,
            method: ClientMethod::TerminalOutput,
        };

        let bridge = make_bridge(nats.clone());
        let ctx = DispatchContext {
            nats: &nats,
            client: &client,
            bridge: &bridge,
            serializer: &StdJsonSerialize,
        };
        dispatch_client_method(
            "acp.sess-1.client.terminal.output",
            parsed,
            payload,
            Some("_INBOX.reply".to_string()),
            &ctx,
        )
        .await;

        assert_eq!(nats.published_messages(), vec!["_INBOX.reply"]);
    }

    #[tokio::test]
    async fn dispatch_client_method_dispatches_terminal_output_serialization_fallback() {
        let nats = MockNatsClient::new();
        let client = MockClient::new();
        let serializer = FailNextSerialize::new(1);
        let session_id = AcpSessionId::new("sess-1").unwrap();

        let envelope = Request {
            id: RequestId::Number(1),
            method: std::sync::Arc::from("terminal/output"),
            params: Some(TerminalOutputRequest::new("sess-1", "term-001")),
        };
        let payload = bytes::Bytes::from(serde_json::to_vec(&envelope).unwrap());

        let parsed = crate::nats::ParsedClientSubject {
            session_id,
            method: ClientMethod::TerminalOutput,
        };

        let bridge = make_bridge(nats.clone());
        let ctx = DispatchContext {
            nats: &nats,
            client: &client,
            bridge: &bridge,
            serializer: &serializer,
        };
        dispatch_client_method(
            "acp.sess-1.client.terminal.output",
            parsed,
            payload,
            Some("_INBOX.reply".to_string()),
            &ctx,
        )
        .await;

        assert_eq!(nats.published_messages(), vec!["_INBOX.reply"]);
    }

    #[tokio::test]
    async fn dispatch_client_method_terminal_kill_no_reply_does_not_call_client_or_publish() {
        let nats = MockNatsClient::new();
        let client = MockClient::new();
        let session_id = AcpSessionId::new("sess-1").unwrap();

        let envelope = Request {
            id: RequestId::Number(1),
            method: std::sync::Arc::from("terminal/kill_command"),
            params: Some(KillTerminalCommandRequest::new(
                agent_client_protocol::SessionId::from("sess-1"),
                "term-001".to_string(),
            )),
        };
        let payload = bytes::Bytes::from(serde_json::to_vec(&envelope).unwrap());

        let parsed = crate::nats::ParsedClientSubject {
            session_id,
            method: ClientMethod::TerminalKill,
        };

        let bridge = make_bridge(nats.clone());
        let ctx = DispatchContext {
            nats: &nats,
            client: &client,
            bridge: &bridge,
            serializer: &StdJsonSerialize,
        };
        dispatch_client_method(
            "acp.sess-1.client.terminal.kill",
            parsed,
            payload,
            None,
            &ctx,
        )
        .await;

        assert!(nats.published_messages().is_empty());
        assert_eq!(client.kill_terminal_call_count(), 0);
    }

    #[tokio::test]
    async fn dispatch_client_method_dispatches_terminal_kill_session_id_mismatch_publishes_error_reply()
     {
        let nats = MockNatsClient::new();
        let client = MockClient::new();
        let session_id = AcpSessionId::new("sess-a").unwrap();

        let envelope = Request {
            id: RequestId::Number(1),
            method: std::sync::Arc::from("terminal/kill_command"),
            params: Some(KillTerminalCommandRequest::new(
                agent_client_protocol::SessionId::from("sess-b"),
                "term-001".to_string(),
            )),
        };
        let payload = bytes::Bytes::from(serde_json::to_vec(&envelope).unwrap());

        let parsed = crate::nats::ParsedClientSubject {
            session_id,
            method: ClientMethod::TerminalKill,
        };

        let bridge = make_bridge(nats.clone());
        let ctx = DispatchContext {
            nats: &nats,
            client: &client,
            bridge: &bridge,
            serializer: &StdJsonSerialize,
        };
        dispatch_client_method(
            "acp.sess-a.client.terminal.kill",
            parsed,
            payload,
            Some("_INBOX.reply".to_string()),
            &ctx,
        )
        .await;

        assert_eq!(nats.published_messages(), vec!["_INBOX.reply"]);
        let payloads = nats.published_payloads();
        assert_eq!(payloads.len(), 1);
        let response: serde_json::Value = serde_json::from_slice(payloads[0].as_ref()).unwrap();
        assert!(response.get("error").is_some());
        assert!(response.get("result").is_none());
        assert_eq!(
            response.get("error").and_then(|e| e.get("code")),
            Some(&serde_json::Value::from(-32602))
        );
        assert!(
            response
                .get("error")
                .and_then(|e| e.get("message"))
                .and_then(|v| v.as_str())
                .unwrap_or_default()
                .contains("params.sessionId")
        );
    }

    #[tokio::test]
    async fn dispatch_client_method_dispatches_terminal_kill_invalid_json_publishes_parse_error() {
        let nats = MockNatsClient::new();
        let client = MockClient::new();
        let session_id = AcpSessionId::new("sess-1").unwrap();
        let payload = bytes::Bytes::from_static(b"not json");

        let parsed = crate::nats::ParsedClientSubject {
            session_id,
            method: ClientMethod::TerminalKill,
        };

        let bridge = make_bridge(nats.clone());
        let ctx = DispatchContext {
            nats: &nats,
            client: &client,
            bridge: &bridge,
            serializer: &StdJsonSerialize,
        };
        dispatch_client_method(
            "acp.sess-1.client.terminal.kill",
            parsed,
            payload,
            Some("_INBOX.reply".to_string()),
            &ctx,
        )
        .await;

        assert_eq!(nats.published_messages(), vec!["_INBOX.reply"]);
        let payloads = nats.published_payloads();
        assert_eq!(payloads.len(), 1);
        let response: serde_json::Value = serde_json::from_slice(payloads[0].as_ref()).unwrap();
        assert_eq!(
            response.get("error").and_then(|e| e.get("code")),
            Some(&serde_json::Value::from(-32700))
        );
    }

    pub(super) struct TerminalKillFailingClient;

    #[async_trait(?Send)]
    impl Client for TerminalKillFailingClient {
        async fn session_notification(
            &self,
            n: agent_client_protocol::SessionNotification,
        ) -> agent_client_protocol::Result<()> {
            let _ = n;
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

        async fn write_text_file(
            &self,
            _: WriteTextFileRequest,
        ) -> agent_client_protocol::Result<WriteTextFileResponse> {
            Ok(WriteTextFileResponse::new())
        }

        async fn create_terminal(
            &self,
            _: CreateTerminalRequest,
        ) -> agent_client_protocol::Result<CreateTerminalResponse> {
            Ok(CreateTerminalResponse::new("term-001"))
        }

        async fn kill_terminal_command(
            &self,
            _: KillTerminalCommandRequest,
        ) -> agent_client_protocol::Result<KillTerminalCommandResponse> {
            Err(agent_client_protocol::Error::new(
                -32603,
                "mock kill_terminal_command failure",
            ))
        }

        async fn terminal_output(
            &self,
            _: TerminalOutputRequest,
        ) -> agent_client_protocol::Result<TerminalOutputResponse> {
            Err(agent_client_protocol::Error::new(
                -32603,
                "mock terminal_output failure",
            ))
        }

        async fn release_terminal(
            &self,
            _: ReleaseTerminalRequest,
        ) -> agent_client_protocol::Result<ReleaseTerminalResponse> {
            Err(agent_client_protocol::Error::new(
                -32603,
                "mock release_terminal failure",
            ))
        }
    }

    pub(super) struct TerminalReleaseFailingClient;

    #[async_trait(?Send)]
    impl Client for TerminalReleaseFailingClient {
        async fn session_notification(
            &self,
            n: agent_client_protocol::SessionNotification,
        ) -> agent_client_protocol::Result<()> {
            let _ = n;
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

        async fn write_text_file(
            &self,
            _: WriteTextFileRequest,
        ) -> agent_client_protocol::Result<WriteTextFileResponse> {
            Ok(WriteTextFileResponse::new())
        }

        async fn create_terminal(
            &self,
            _: CreateTerminalRequest,
        ) -> agent_client_protocol::Result<CreateTerminalResponse> {
            Ok(CreateTerminalResponse::new("term-001"))
        }

        async fn kill_terminal_command(
            &self,
            _: KillTerminalCommandRequest,
        ) -> agent_client_protocol::Result<KillTerminalCommandResponse> {
            Ok(KillTerminalCommandResponse::new())
        }

        async fn terminal_output(
            &self,
            _: TerminalOutputRequest,
        ) -> agent_client_protocol::Result<TerminalOutputResponse> {
            Ok(TerminalOutputResponse::new(
                "mock output".to_string(),
                false,
            ))
        }

        async fn release_terminal(
            &self,
            _: ReleaseTerminalRequest,
        ) -> agent_client_protocol::Result<ReleaseTerminalResponse> {
            Err(agent_client_protocol::Error::new(
                -32603,
                "mock release_terminal failure",
            ))
        }

        async fn wait_for_terminal_exit(
            &self,
            _: WaitForTerminalExitRequest,
        ) -> agent_client_protocol::Result<WaitForTerminalExitResponse> {
            Ok(WaitForTerminalExitResponse::new(
                TerminalExitStatus::new().exit_code(0u32),
            ))
        }
    }

    pub(super) struct TerminalWaitForExitFailingClient;

    #[async_trait(?Send)]
    impl Client for TerminalWaitForExitFailingClient {
        async fn session_notification(
            &self,
            n: agent_client_protocol::SessionNotification,
        ) -> agent_client_protocol::Result<()> {
            let _ = n;
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

        async fn write_text_file(
            &self,
            _: WriteTextFileRequest,
        ) -> agent_client_protocol::Result<WriteTextFileResponse> {
            Ok(WriteTextFileResponse::new())
        }

        async fn create_terminal(
            &self,
            _: CreateTerminalRequest,
        ) -> agent_client_protocol::Result<CreateTerminalResponse> {
            Ok(CreateTerminalResponse::new("term-001"))
        }

        async fn kill_terminal_command(
            &self,
            _: KillTerminalCommandRequest,
        ) -> agent_client_protocol::Result<KillTerminalCommandResponse> {
            Ok(KillTerminalCommandResponse::new())
        }

        async fn terminal_output(
            &self,
            _: TerminalOutputRequest,
        ) -> agent_client_protocol::Result<TerminalOutputResponse> {
            Ok(TerminalOutputResponse::new(
                "mock output".to_string(),
                false,
            ))
        }

        async fn release_terminal(
            &self,
            _: ReleaseTerminalRequest,
        ) -> agent_client_protocol::Result<ReleaseTerminalResponse> {
            Ok(ReleaseTerminalResponse::new())
        }

        async fn wait_for_terminal_exit(
            &self,
            _: WaitForTerminalExitRequest,
        ) -> agent_client_protocol::Result<WaitForTerminalExitResponse> {
            Err(agent_client_protocol::Error::new(
                -32603,
                "mock wait_for_terminal_exit failure",
            ))
        }
    }

    pub(super) struct TerminalWaitForExitTimeoutClient;

    #[async_trait(?Send)]
    impl Client for TerminalWaitForExitTimeoutClient {
        async fn session_notification(
            &self,
            n: agent_client_protocol::SessionNotification,
        ) -> agent_client_protocol::Result<()> {
            let _ = n;
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

        async fn write_text_file(
            &self,
            _: WriteTextFileRequest,
        ) -> agent_client_protocol::Result<WriteTextFileResponse> {
            Ok(WriteTextFileResponse::new())
        }

        async fn create_terminal(
            &self,
            _: CreateTerminalRequest,
        ) -> agent_client_protocol::Result<CreateTerminalResponse> {
            Ok(CreateTerminalResponse::new("term-001"))
        }

        async fn kill_terminal_command(
            &self,
            _: KillTerminalCommandRequest,
        ) -> agent_client_protocol::Result<KillTerminalCommandResponse> {
            Ok(KillTerminalCommandResponse::new())
        }

        async fn terminal_output(
            &self,
            _: TerminalOutputRequest,
        ) -> agent_client_protocol::Result<TerminalOutputResponse> {
            Ok(TerminalOutputResponse::new(
                "mock output".to_string(),
                false,
            ))
        }

        async fn release_terminal(
            &self,
            _: ReleaseTerminalRequest,
        ) -> agent_client_protocol::Result<ReleaseTerminalResponse> {
            Ok(ReleaseTerminalResponse::new())
        }

        async fn wait_for_terminal_exit(
            &self,
            _: WaitForTerminalExitRequest,
        ) -> agent_client_protocol::Result<WaitForTerminalExitResponse> {
            tokio::time::sleep(std::time::Duration::from_secs(60)).await;
            Ok(WaitForTerminalExitResponse::new(
                TerminalExitStatus::new().exit_code(0u32),
            ))
        }
    }

    #[tokio::test]
    async fn dispatch_client_method_dispatches_terminal_kill_client_error_publishes_error_reply() {
        let nats = MockNatsClient::new();
        let client = TerminalKillFailingClient;
        let session_id = AcpSessionId::new("sess-1").unwrap();

        let envelope = Request {
            id: RequestId::Number(1),
            method: std::sync::Arc::from("terminal/kill_command"),
            params: Some(KillTerminalCommandRequest::new(
                agent_client_protocol::SessionId::from("sess-1"),
                "term-001".to_string(),
            )),
        };
        let payload = bytes::Bytes::from(serde_json::to_vec(&envelope).unwrap());

        let parsed = crate::nats::ParsedClientSubject {
            session_id,
            method: ClientMethod::TerminalKill,
        };

        let bridge = make_bridge(nats.clone());
        let ctx = DispatchContext {
            nats: &nats,
            client: &client,
            bridge: &bridge,
            serializer: &StdJsonSerialize,
        };
        dispatch_client_method(
            "acp.sess-1.client.terminal.kill",
            parsed,
            payload,
            Some("_INBOX.reply".to_string()),
            &ctx,
        )
        .await;

        assert_eq!(nats.published_messages(), vec!["_INBOX.reply"]);
        let payloads = nats.published_payloads();
        assert_eq!(payloads.len(), 1);
        let response: serde_json::Value = serde_json::from_slice(payloads[0].as_ref()).unwrap();
        assert!(response.get("error").is_some());
        assert_eq!(
            response.get("error").and_then(|e| e.get("code")),
            Some(&serde_json::Value::from(-32603))
        );
        assert_eq!(
            response.get("error").and_then(|e| e.get("message")),
            Some(&serde_json::Value::from(
                "mock kill_terminal_command failure"
            ))
        );
    }

    #[tokio::test]
    async fn dispatch_client_method_terminal_kill_client_error_serialization_fallback() {
        let nats = MockNatsClient::new();
        let client = TerminalKillFailingClient;
        let serializer = FailNextSerialize::new(1);
        let session_id = AcpSessionId::new("sess-1").unwrap();

        let envelope = Request {
            id: RequestId::Number(42),
            method: std::sync::Arc::from("terminal/kill_command"),
            params: Some(KillTerminalCommandRequest::new(
                agent_client_protocol::SessionId::from("sess-1"),
                "term-001".to_string(),
            )),
        };
        let payload = bytes::Bytes::from(serde_json::to_vec(&envelope).unwrap());

        let parsed = crate::nats::ParsedClientSubject {
            session_id,
            method: ClientMethod::TerminalKill,
        };

        let bridge = make_bridge(nats.clone());
        let ctx = DispatchContext {
            nats: &nats,
            client: &client,
            bridge: &bridge,
            serializer: &serializer,
        };
        dispatch_client_method(
            "acp.sess-1.client.terminal.kill",
            parsed,
            payload,
            Some("_INBOX.reply".to_string()),
            &ctx,
        )
        .await;

        assert_eq!(nats.published_messages(), vec!["_INBOX.reply"]);
        let payloads = nats.published_payloads();
        assert_eq!(payloads.len(), 1);
        let response: serde_json::Value = serde_json::from_slice(payloads[0].as_ref()).unwrap();
        assert_eq!(response.get("id"), Some(&serde_json::Value::Null));
        assert_eq!(
            response.get("error").and_then(|e| e.get("code")),
            Some(&serde_json::Value::from(-32603))
        );
    }

    #[tokio::test]
    async fn dispatch_client_method_terminal_kill_success_publish_failure_exercises_error_path() {
        let nats = AdvancedMockNatsClient::new();
        nats.fail_next_publish();
        let client = MockClient::new();
        let session_id = AcpSessionId::new("sess-1").unwrap();

        let envelope = Request {
            id: RequestId::Number(1),
            method: std::sync::Arc::from("terminal/kill_command"),
            params: Some(KillTerminalCommandRequest::new(
                agent_client_protocol::SessionId::from("sess-1"),
                "term-001".to_string(),
            )),
        };
        let payload = bytes::Bytes::from(serde_json::to_vec(&envelope).unwrap());

        let parsed = crate::nats::ParsedClientSubject {
            session_id,
            method: ClientMethod::TerminalKill,
        };

        let bridge = make_bridge_advanced(nats.clone());
        let ctx = DispatchContext {
            nats: &nats,
            client: &client,
            bridge: &bridge,
            serializer: &StdJsonSerialize,
        };
        dispatch_client_method(
            "acp.sess-1.client.terminal.kill",
            parsed,
            payload,
            Some("_INBOX.reply".to_string()),
            &ctx,
        )
        .await;

        assert!(nats.published_messages().is_empty());
    }

    #[tokio::test]
    async fn dispatch_client_method_terminal_kill_success_flush_failure_exercises_warn_path() {
        let nats = AdvancedMockNatsClient::new();
        nats.fail_next_flush();
        let client = MockClient::new();
        let session_id = AcpSessionId::new("sess-1").unwrap();

        let envelope = Request {
            id: RequestId::Number(1),
            method: std::sync::Arc::from("terminal/kill_command"),
            params: Some(KillTerminalCommandRequest::new(
                agent_client_protocol::SessionId::from("sess-1"),
                "term-001".to_string(),
            )),
        };
        let payload = bytes::Bytes::from(serde_json::to_vec(&envelope).unwrap());

        let parsed = crate::nats::ParsedClientSubject {
            session_id,
            method: ClientMethod::TerminalKill,
        };

        let bridge = make_bridge_advanced(nats.clone());
        let ctx = DispatchContext {
            nats: &nats,
            client: &client,
            bridge: &bridge,
            serializer: &StdJsonSerialize,
        };
        dispatch_client_method(
            "acp.sess-1.client.terminal.kill",
            parsed,
            payload,
            Some("_INBOX.reply".to_string()),
            &ctx,
        )
        .await;

        assert_eq!(nats.published_messages(), vec!["_INBOX.reply"]);
    }

    #[tokio::test]
    async fn dispatch_client_method_terminal_kill_client_error_publish_failure_exercises_error_path()
     {
        let nats = AdvancedMockNatsClient::new();
        nats.fail_next_publish();
        let client = TerminalKillFailingClient;
        let session_id = AcpSessionId::new("sess-1").unwrap();

        let envelope = Request {
            id: RequestId::Number(1),
            method: std::sync::Arc::from("terminal/kill_command"),
            params: Some(KillTerminalCommandRequest::new(
                agent_client_protocol::SessionId::from("sess-1"),
                "term-001".to_string(),
            )),
        };
        let payload = bytes::Bytes::from(serde_json::to_vec(&envelope).unwrap());

        let parsed = crate::nats::ParsedClientSubject {
            session_id,
            method: ClientMethod::TerminalKill,
        };

        let bridge = make_bridge_advanced(nats.clone());
        let ctx = DispatchContext {
            nats: &nats,
            client: &client,
            bridge: &bridge,
            serializer: &StdJsonSerialize,
        };
        dispatch_client_method(
            "acp.sess-1.client.terminal.kill",
            parsed,
            payload,
            Some("_INBOX.reply".to_string()),
            &ctx,
        )
        .await;

        assert!(nats.published_messages().is_empty());
    }

    #[tokio::test]
    async fn dispatch_client_method_terminal_kill_client_error_flush_failure_exercises_warn_path() {
        let nats = AdvancedMockNatsClient::new();
        nats.fail_next_flush();
        let client = TerminalKillFailingClient;
        let session_id = AcpSessionId::new("sess-1").unwrap();

        let envelope = Request {
            id: RequestId::Number(1),
            method: std::sync::Arc::from("terminal/kill_command"),
            params: Some(KillTerminalCommandRequest::new(
                agent_client_protocol::SessionId::from("sess-1"),
                "term-001".to_string(),
            )),
        };
        let payload = bytes::Bytes::from(serde_json::to_vec(&envelope).unwrap());

        let parsed = crate::nats::ParsedClientSubject {
            session_id,
            method: ClientMethod::TerminalKill,
        };

        let bridge = make_bridge_advanced(nats.clone());
        let ctx = DispatchContext {
            nats: &nats,
            client: &client,
            bridge: &bridge,
            serializer: &StdJsonSerialize,
        };
        dispatch_client_method(
            "acp.sess-1.client.terminal.kill",
            parsed,
            payload,
            Some("_INBOX.reply".to_string()),
            &ctx,
        )
        .await;

        assert_eq!(nats.published_messages(), vec!["_INBOX.reply"]);
    }

    #[tokio::test]
    async fn dispatch_client_method_dispatches_session_update_with_terminal_kill_failing_client() {
        let nats = MockNatsClient::new();
        let client = TerminalKillFailingClient;
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

        let bridge = make_bridge(nats.clone());
        let ctx = DispatchContext {
            nats: &nats,
            client: &client,
            bridge: &bridge,
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
    }

    #[tokio::test]
    async fn dispatch_client_method_dispatches_fs_read_text_file_with_terminal_kill_failing_client()
    {
        let nats = MockNatsClient::new();
        let client = TerminalKillFailingClient;
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

        let bridge = make_bridge(nats.clone());
        let ctx = DispatchContext {
            nats: &nats,
            client: &client,
            bridge: &bridge,
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
    async fn dispatch_client_method_dispatches_terminal_create_with_terminal_kill_failing_client() {
        let nats = MockNatsClient::new();
        let client = TerminalKillFailingClient;
        let session_id = AcpSessionId::new("sess-1").unwrap();

        let envelope = Request {
            id: RequestId::Number(1),
            method: std::sync::Arc::from("terminal/create"),
            params: Some(CreateTerminalRequest::new("sess-1", "echo hi")),
        };
        let payload = bytes::Bytes::from(serde_json::to_vec(&envelope).unwrap());

        let parsed = crate::nats::ParsedClientSubject {
            session_id,
            method: ClientMethod::TerminalCreate,
        };

        let bridge = make_bridge(nats.clone());
        let ctx = DispatchContext {
            nats: &nats,
            client: &client,
            bridge: &bridge,
            serializer: &StdJsonSerialize,
        };
        dispatch_client_method(
            "acp.sess-1.client.terminal.create",
            parsed,
            payload,
            Some("_INBOX.reply".to_string()),
            &ctx,
        )
        .await;

        assert_eq!(nats.published_messages(), vec!["_INBOX.reply"]);
    }

    #[tokio::test]
    async fn dispatch_client_method_dispatches_request_permission_with_terminal_kill_failing_client()
     {
        let nats = MockNatsClient::new();
        let client = TerminalKillFailingClient;
        let session_id = AcpSessionId::new("sess-1").unwrap();

        let tool_call = ToolCallUpdate::new("call-1", ToolCallUpdateFields::new());
        let request = RequestPermissionRequest::new("sess-1", tool_call, vec![]);
        let envelope = Request {
            id: RequestId::Number(1),
            method: std::sync::Arc::from("session/request_permission"),
            params: Some(request),
        };
        let payload = bytes::Bytes::from(serde_json::to_vec(&envelope).unwrap());

        let parsed = crate::nats::ParsedClientSubject {
            session_id,
            method: ClientMethod::SessionRequestPermission,
        };

        let bridge = make_bridge(nats.clone());
        let ctx = DispatchContext {
            nats: &nats,
            client: &client,
            bridge: &bridge,
            serializer: &StdJsonSerialize,
        };
        dispatch_client_method(
            "acp.sess-1.client.session.request_permission",
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

        let bridge = make_bridge_advanced(nats.clone());
        let ctx = DispatchContext {
            nats: &nats,
            client: &client,
            bridge: &bridge,
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

        let bridge = make_bridge(nats.clone());
        let ctx = DispatchContext {
            nats: &nats,
            client: &client,
            bridge: &bridge,
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

    #[derive(Debug)]
    struct RpcMockClient;

    #[async_trait(?Send)]
    impl Client for RpcMockClient {
        async fn session_notification(
            &self,
            _: SessionNotification,
        ) -> agent_client_protocol::Result<()> {
            Ok(())
        }

        async fn request_permission(
            &self,
            _: RequestPermissionRequest,
        ) -> agent_client_protocol::Result<RequestPermissionResponse> {
            Ok(RequestPermissionResponse::new(
                RequestPermissionOutcome::Cancelled,
            ))
        }

        async fn read_text_file(
            &self,
            _: ReadTextFileRequest,
        ) -> agent_client_protocol::Result<ReadTextFileResponse> {
            Ok(ReadTextFileResponse::new("file contents".to_string()))
        }

        async fn write_text_file(
            &self,
            _: WriteTextFileRequest,
        ) -> agent_client_protocol::Result<WriteTextFileResponse> {
            Ok(WriteTextFileResponse::new())
        }

        async fn terminal_output(
            &self,
            _: TerminalOutputRequest,
        ) -> agent_client_protocol::Result<TerminalOutputResponse> {
            Ok(TerminalOutputResponse::new(
                "rpc mock output".to_string(),
                false,
            ))
        }
    }

    #[tokio::test]
    async fn dispatch_client_method_dispatches_session_update_with_rpc_mock_client() {
        let nats = MockNatsClient::new();
        let client = RpcMockClient;
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

        let bridge = make_bridge(nats.clone());
        let ctx = DispatchContext {
            nats: &nats,
            client: &client,
            bridge: &bridge,
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
    }

    #[tokio::test]
    async fn dispatch_client_method_dispatches_fs_read_text_file_with_rpc_mock_client() {
        let nats = MockNatsClient::new();
        let client = RpcMockClient;
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

        let bridge = make_bridge(nats.clone());
        let ctx = DispatchContext {
            nats: &nats,
            client: &client,
            bridge: &bridge,
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
    async fn dispatch_client_method_dispatches_request_permission() {
        let nats = MockNatsClient::new();
        let client = RpcMockClient;
        let session_id = AcpSessionId::new("sess-1").unwrap();

        let request = RequestPermissionRequest::new(
            "sess-1",
            agent_client_protocol::ToolCallUpdate::new(
                "call-1",
                agent_client_protocol::ToolCallUpdateFields::new(),
            ),
            vec![],
        );
        let envelope = Request {
            id: RequestId::Number(1),
            method: std::sync::Arc::from("session/request_permission"),
            params: Some(request),
        };
        let payload = bytes::Bytes::from(serde_json::to_vec(&envelope).unwrap());

        let parsed = crate::nats::ParsedClientSubject {
            session_id,
            method: ClientMethod::SessionRequestPermission,
        };

        let bridge = make_bridge(nats.clone());
        let ctx = DispatchContext {
            nats: &nats,
            client: &client,
            bridge: &bridge,
            serializer: &StdJsonSerialize,
        };
        dispatch_client_method(
            "acp.sess-1.client.session.request_permission",
            parsed,
            payload,
            Some("_INBOX.reply".to_string()),
            &ctx,
        )
        .await;

        assert_eq!(nats.published_messages(), vec!["_INBOX.reply"]);
    }

    #[tokio::test]
    async fn dispatch_client_method_rpc_mock_client_write_text_file_covers_stubs() {
        let nats = MockNatsClient::new();
        let client = RpcMockClient;
        let envelope = Request {
            id: RequestId::Number(1),
            method: std::sync::Arc::from("fs/write_text_file"),
            params: Some(WriteTextFileRequest::new(
                agent_client_protocol::SessionId::from("sess-1"),
                "/tmp/foo.txt".to_string(),
                "content".to_string(),
            )),
        };
        let payload = bytes::Bytes::from(serde_json::to_vec(&envelope).unwrap());
        let parsed = crate::nats::ParsedClientSubject {
            session_id: AcpSessionId::new("sess-1").unwrap(),
            method: ClientMethod::FsWriteTextFile,
        };
        let bridge = make_bridge(nats.clone());
        let ctx = DispatchContext {
            nats: &nats,
            client: &client,
            bridge: &bridge,
            serializer: &StdJsonSerialize,
        };
        dispatch_client_method(
            "acp.sess-1.client.fs.write_text_file",
            parsed,
            payload,
            Some("_INBOX.reply".to_string()),
            &ctx,
        )
        .await;
        assert_eq!(nats.published_messages(), vec!["_INBOX.reply"]);
    }

    #[tokio::test]
    async fn dispatch_client_method_dispatches_request_permission_client_error_publishes_error_reply()
     {
        let nats = MockNatsClient::new();
        let client = MockClient::new();
        let session_id = AcpSessionId::new("sess-1").unwrap();

        let request = RequestPermissionRequest::new(
            "sess-1",
            agent_client_protocol::ToolCallUpdate::new(
                "call-1",
                agent_client_protocol::ToolCallUpdateFields::new(),
            ),
            vec![],
        );
        let envelope = Request {
            id: RequestId::Number(1),
            method: std::sync::Arc::from("session/request_permission"),
            params: Some(request),
        };
        let payload = bytes::Bytes::from(serde_json::to_vec(&envelope).unwrap());

        let parsed = crate::nats::ParsedClientSubject {
            session_id,
            method: ClientMethod::SessionRequestPermission,
        };

        let bridge = make_bridge(nats.clone());
        let ctx = DispatchContext {
            nats: &nats,
            client: &client,
            bridge: &bridge,
            serializer: &StdJsonSerialize,
        };
        dispatch_client_method(
            "acp.sess-1.client.session.request_permission",
            parsed,
            payload,
            Some("_INBOX.err".to_string()),
            &ctx,
        )
        .await;

        assert_eq!(nats.published_messages(), vec!["_INBOX.err"]);
    }

    #[tokio::test]
    async fn dispatch_client_method_dispatches_request_permission_with_advanced_mock() {
        let nats = AdvancedMockNatsClient::new();
        let client = MockClient::new();
        let session_id = AcpSessionId::new("sess-1").unwrap();

        let request = RequestPermissionRequest::new(
            "sess-1",
            agent_client_protocol::ToolCallUpdate::new(
                "call-1",
                agent_client_protocol::ToolCallUpdateFields::new(),
            ),
            vec![],
        );
        let envelope = Request {
            id: RequestId::Number(1),
            method: std::sync::Arc::from("session/request_permission"),
            params: Some(request),
        };
        let payload = bytes::Bytes::from(serde_json::to_vec(&envelope).unwrap());

        let parsed = crate::nats::ParsedClientSubject {
            session_id,
            method: ClientMethod::SessionRequestPermission,
        };

        let bridge = make_bridge_advanced(nats.clone());
        let ctx = DispatchContext {
            nats: &nats,
            client: &client,
            bridge: &bridge,
            serializer: &StdJsonSerialize,
        };
        dispatch_client_method(
            "acp.sess-1.client.session.request_permission",
            parsed,
            payload,
            Some("_INBOX.err".to_string()),
            &ctx,
        )
        .await;

        assert_eq!(nats.published_messages(), vec!["_INBOX.err"]);
    }

    #[tokio::test]
    async fn dispatch_client_method_dispatches_request_permission_client_error_serialization_fallback()
     {
        let nats = MockNatsClient::new();
        let client = MockClient::new();
        let serializer = FailNextSerialize::new(1);
        let session_id = AcpSessionId::new("sess-1").unwrap();

        let request = RequestPermissionRequest::new(
            "sess-1",
            agent_client_protocol::ToolCallUpdate::new(
                "call-1",
                agent_client_protocol::ToolCallUpdateFields::new(),
            ),
            vec![],
        );
        let envelope = Request {
            id: RequestId::Number(1),
            method: std::sync::Arc::from("session/request_permission"),
            params: Some(request),
        };
        let payload = bytes::Bytes::from(serde_json::to_vec(&envelope).unwrap());

        let parsed = crate::nats::ParsedClientSubject {
            session_id,
            method: ClientMethod::SessionRequestPermission,
        };

        let bridge = make_bridge(nats.clone());
        let ctx = DispatchContext {
            nats: &nats,
            client: &client,
            bridge: &bridge,
            serializer: &serializer,
        };
        dispatch_client_method(
            "acp.sess-1.client.session.request_permission",
            parsed,
            payload,
            Some("_INBOX.err".to_string()),
            &ctx,
        )
        .await;

        assert_eq!(nats.published_messages(), vec!["_INBOX.err"]);
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
