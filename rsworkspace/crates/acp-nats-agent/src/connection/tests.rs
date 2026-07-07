use super::*;
use acp_nats::nats::{ParsedAgentSubject, parse_agent_subject};
use agent_client_protocol::{
    AuthenticateResponse, Error as AcpError, ErrorCode, InitializeResponse, LogoutResponse, NewSessionResponse,
    PromptResponse, StopReason,
};
use std::cell::RefCell;
use tracing_subscriber::util::SubscriberInitExt;
use trogon_nats::MockNatsClient;
use trogon_nats::jetstream::{MockJetStreamConsumer, MockJetStreamConsumerFactory};

struct MockAgent {
    initialized: RefCell<bool>,
    cancelled: RefCell<Vec<String>>,
    fail_cancel: bool,
    received_new_session: RefCell<Option<NewSessionRequest>>,
}

impl MockAgent {
    fn new() -> Self {
        Self {
            initialized: RefCell::new(false),
            cancelled: RefCell::new(Vec::new()),
            fail_cancel: false,
            received_new_session: RefCell::new(None),
        }
    }

    fn failing_cancel() -> Self {
        Self {
            initialized: RefCell::new(false),
            cancelled: RefCell::new(Vec::new()),
            fail_cancel: true,
            received_new_session: RefCell::new(None),
        }
    }
}

#[async_trait::async_trait(?Send)]
impl Agent for MockAgent {
    async fn initialize(&self, _args: InitializeRequest) -> agent_client_protocol::Result<InitializeResponse> {
        *self.initialized.borrow_mut() = true;
        Ok(InitializeResponse::new(agent_client_protocol::ProtocolVersion::V0))
    }

    async fn authenticate(&self, _args: AuthenticateRequest) -> agent_client_protocol::Result<AuthenticateResponse> {
        Err(AcpError::method_not_found())
    }

    async fn logout(&self, _args: LogoutRequest) -> agent_client_protocol::Result<LogoutResponse> {
        Ok(LogoutResponse::new())
    }

    async fn new_session(
        &self,
        args: NewSessionRequest,
    ) -> agent_client_protocol::Result<agent_client_protocol::NewSessionResponse> {
        *self.received_new_session.borrow_mut() = Some(args);
        Ok(agent_client_protocol::NewSessionResponse::new("sess-1"))
    }

    async fn prompt(&self, _args: PromptRequest) -> agent_client_protocol::Result<PromptResponse> {
        Ok(PromptResponse::new(StopReason::EndTurn))
    }

    async fn cancel(&self, args: CancelNotification) -> agent_client_protocol::Result<()> {
        if self.fail_cancel {
            return Err(AcpError::internal_error());
        }
        self.cancelled.borrow_mut().push(args.session_id.to_string());
        Ok(())
    }
}

fn make_nats_message(subject: &str, payload: &[u8], reply: Option<&str>) -> Message {
    Message {
        subject: subject.into(),
        reply: reply.map(|r| r.into()),
        payload: Bytes::copy_from_slice(payload),
        headers: None,
        status: None,
        description: None,
        length: 0,
    }
}

fn serialize<T: serde::Serialize>(value: &T) -> Vec<u8> {
    serde_json::to_vec(value).unwrap()
}

fn wire_method_for_subject(subject: &str) -> String {
    match parse_agent_subject(subject).expect("valid test subject") {
        ParsedAgentSubject::Global(method) => method.wire_method(),
        ParsedAgentSubject::Session { method, .. } => method.wire_method().to_string(),
    }
}

fn wire_encode_request<T: serde::Serialize>(method: &str, args: &T) -> jsonrpc_nats::Encoded {
    acp_nats::wire::encode_request(method, jsonrpc_nats::RequestId::Number(1), args).unwrap()
}

fn wire_encode_notification<T: serde::Serialize>(method: &str, args: &T) -> jsonrpc_nats::Encoded {
    acp_nats::wire::encode_notification(method, args).unwrap()
}

async fn dispatch<T: serde::Serialize>(subject: &str, args: &T, reply: Option<&str>) -> (MockNatsClient, MockAgent) {
    let nats = MockNatsClient::new();
    let agent = MockAgent::new();
    let method = wire_method_for_subject(subject);
    let encoded = if reply.is_none() && method == "cancel" {
        wire_encode_notification(&method, args)
    } else {
        wire_encode_request(&method, args)
    };
    let mut msg = make_nats_message(subject, &encoded.body, reply);
    msg.headers = Some(encoded.headers);
    dispatch_message(msg, &agent, &nats).await;
    (nats, agent)
}

async fn dispatch_raw(subject: &str, payload: &[u8], reply: Option<&str>) -> (MockNatsClient, MockAgent) {
    let nats = MockNatsClient::new();
    let agent = MockAgent::new();
    let msg = make_nats_message(subject, payload, reply);
    dispatch_message(msg, &agent, &nats).await;
    (nats, agent)
}

fn published_response<T: serde::de::DeserializeOwned>(nats: &MockNatsClient) -> T {
    let payloads = nats.published_payloads();
    let headers = nats.published_headers();
    assert_eq!(payloads.len(), 1);
    if headers
        .first()
        .is_some_and(|h| h.get(jsonrpc_nats::HEADER_ID).is_some() || h.get(jsonrpc_nats::HEADER_ERROR_CODE).is_some())
    {
        match acp_nats::wire::decode_response::<T>(&headers[0], &payloads[0]).unwrap() {
            Ok(value) => value,
            Err(error) => panic!("expected success response, got error: {error}"),
        }
    } else {
        serde_json::from_slice(&payloads[0]).unwrap()
    }
}

fn init_request() -> InitializeRequest {
    InitializeRequest::new(agent_client_protocol::ProtocolVersion::V0)
}

fn test_prefix() -> AcpPrefix {
    AcpPrefix::new("acp").expect("test prefix")
}

fn test_prefix_custom(s: &str) -> AcpPrefix {
    AcpPrefix::new(s).expect("test prefix")
}

fn test_session_id(s: &str) -> AcpSessionId {
    AcpSessionId::new(s).expect("test session id")
}

#[tokio::test]
async fn dispatch_initialize_calls_agent_and_publishes_response() {
    let (nats, agent) = dispatch("acp.agent.initialize", &init_request(), Some("_INBOX.1")).await;

    assert!(*agent.initialized.borrow());
    let response: InitializeResponse = published_response(&nats);
    assert_eq!(response.protocol_version, agent_client_protocol::ProtocolVersion::V0);
}

async fn dispatch_authenticate<T: serde::Serialize>(
    subject: &str,
    args: &T,
    reply: Option<&str>,
) -> (MockNatsClient, MockAgent) {
    let nats = MockNatsClient::new();
    let agent = MockAgent::new();
    let encoded = acp_nats::wire::encode_request("authenticate", jsonrpc_nats::RequestId::Number(1), args).unwrap();
    let mut msg = make_nats_message(subject, &encoded.body, reply);
    msg.headers = Some(encoded.headers);
    dispatch_message(msg, &agent, &nats).await;
    (nats, agent)
}

fn published_wire_error(nats: &MockNatsClient) -> AcpError {
    let payloads = nats.published_payloads();
    let headers = nats.published_headers();
    assert_eq!(payloads.len(), 1);
    match acp_nats::wire::decode_response::<serde_json::Value>(&headers[0], &payloads[0]).unwrap() {
        Ok(_) => panic!("expected error response"),
        Err(error) => error,
    }
}

#[tokio::test]
async fn dispatch_authenticate_error_publishes_jsonrpc_error_header() {
    let (nats, _) = dispatch_authenticate(
        "acp.agent.authenticate",
        &AuthenticateRequest::new("basic"),
        Some("_INBOX.2"),
    )
    .await;

    let error = published_wire_error(&nats);
    assert_eq!(error.code, ErrorCode::MethodNotFound);
}

#[tokio::test]
async fn dispatch_logout_publishes_response() {
    let (nats, _) = dispatch("acp.agent.logout", &LogoutRequest::new(), Some("_INBOX.r")).await;
    assert_eq!(nats.published_messages(), vec!["_INBOX.r"]);
    let _: LogoutResponse = published_response(&nats);
}

#[tokio::test]
async fn dispatch_cancel_is_notification_no_reply_published() {
    let (nats, agent) = dispatch("acp.session.s1.agent.cancel", &CancelNotification::new("s1"), None).await;

    assert_eq!(agent.cancelled.borrow().as_slice(), ["s1"]);
    assert!(nats.published_messages().is_empty());
}

#[tokio::test]
async fn dispatch_invalid_payload_publishes_error_reply() {
    let (nats, agent) = dispatch_raw("acp.agent.initialize", b"not json", Some("_INBOX.err")).await;

    assert!(!*agent.initialized.borrow());
    let error = published_wire_error(&nats);
    assert_eq!(error.code, ErrorCode::InvalidParams);
}

#[tokio::test]
async fn dispatch_request_without_reply_subject_does_not_publish() {
    let (nats, _) = dispatch("acp.agent.initialize", &init_request(), None).await;
    assert!(nats.published_messages().is_empty());
}

#[tokio::test]
async fn dispatch_unknown_subject_is_silently_ignored() {
    let (nats, _) = dispatch_raw("acp.something.else", b"{}", Some("_INBOX.1")).await;
    assert!(nats.published_messages().is_empty());
}

#[tokio::test]
async fn dispatch_prompt_returns_stop_reason() {
    let (nats, _) = dispatch(
        "acp.session.s1.agent.prompt",
        &PromptRequest::new("s1", vec![]),
        Some("_INBOX.3"),
    )
    .await;

    let response: PromptResponse = published_response(&nats);
    assert_eq!(response.stop_reason, StopReason::EndTurn);
}

#[tokio::test]
async fn dispatch_publishes_to_correct_reply_subject() {
    let (nats, _) = dispatch("acp.agent.initialize", &init_request(), Some("_INBOX.specific")).await;
    assert_eq!(nats.published_messages(), vec!["_INBOX.specific"]);
    let response: InitializeResponse = published_response(&nats);
    assert_eq!(response.protocol_version, agent_client_protocol::ProtocolVersion::V0);
}

#[test]
fn connection_error_display() {
    let err = ConnectionError::Subscribe(Box::new(std::io::Error::other("test")));
    assert_eq!(err.to_string(), "failed to subscribe: test");
}

#[test]
fn connection_error_debug() {
    let err = ConnectionError::Subscribe(Box::new(std::io::Error::other("test")));
    let debug = format!("{:?}", err);
    assert!(debug.contains("Subscribe"));
}

#[test]
fn dispatch_error_display_variants() {
    assert_eq!(DispatchError::NoReplySubject.to_string(), "no reply subject");

    let json_err = serde_json::from_slice::<()>(b"bad").unwrap_err();
    assert!(matches!(
        DispatchError::DeserializeRequest(json_err),
        DispatchError::DeserializeRequest(_)
    ));

    let json_err = serde_json::from_slice::<()>(b"bad").unwrap_err();
    assert!(matches!(
        DispatchError::DeserializeNotification(json_err),
        DispatchError::DeserializeNotification(_)
    ));

    let acp_err = agent_client_protocol::Error::internal_error();
    assert!(matches!(
        DispatchError::NotificationHandler(acp_err),
        DispatchError::NotificationHandler(_)
    ));
}

fn raw_value(json: &str) -> std::sync::Arc<serde_json::value::RawValue> {
    std::sync::Arc::from(serde_json::value::RawValue::from_string(json.to_string()).unwrap())
}

#[tokio::test]
async fn dispatch_ext_with_reply_calls_ext_method() {
    let (nats, _) = dispatch(
        "acp.agent.ext.my_tool",
        &agent_client_protocol::ExtRequest::new("my_tool", raw_value("{}")),
        Some("_INBOX.ext"),
    )
    .await;
    assert_eq!(nats.published_messages(), vec!["_INBOX.ext"]);
    let value: serde_json::Value = published_response(&nats);
    assert!(value.is_null());
}

#[tokio::test]
async fn dispatch_ext_without_reply_calls_ext_notification() {
    let (nats, _) = dispatch(
        "acp.agent.ext.my_tool",
        &agent_client_protocol::ExtNotification::new("my_tool", raw_value("{}")),
        None,
    )
    .await;
    assert!(nats.published_messages().is_empty());
}

async fn assert_dispatch_method_not_found<T: serde::Serialize>(subject: &str, args: &T) {
    let (nats, _) = dispatch(subject, args, Some("_INBOX.r")).await;
    assert_eq!(nats.published_messages(), vec!["_INBOX.r"]);
    let error = published_wire_error(&nats);
    assert_eq!(error.code, ErrorCode::MethodNotFound);
}

#[tokio::test]
async fn dispatch_new_session_publishes_response() {
    let (nats, _) = dispatch(
        "acp.agent.session.new",
        &NewSessionRequest::new("/tmp"),
        Some("_INBOX.r"),
    )
    .await;

    assert_eq!(nats.published_messages(), vec!["_INBOX.r"]);
    let response: NewSessionResponse = published_response(&nats);
    assert_eq!(response.session_id.to_string(), "sess-1");
}

#[tokio::test]
async fn dispatch_new_session_additional_directories_survive_bridge() {
    let request = NewSessionRequest::new("/tmp").additional_directories(vec![std::path::PathBuf::from("/tmp/extra")]);
    let (nats, agent) = dispatch("acp.agent.session.new", &request, Some("_INBOX.r")).await;

    assert_eq!(nats.published_messages(), vec!["_INBOX.r"]);
    let received = agent.received_new_session.borrow();
    assert_eq!(
        received.as_ref().unwrap().additional_directories,
        request.additional_directories
    );
}

#[tokio::test]
async fn dispatch_session_load_publishes_response() {
    assert_dispatch_method_not_found("acp.session.s1.agent.load", &LoadSessionRequest::new("s1", "/tmp")).await;
}

#[tokio::test]
async fn dispatch_session_load_additional_directories_survive_wire_decode() {
    let request =
        LoadSessionRequest::new("s1", "/tmp").additional_directories(vec![std::path::PathBuf::from("/tmp/extra")]);
    let method = wire_method_for_subject("acp.session.s1.agent.load");
    let encoded = wire_encode_request(&method, &request);
    let decoded: LoadSessionRequest =
        acp_nats::wire::decode_request_params(&method, &encoded.headers, &encoded.body).unwrap();

    assert_eq!(decoded.additional_directories, request.additional_directories);
}

#[tokio::test]
async fn dispatch_list_sessions_publishes_response() {
    assert_dispatch_method_not_found("acp.agent.session.list", &ListSessionsRequest::new()).await;
}

#[tokio::test]
async fn dispatch_set_session_mode_publishes_response() {
    assert_dispatch_method_not_found(
        "acp.session.s1.agent.set_mode",
        &SetSessionModeRequest::new("s1", "code"),
    )
    .await;
}

#[tokio::test]
async fn dispatch_set_session_config_option_publishes_response() {
    assert_dispatch_method_not_found(
        "acp.session.s1.agent.set_config_option",
        &SetSessionConfigOptionRequest::new("s1", "key", "val"),
    )
    .await;
}

#[tokio::test]
async fn dispatch_set_session_model_publishes_response() {
    assert_dispatch_method_not_found(
        "acp.session.s1.agent.set_model",
        &SetSessionModelRequest::new("s1", "gpt-4"),
    )
    .await;
}

#[tokio::test]
async fn dispatch_fork_session_publishes_response() {
    assert_dispatch_method_not_found("acp.session.s1.agent.fork", &ForkSessionRequest::new("s1", "/tmp")).await;
}

#[tokio::test]
async fn dispatch_resume_session_publishes_response() {
    assert_dispatch_method_not_found("acp.session.s1.agent.resume", &ResumeSessionRequest::new("s1", "/tmp")).await;
}

#[tokio::test]
async fn dispatch_close_session_publishes_response() {
    assert_dispatch_method_not_found("acp.session.s1.agent.close", &CloseSessionRequest::new("s1")).await;
}

#[test]
fn dispatch_error_display_reply_variant() {
    let err = DispatchError::Reply(trogon_nats::NatsError::Timeout {
        subject: "test".to_string(),
    });
    assert_eq!(
        err.to_string(),
        "reply: Request to 'test' timed out. The backend may be overloaded or unresponsive."
    );
}

#[tokio::test]
async fn new_runs_io_task_to_completion() {
    let nats = MockNatsClient::new();
    let global_tx = nats.inject_messages();
    let session_tx = nats.inject_messages();
    drop(global_tx);
    drop(session_tx);

    let local = tokio::task::LocalSet::new();
    local
        .run_until(async {
            let (conn, io_task) = AgentSideNatsConnection::new(MockAgent::new(), nats, test_prefix(), |fut| {
                tokio::task::spawn_local(fut);
            });

            assert_eq!(conn.acp_prefix.as_str(), "acp");

            let result = io_task.await;
            assert!(result.is_ok());
        })
        .await;
}

#[tokio::test]
async fn client_for_session_returns_proxy() {
    let nats = MockNatsClient::new();
    let global_tx = nats.inject_messages();
    let session_tx = nats.inject_messages();
    drop(global_tx);
    drop(session_tx);

    let local = tokio::task::LocalSet::new();
    local
        .run_until(async {
            let (conn, io_task) = AgentSideNatsConnection::new(MockAgent::new(), nats, test_prefix(), |fut| {
                tokio::task::spawn_local(fut);
            });

            let _client = conn.client_for_session(test_session_id("sess-1"));

            let result = io_task.await;
            assert!(result.is_ok());
        })
        .await;
}

use trogon_nats::jetstream::mocks::*;

fn make_js_msg_raw(subject: &str, payload: &[u8], reply: Option<&str>) -> MockJsMessage {
    let mut headers = async_nats::HeaderMap::new();
    headers.insert(trogon_nats::REQ_ID_HEADER, "req-1");
    MockJsMessage::new(async_nats::Message {
        subject: subject.into(),
        reply: reply.map(|r| r.into()),
        payload: Bytes::copy_from_slice(payload),
        headers: Some(headers),
        status: None,
        description: None,
        length: payload.len(),
    })
}

fn make_js_msg<T: serde::Serialize>(subject: &str, args: &T, reply: Option<&str>) -> MockJsMessage {
    let method = wire_method_for_subject(subject);
    let encoded = if method == "cancel" {
        wire_encode_notification(&method, args)
    } else {
        wire_encode_request(&method, args)
    };
    let mut headers = encoded.headers;
    headers.insert(trogon_nats::REQ_ID_HEADER, "req-1");
    MockJsMessage::new(async_nats::Message {
        subject: subject.into(),
        reply: reply.map(|r| r.into()),
        payload: Bytes::copy_from_slice(&encoded.body),
        headers: Some(headers),
        status: None,
        description: None,
        length: encoded.body.len(),
    })
}

#[tokio::test]
async fn with_jetstream_runs_both_loops() {
    let nats = MockNatsClient::new();
    let agent = MockAgent::new();
    let factory = MockJetStreamConsumerFactory::new();

    // Global + ext subs — drop immediately to end serve_global
    let global_tx = nats.inject_messages();
    let ext_tx = nats.inject_messages();
    drop(global_tx);
    drop(ext_tx);

    // serve_js will fail to create consumer (no mock consumer added) — that's OK
    let local = tokio::task::LocalSet::new();
    local
        .run_until(async {
            let (conn, io_task) = AgentSideNatsConnection::with_jetstream(agent, nats, factory, test_prefix(), |fut| {
                tokio::task::spawn_local(fut);
            });

            assert_eq!(conn.acp_prefix.as_str(), "acp");

            let result = io_task.await;
            // Either serve_global ends (Ok) or serve_js fails (Err) — both are fine
            let _ = result;
        })
        .await;
}

#[tokio::test]
async fn serve_global_subscribes_to_global_and_ext() {
    let nats = MockNatsClient::new();
    let agent = MockAgent::new();

    let global_tx = nats.inject_messages();
    let ext_tx = nats.inject_messages();
    drop(global_tx);
    drop(ext_tx);

    let local = tokio::task::LocalSet::new();
    local
        .run_until(async {
            let _ = serve_global(Rc::new(agent), nats.clone(), &test_prefix_custom("myprefix"), |fut| {
                tokio::task::spawn_local(fut);
            })
            .await;

            let subjects = nats.subscribed_to();
            assert_eq!(subjects.len(), 2);
            assert!(subjects.contains(&"myprefix.agent.>".to_string()));
            assert!(subjects.contains(&"myprefix.session.*.agent.ext.>".to_string()));
            assert!(!subjects.contains(&"myprefix.session.*.agent.>".to_string()));
        })
        .await;
}

#[tokio::test]
async fn serve_global_dispatches_message() {
    let nats = MockNatsClient::new();
    let agent = MockAgent::new();

    let global_tx = nats.inject_messages();
    let ext_tx = nats.inject_messages();

    let local = tokio::task::LocalSet::new();
    local
        .run_until(async {
            let encoded = wire_encode_request(
                "initialize",
                &InitializeRequest::new(agent_client_protocol::ProtocolVersion::V0),
            );
            let msg = Message {
                subject: "acp.agent.initialize".into(),
                reply: Some("_INBOX.serve".into()),
                payload: Bytes::copy_from_slice(&encoded.body),
                headers: Some(encoded.headers),
                status: None,
                description: None,
                length: 0,
            };
            global_tx.unbounded_send(msg).unwrap();
            drop(global_tx);
            drop(ext_tx);

            let _ = serve_global(Rc::new(agent), nats.clone(), &test_prefix(), |fut| {
                tokio::task::spawn_local(fut);
            })
            .await;

            tokio::task::yield_now().await;
            tokio::task::yield_now().await;

            assert_eq!(nats.published_messages(), vec!["_INBOX.serve"]);
            let response: InitializeResponse = published_response(&nats);
            assert_eq!(response.protocol_version, agent_client_protocol::ProtocolVersion::V0);
        })
        .await;
}

#[tokio::test]
async fn serve_js_dispatches_message() {
    let nats = MockNatsClient::new();
    let agent = MockAgent::new();
    let factory = MockJetStreamConsumerFactory::new();

    let (consumer, tx) = MockJetStreamConsumer::new();
    factory.add_consumer(consumer);

    let js_msg = make_js_msg(
        "acp.session.s1.agent.load",
        &LoadSessionRequest::new("s1", "/tmp"),
        None,
    );
    tx.unbounded_send(Ok(js_msg)).unwrap();
    drop(tx);

    let local = tokio::task::LocalSet::new();
    local
        .run_until(async {
            let _ = serve_js(Rc::new(agent), nats.clone(), factory, &test_prefix(), |fut| {
                tokio::task::spawn_local(fut);
            })
            .await;

            tokio::task::yield_now().await;
            tokio::task::yield_now().await;

            assert_js_response_method_not_found(&nats, "acp.session.s1.agent.response.req-1");
        })
        .await;
}

#[tokio::test]
async fn serve_js_handles_consumer_stream_error() {
    let nats = MockNatsClient::new();
    let agent = MockAgent::new();
    let factory = MockJetStreamConsumerFactory::new();

    let (consumer, tx) = MockJetStreamConsumer::new();
    factory.add_consumer(consumer);

    tx.unbounded_send(Err(trogon_nats::mocks::MockError("stream error".into())))
        .unwrap();
    drop(tx);

    let local = tokio::task::LocalSet::new();
    local
        .run_until(async {
            let _ = serve_js(Rc::new(agent), nats.clone(), factory, &test_prefix(), |fut| {
                tokio::task::spawn_local(fut);
            })
            .await;
        })
        .await;
}

#[tokio::test]
async fn serve_js_consumer_creation_failure() {
    let nats = MockNatsClient::new();
    let agent = MockAgent::new();
    let factory = MockJetStreamConsumerFactory::new();

    let local = tokio::task::LocalSet::new();
    local
        .run_until(async {
            let result = serve_js(Rc::new(agent), nats.clone(), factory, &test_prefix(), |fut| {
                tokio::task::spawn_local(fut);
            })
            .await;
            assert!(result.is_err());
        })
        .await;
}

#[tokio::test]
async fn dispatch_js_message_unknown_subject_terms() {
    let nats = MockNatsClient::new();
    let agent = MockAgent::new();
    let js_msg = make_js_msg_raw("acp.unknown.something", b"{}", None);

    dispatch_js_message(js_msg, &agent, &nats, &test_prefix()).await;

    assert!(nats.published_messages().is_empty());
}

#[tokio::test]
async fn dispatch_js_message_bad_payload_terms() {
    let nats = MockNatsClient::new();
    let agent = MockAgent::new();
    let js_msg = make_js_msg_raw("acp.session.s1.agent.load", b"not json", None);

    dispatch_js_message(js_msg, &agent, &nats, &test_prefix()).await;

    assert_eq!(nats.published_messages(), vec!["acp.session.s1.agent.response.req-1"]);
    let error = published_wire_error(&nats);
    assert_eq!(error.code, ErrorCode::InvalidParams);
}

#[tokio::test]
async fn dispatch_js_message_missing_reply_terms() {
    let nats = MockNatsClient::new();
    let agent = MockAgent::new();
    let js_msg = make_js_msg("acp.agent.initialize", &init_request(), None);

    dispatch_js_message(js_msg, &agent, &nats, &test_prefix()).await;

    assert!(nats.published_messages().is_empty());
}

#[tokio::test]
async fn dispatch_js_message_missing_req_id_header() {
    let nats = MockNatsClient::new();
    let agent = MockAgent::new();
    let payload = serialize(&LoadSessionRequest::new("s1", "/tmp"));
    // Create message without X-Req-Id header
    let js_msg = MockJsMessage::new(async_nats::Message {
        subject: "acp.session.s1.agent.load".into(),
        reply: None,
        payload: Bytes::copy_from_slice(&payload),
        headers: None,
        status: None,
        description: None,
        length: payload.len(),
    });
    dispatch_js_message(js_msg, &agent, &nats, &test_prefix()).await;
    // No reply published because no req_id → no reply subject
    assert!(nats.published_messages().is_empty());
}

#[tokio::test]
async fn dispatch_js_message_global_method_returns_early() {
    let nats = MockNatsClient::new();
    let agent = MockAgent::new();
    let js_msg = make_js_msg(
        "acp.agent.initialize",
        &InitializeRequest::new(agent_client_protocol::ProtocolVersion::V0),
        Some("_INBOX.1"),
    );
    dispatch_js_message(js_msg, &agent, &nats, &test_prefix()).await;
    // Global methods return early — no dispatch, no reply
    assert!(nats.published_messages().is_empty());
}

#[tokio::test]
async fn dispatch_js_message_global_method_ack_failure() {
    let nats = MockNatsClient::new();
    let agent = MockAgent::new();
    let js_msg = make_failing_js_msg(
        "acp.agent.initialize",
        &InitializeRequest::new(agent_client_protocol::ProtocolVersion::V0),
    );
    dispatch_js_message(js_msg, &agent, &nats, &test_prefix()).await;
}

fn make_js_msg_no_headers(subject: &str, payload: &[u8]) -> MockJsMessage {
    MockJsMessage::new(async_nats::Message {
        subject: subject.into(),
        reply: None,
        payload: Bytes::copy_from_slice(payload),
        headers: None,
        status: None,
        description: None,
        length: payload.len(),
    })
}

#[tokio::test]
async fn dispatch_js_message_ext_notification_handler_error() {
    let _guard = tracing_subscriber::fmt().with_test_writer().set_default();

    let nats = MockNatsClient::new();
    let agent = MockAgent::new();
    let raw = std::sync::Arc::from(serde_json::value::RawValue::from_string("{}".to_string()).unwrap());
    let payload = serialize(&agent_client_protocol::ExtNotification::new("my_tool", raw));
    // No X-Req-Id → ext notification path (reply_subject is None → msg.reply is None)
    let js_msg = make_js_msg_no_headers("acp.session.s1.agent.ext.my_tool", &payload);
    dispatch_js_message(js_msg, &agent, &nats, &test_prefix()).await;
}

#[tokio::test]
async fn dispatch_js_message_ext_notification_handler_error_ack_failure() {
    let _guard = tracing_subscriber::fmt().with_test_writer().set_default();

    let nats = MockNatsClient::new();
    let agent = MockAgent::new();
    let raw = std::sync::Arc::from(serde_json::value::RawValue::from_string("{}".to_string()).unwrap());
    let payload = serialize(&agent_client_protocol::ExtNotification::new("my_tool", raw));
    let js_msg = MockJsMessage::with_failing_signals(async_nats::Message {
        subject: "acp.session.s1.agent.ext.my_tool".into(),
        reply: None,
        payload: Bytes::copy_from_slice(&payload),
        headers: None,
        status: None,
        description: None,
        length: payload.len(),
    });
    dispatch_js_message(js_msg, &agent, &nats, &test_prefix()).await;
}

#[tokio::test]
async fn dispatch_js_message_global_ext_no_session_id() {
    let nats = MockNatsClient::new();
    let agent = MockAgent::new();
    let raw = std::sync::Arc::from(serde_json::value::RawValue::from_string("{}".to_string()).unwrap());
    let payload = serialize(&agent_client_protocol::ExtNotification::new("my_tool", raw));
    let js_msg = make_js_msg_no_headers("acp.agent.ext.my_tool", &payload);
    dispatch_js_message(js_msg, &agent, &nats, &test_prefix()).await;
}

#[tokio::test]
async fn dispatch_js_message_prompt_uses_prompt_response_subject() {
    let nats = MockNatsClient::new();
    let agent = MockAgent::new();
    let js_msg = make_js_msg("acp.session.s1.agent.prompt", &PromptRequest::new("s1", vec![]), None);

    dispatch_js_message(js_msg, &agent, &nats, &test_prefix()).await;

    assert_eq!(
        nats.published_messages(),
        vec!["acp.session.s1.agent.prompt.response.req-1"]
    );
    let response: PromptResponse = published_response(&nats);
    assert_eq!(response.stop_reason, StopReason::EndTurn);
}

#[tokio::test]
async fn dispatch_js_message_non_prompt_session_uses_response_subject() {
    let nats = MockNatsClient::new();
    let agent = MockAgent::new();
    let js_msg = make_js_msg(
        "acp.session.s1.agent.load",
        &LoadSessionRequest::new("s1", "/tmp"),
        None,
    );

    dispatch_js_message(js_msg, &agent, &nats, &test_prefix()).await;

    assert_js_response_method_not_found(&nats, "acp.session.s1.agent.response.req-1");
}

#[tokio::test]
async fn dispatch_error_logs_warning_with_subscriber() {
    let _guard = tracing_subscriber::fmt().with_test_writer().set_default();

    let nats = MockNatsClient::new();
    let agent = MockAgent::new();
    let payload = serialize(&InitializeRequest::new(agent_client_protocol::ProtocolVersion::V0));
    let msg = make_nats_message("acp.agent.initialize", &payload, None);

    dispatch_message(msg, &agent, &nats).await;
}

#[tokio::test]
async fn serve_subscribes_and_dispatches_messages() {
    let nats = MockNatsClient::new();
    let agent = MockAgent::new();

    let global_tx = nats.inject_messages();
    let session_tx = nats.inject_messages();

    let local = tokio::task::LocalSet::new();
    local
        .run_until(async {
            let encoded = wire_encode_request(
                "initialize",
                &InitializeRequest::new(agent_client_protocol::ProtocolVersion::V0),
            );
            let msg = Message {
                subject: "acp.agent.initialize".into(),
                reply: Some("_INBOX.serve".into()),
                payload: Bytes::copy_from_slice(&encoded.body),
                headers: Some(encoded.headers),
                status: None,
                description: None,
                length: 0,
            };

            global_tx.unbounded_send(msg).unwrap();
            drop(global_tx);
            drop(session_tx);

            let result = serve(agent, nats.clone(), &test_prefix(), |fut| {
                tokio::task::spawn_local(fut);
            })
            .await;

            assert!(result.is_ok());

            tokio::task::yield_now().await;
            tokio::task::yield_now().await;

            assert_eq!(nats.published_messages(), vec!["_INBOX.serve"]);
            let response: InitializeResponse = published_response(&nats);
            assert_eq!(response.protocol_version, agent_client_protocol::ProtocolVersion::V0);
        })
        .await;
}

#[tokio::test]
async fn serve_returns_ok_when_subscription_ends() {
    let nats = MockNatsClient::new();
    let agent = MockAgent::new();

    let global_tx = nats.inject_messages();
    let session_tx = nats.inject_messages();

    drop(global_tx);
    drop(session_tx);

    let local = tokio::task::LocalSet::new();
    local
        .run_until(async {
            let result = serve(agent, nats, &test_prefix(), |fut| {
                tokio::task::spawn_local(fut);
            })
            .await;

            assert!(result.is_ok());
        })
        .await;
}

#[tokio::test]
async fn serve_subscribes_to_correct_subjects() {
    let nats = MockNatsClient::new();
    let agent = MockAgent::new();

    let global_tx = nats.inject_messages();
    let session_tx = nats.inject_messages();

    drop(global_tx);
    drop(session_tx);

    let local = tokio::task::LocalSet::new();
    local
        .run_until(async {
            let _ = serve(agent, nats.clone(), &test_prefix_custom("myprefix"), |fut| {
                tokio::task::spawn_local(fut);
            })
            .await;

            let subjects = nats.subscribed_to();
            assert!(subjects.contains(&"myprefix.agent.>".to_string()));
            assert!(subjects.contains(&"myprefix.session.*.agent.>".to_string()));
        })
        .await;
}

#[test]
fn connection_error_jetstream_display() {
    let err = ConnectionError::JetStream(Box::new(std::io::Error::other("js err")));
    assert_eq!(err.to_string(), "jetstream error: js err");
    let debug = format!("{:?}", err);
    assert!(debug.contains("JetStream"));
}

#[tokio::test]
async fn dispatch_js_message_cancel_notification() {
    let nats = MockNatsClient::new();
    let agent = MockAgent::new();
    let js_msg = make_js_msg("acp.session.s1.agent.cancel", &CancelNotification::new("s1"), None);

    dispatch_js_message(js_msg, &agent, &nats, &test_prefix()).await;

    assert_eq!(agent.cancelled.borrow().as_slice(), ["s1"]);
}

fn assert_js_response_method_not_found(nats: &MockNatsClient, expected_subject: &str) {
    assert_eq!(nats.published_messages(), vec![expected_subject]);
    let error = published_wire_error(nats);
    assert_eq!(error.code, ErrorCode::MethodNotFound);
}

#[tokio::test]
async fn dispatch_js_message_set_mode() {
    let nats = MockNatsClient::new();
    let agent = MockAgent::new();
    let js_msg = make_js_msg(
        "acp.session.s1.agent.set_mode",
        &SetSessionModeRequest::new("s1", "code"),
        None,
    );

    dispatch_js_message(js_msg, &agent, &nats, &test_prefix()).await;

    assert_js_response_method_not_found(&nats, "acp.session.s1.agent.response.req-1");
}

#[tokio::test]
async fn dispatch_js_message_close_session() {
    let nats = MockNatsClient::new();
    let agent = MockAgent::new();
    let js_msg = make_js_msg("acp.session.s1.agent.close", &CloseSessionRequest::new("s1"), None);

    dispatch_js_message(js_msg, &agent, &nats, &test_prefix()).await;

    assert_js_response_method_not_found(&nats, "acp.session.s1.agent.response.req-1");
}

#[tokio::test]
async fn dispatch_js_message_fork_session() {
    let nats = MockNatsClient::new();
    let agent = MockAgent::new();
    let js_msg = make_js_msg(
        "acp.session.s1.agent.fork",
        &ForkSessionRequest::new("s1", "/tmp"),
        None,
    );

    dispatch_js_message(js_msg, &agent, &nats, &test_prefix()).await;

    assert_js_response_method_not_found(&nats, "acp.session.s1.agent.response.req-1");
}

#[tokio::test]
async fn dispatch_js_message_set_config_option() {
    let nats = MockNatsClient::new();
    let agent = MockAgent::new();
    let js_msg = make_js_msg(
        "acp.session.s1.agent.set_config_option",
        &SetSessionConfigOptionRequest::new("s1", "key", "val"),
        None,
    );

    dispatch_js_message(js_msg, &agent, &nats, &test_prefix()).await;

    assert_js_response_method_not_found(&nats, "acp.session.s1.agent.response.req-1");
}

#[tokio::test]
async fn dispatch_js_message_set_model() {
    let nats = MockNatsClient::new();
    let agent = MockAgent::new();
    let js_msg = make_js_msg(
        "acp.session.s1.agent.set_model",
        &SetSessionModelRequest::new("s1", "gpt-4"),
        None,
    );

    dispatch_js_message(js_msg, &agent, &nats, &test_prefix()).await;

    assert_js_response_method_not_found(&nats, "acp.session.s1.agent.response.req-1");
}

#[tokio::test]
async fn dispatch_js_message_resume_session() {
    let nats = MockNatsClient::new();
    let agent = MockAgent::new();
    let js_msg = make_js_msg(
        "acp.session.s1.agent.resume",
        &ResumeSessionRequest::new("s1", "/tmp"),
        None,
    );

    dispatch_js_message(js_msg, &agent, &nats, &test_prefix()).await;

    assert_js_response_method_not_found(&nats, "acp.session.s1.agent.response.req-1");
}

#[tokio::test]
async fn dispatch_js_message_reply_failure_acks() {
    let nats = trogon_nats::AdvancedMockNatsClient::new();
    nats.fail_next_publish();
    let agent = MockAgent::new();
    let js_msg = make_js_msg(
        "acp.session.s1.agent.load",
        &LoadSessionRequest::new("s1", "/tmp"),
        Some("_INBOX.r"),
    );

    dispatch_js_message(js_msg, &agent, &nats, &test_prefix()).await;
}

fn make_failing_js_msg<T: serde::Serialize>(subject: &str, args: &T) -> MockJsMessage {
    let method = wire_method_for_subject(subject);
    let encoded = if method == "cancel" {
        wire_encode_notification(&method, args)
    } else {
        wire_encode_request(&method, args)
    };
    let mut headers = encoded.headers;
    headers.insert(trogon_nats::REQ_ID_HEADER, "req-1");
    MockJsMessage::with_failing_signals(async_nats::Message {
        subject: subject.into(),
        reply: None,
        payload: Bytes::copy_from_slice(&encoded.body),
        headers: Some(headers),
        status: None,
        description: None,
        length: encoded.body.len(),
    })
}

fn make_failing_js_msg_raw(subject: &str, payload: &[u8]) -> MockJsMessage {
    let mut headers = async_nats::HeaderMap::new();
    headers.insert(trogon_nats::REQ_ID_HEADER, "req-1");
    MockJsMessage::with_failing_signals(async_nats::Message {
        subject: subject.into(),
        reply: None,
        payload: Bytes::copy_from_slice(payload),
        headers: Some(headers),
        status: None,
        description: None,
        length: payload.len(),
    })
}

#[tokio::test]
async fn dispatch_js_message_ack_failure_logs_warning() {
    let nats = MockNatsClient::new();
    let agent = MockAgent::new();
    let js_msg = make_failing_js_msg("acp.session.s1.agent.load", &LoadSessionRequest::new("s1", "/tmp"));
    dispatch_js_message(js_msg, &agent, &nats, &test_prefix()).await;
}

#[tokio::test]
async fn dispatch_js_message_term_failure_logs_warning() {
    let nats = MockNatsClient::new();
    let agent = MockAgent::new();
    let js_msg = make_failing_js_msg_raw("unknown.subject", b"{}");
    dispatch_js_message(js_msg, &agent, &nats, &test_prefix()).await;
}

#[tokio::test]
async fn dispatch_js_message_term_bad_payload_failure_logs_warning() {
    let nats = MockNatsClient::new();
    let agent = MockAgent::new();
    let js_msg = make_failing_js_msg_raw("acp.session.s1.agent.load", b"not json");
    dispatch_js_message(js_msg, &agent, &nats, &test_prefix()).await;
}

#[tokio::test]
async fn dispatch_js_message_no_reply_term_failure() {
    let nats = MockNatsClient::new();
    let agent = MockAgent::new();
    let payload = serialize(&LoadSessionRequest::new("s1", "/tmp"));
    // Session message without X-Req-Id → NoReplySubject → term → term fails
    let js_msg = MockJsMessage::with_failing_signals(async_nats::Message {
        subject: "acp.session.s1.agent.load".into(),
        reply: None,
        payload: Bytes::copy_from_slice(&payload),
        headers: None,
        status: None,
        description: None,
        length: payload.len(),
    });
    dispatch_js_message(js_msg, &agent, &nats, &test_prefix()).await;
}

#[tokio::test]
async fn dispatch_js_message_reply_failure_ack_failure() {
    let nats = trogon_nats::AdvancedMockNatsClient::new();
    nats.fail_next_publish();
    let agent = MockAgent::new();
    let js_msg = make_failing_js_msg("acp.session.s1.agent.load", &LoadSessionRequest::new("s1", "/tmp"));
    dispatch_js_message(js_msg, &agent, &nats, &test_prefix()).await;
}

#[tokio::test]
async fn dispatch_js_message_cancel_notification_ack_failure() {
    let nats = MockNatsClient::new();
    let agent = MockAgent::new();
    let js_msg = make_failing_js_msg("acp.session.s1.agent.cancel", &CancelNotification::new("s1"));
    dispatch_js_message(js_msg, &agent, &nats, &test_prefix()).await;
}

#[tokio::test]
async fn dispatch_js_message_cancel_notification_handler_error_ack_failure() {
    let _guard = tracing_subscriber::fmt().with_test_writer().set_default();

    let nats = MockNatsClient::new();
    let agent = MockAgent::failing_cancel();
    let payload = serialize(&CancelNotification::new("s1"));
    let js_msg = MockJsMessage::with_failing_signals(async_nats::Message {
        subject: "acp.session.s1.agent.cancel".into(),
        reply: None,
        payload: Bytes::copy_from_slice(&payload),
        headers: None,
        status: None,
        description: None,
        length: payload.len(),
    });
    dispatch_js_message(js_msg, &agent, &nats, &test_prefix()).await;
}

fn init_handler_error(_: InitializeRequest) -> std::future::Ready<agent_client_protocol::Result<InitializeResponse>> {
    std::future::ready(Err(AcpError::internal_error()))
}

#[tokio::test]
async fn handle_request_with_keepalive_completes_fast() {
    let nats = MockNatsClient::new();
    let init = InitializeRequest::new(agent_client_protocol::ProtocolVersion::V0);
    let encoded = wire_encode_request("initialize", &init);
    let msg = make_nats_message("acp.agent.initialize", &encoded.body, Some("_INBOX.1"));
    let mut msg = msg;
    msg.headers = Some(encoded.headers);
    let js_msg = make_js_msg("acp.agent.initialize", &init, Some("_INBOX.1"));

    let agent = MockAgent::new();
    let result = handle_jsonrpc_request_with_keepalive(&msg, "initialize", &nats, &js_msg, |req: InitializeRequest| {
        agent.initialize(req)
    })
    .await;
    assert!(result.is_ok());
    assert_eq!(nats.published_messages(), vec!["_INBOX.1"]);
    let response: InitializeResponse = published_response(&nats);
    assert_eq!(response.protocol_version, agent_client_protocol::ProtocolVersion::V0);
}

#[tokio::test]
async fn handle_request_with_keepalive_no_reply_subject() {
    let nats = MockNatsClient::new();
    let init = InitializeRequest::new(agent_client_protocol::ProtocolVersion::V0);
    let encoded = wire_encode_request("initialize", &init);
    let msg = make_nats_message("acp.agent.initialize", &encoded.body, None);
    let mut msg = msg;
    msg.headers = Some(encoded.headers);
    let js_msg = make_js_msg("acp.agent.initialize", &init, None);
    let result = handle_jsonrpc_request_with_keepalive(&msg, "initialize", &nats, &js_msg, init_handler_error).await;
    assert!(result.is_err());
}

#[tokio::test]
async fn handle_request_with_keepalive_bad_payload() {
    let nats = MockNatsClient::new();
    let msg = make_nats_message("acp.agent.initialize", b"not json", Some("_INBOX.1"));
    let js_msg = make_js_msg_raw("acp.agent.initialize", b"not json", Some("_INBOX.1"));
    let result = handle_jsonrpc_request_with_keepalive(&msg, "initialize", &nats, &js_msg, init_handler_error).await;
    assert!(result.is_err());
}

#[tokio::test]
async fn handle_request_with_keepalive_handler_returns_error() {
    let nats = MockNatsClient::new();
    let init = InitializeRequest::new(agent_client_protocol::ProtocolVersion::V0);
    let encoded = wire_encode_request("initialize", &init);
    let msg = make_nats_message("acp.agent.initialize", &encoded.body, Some("_INBOX.1"));
    let mut msg = msg;
    msg.headers = Some(encoded.headers);
    let js_msg = make_js_msg("acp.agent.initialize", &init, Some("_INBOX.1"));
    let result = handle_jsonrpc_request_with_keepalive(&msg, "initialize", &nats, &js_msg, init_handler_error).await;
    assert!(result.is_ok());
    assert_eq!(nats.published_messages(), vec!["_INBOX.1"]);
    let error = published_wire_error(&nats);
    assert_eq!(error.code, ErrorCode::InternalError);
}

#[tokio::test(start_paused = true)]
async fn handle_request_with_keepalive_progress_ack_failure() {
    let _guard = tracing_subscriber::fmt().with_test_writer().set_default();

    let nats = MockNatsClient::new();
    let init = InitializeRequest::new(agent_client_protocol::ProtocolVersion::V0);
    let encoded = wire_encode_request("initialize", &init);
    let msg = make_nats_message("acp.agent.initialize", &encoded.body, Some("_INBOX.1"));
    let mut msg = msg;
    msg.headers = Some(encoded.headers.clone());

    let mut headers = encoded.headers;
    headers.insert(trogon_nats::REQ_ID_HEADER, "req-1");
    let js_msg = MockJsMessage::with_failing_signals(async_nats::Message {
        subject: "acp.agent.initialize".into(),
        reply: Some("_INBOX.1".into()),
        payload: Bytes::copy_from_slice(&encoded.body),
        headers: Some(headers),
        status: None,
        description: None,
        length: encoded.body.len(),
    });

    let agent = MockAgent::new();
    let result =
        handle_jsonrpc_request_with_keepalive(&msg, "initialize", &nats, &js_msg, |req: InitializeRequest| async {
            tokio::time::sleep(Duration::from_secs(20)).await;
            agent.initialize(req).await
        })
        .await;
    assert!(result.is_ok());
}

#[tokio::test]
async fn handle_request_with_keepalive_handler_error() {
    let nats = MockNatsClient::new();
    let auth = AuthenticateRequest::new("basic");
    let encoded = wire_encode_request("authenticate", &auth);
    let msg = make_nats_message("acp.agent.authenticate", &encoded.body, Some("_INBOX.1"));
    let mut msg = msg;
    msg.headers = Some(encoded.headers);
    let js_msg = make_js_msg("acp.agent.authenticate", &auth, Some("_INBOX.1"));

    let agent = MockAgent::new();
    let result =
        handle_jsonrpc_request_with_keepalive(&msg, "authenticate", &nats, &js_msg, |req: AuthenticateRequest| {
            agent.authenticate(req)
        })
        .await;
    assert!(result.is_ok());
    assert_eq!(nats.published_messages(), vec!["_INBOX.1"]);
    let error = published_wire_error(&nats);
    assert_eq!(error.code, ErrorCode::MethodNotFound);
}
