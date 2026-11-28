use super::*;
use acp_nats::nats::{ParsedAgentSubject, parse_agent_subject};
use agent_client_protocol::schema::ProtocolVersion;
use agent_client_protocol::v1::schema::{
    ArchiveSessionResponse, AuthState, AuthStatusResponse, AuthenticateResponse, InitializeRequest, InitializeResponse,
    LogoutResponse, NewSessionResponse, PromptResponse, SessionStatus, SessionStatusResponse, SetSessionTitleResponse,
    StopReason, UnarchiveSessionResponse,
};
use agent_client_protocol::{Error as AcpError, ErrorCode};
use std::sync::Mutex;
use tracing_subscriber::util::SubscriberInitExt;
use trogon_nats::MockNatsClient;
use trogon_nats::jetstream::{MockJetStreamConsumer, MockJetStreamConsumerFactory};

struct MockAgent {
    initialized: Mutex<bool>,
    cancelled: Mutex<Vec<String>>,
    fail_cancel: bool,
    schema_calls: Mutex<Vec<(String, serde_json::Value)>>,
    received_new_session: Mutex<Option<NewSessionRequest>>,
}

impl MockAgent {
    fn new() -> Self {
        Self {
            initialized: Mutex::new(false),
            cancelled: Mutex::new(Vec::new()),
            fail_cancel: false,
            schema_calls: Mutex::new(Vec::new()),
            received_new_session: Mutex::new(None),
        }
    }

    fn failing_cancel() -> Self {
        Self {
            initialized: Mutex::new(false),
            cancelled: Mutex::new(Vec::new()),
            fail_cancel: true,
            schema_calls: Mutex::new(Vec::new()),
            received_new_session: Mutex::new(None),
        }
    }
}

#[async_trait::async_trait]
impl AgentHandler for MockAgent {
    async fn ext_notification(&self, args: ExtNotification) -> agent_client_protocol::Result<()> {
        self.schema_calls
            .lock()
            .unwrap()
            .push(("ext_notification".into(), serde_json::to_value(args).unwrap()));
        Ok(())
    }

    async fn auth_status(&self, args: AuthStatusRequest) -> agent_client_protocol::Result<AuthStatusResponse> {
        self.schema_calls
            .lock()
            .unwrap()
            .push(("auth/status".into(), serde_json::to_value(args).unwrap()));
        Ok(AuthStatusResponse::new(AuthState::Unauthenticated))
    }

    async fn archive_session(
        &self,
        args: ArchiveSessionRequest,
    ) -> agent_client_protocol::Result<ArchiveSessionResponse> {
        self.schema_calls
            .lock()
            .unwrap()
            .push(("session/archive".into(), serde_json::to_value(args).unwrap()));
        Ok(ArchiveSessionResponse::new())
    }

    async fn unarchive_session(
        &self,
        args: UnarchiveSessionRequest,
    ) -> agent_client_protocol::Result<UnarchiveSessionResponse> {
        self.schema_calls
            .lock()
            .unwrap()
            .push(("session/unarchive".into(), serde_json::to_value(args).unwrap()));
        Ok(UnarchiveSessionResponse::new())
    }

    async fn session_status(&self, args: SessionStatusRequest) -> agent_client_protocol::Result<SessionStatusResponse> {
        self.schema_calls
            .lock()
            .unwrap()
            .push(("session/status".into(), serde_json::to_value(args).unwrap()));
        Ok(SessionStatusResponse::new(SessionStatus::Saved))
    }

    async fn set_session_title(
        &self,
        args: SetSessionTitleRequest,
    ) -> agent_client_protocol::Result<SetSessionTitleResponse> {
        let title = args.title.clone();
        self.schema_calls
            .lock()
            .unwrap()
            .push(("session/set_title".into(), serde_json::to_value(args).unwrap()));
        Ok(SetSessionTitleResponse::new(title))
    }

    async fn initialize(&self, _args: InitializeRequest) -> agent_client_protocol::Result<InitializeResponse> {
        *self.initialized.lock().unwrap() = true;
        Ok(InitializeResponse::new(ProtocolVersion::V0))
    }

    async fn authenticate(&self, _args: AuthenticateRequest) -> agent_client_protocol::Result<AuthenticateResponse> {
        Err(AcpError::method_not_found())
    }

    async fn logout(&self, _args: LogoutRequest) -> agent_client_protocol::Result<LogoutResponse> {
        Ok(LogoutResponse::new())
    }

    async fn new_session(&self, args: NewSessionRequest) -> agent_client_protocol::Result<NewSessionResponse> {
        *self.received_new_session.lock().unwrap() = Some(args);
        Ok(NewSessionResponse::new("sess-1"))
    }

    async fn prompt(&self, _args: PromptRequest) -> agent_client_protocol::Result<PromptResponse> {
        Ok(PromptResponse::new(StopReason::EndTurn))
    }

    async fn cancel(&self, args: CancelNotification) -> agent_client_protocol::Result<()> {
        if self.fail_cancel {
            return Err(AcpError::internal_error());
        }
        self.cancelled.lock().unwrap().push(args.session_id.to_string());
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
        ParsedAgentSubject::Global(method) => method.protocol_method(),
        ParsedAgentSubject::Session { method, .. } => method.protocol_method().to_string(),
    }
}

fn wire_encode_request<T: serde::Serialize>(method: &str, args: &T) -> jsonrpc_nats::Encoded {
    acp_nats::wire::encode_request(method, jsonrpc_nats::RequestId::String("req-1".to_string()), args).unwrap()
}

fn wire_encode_notification<T: serde::Serialize>(method: &str, args: &T) -> jsonrpc_nats::Encoded {
    acp_nats::wire::encode_notification(method, args).unwrap()
}

async fn dispatch<T: serde::Serialize>(subject: &str, args: &T, reply: Option<&str>) -> (MockNatsClient, MockAgent) {
    let nats = MockNatsClient::new();
    let agent = MockAgent::new();
    let method = wire_method_for_subject(subject);
    let encoded = if reply.is_none() && method == "session/cancel" {
        wire_encode_notification(&method, args)
    } else {
        wire_encode_request(&method, args)
    };
    let mut msg = make_nats_message(subject, &encoded.body, reply);
    msg.headers = Some(encoded.headers);
    dispatch_message(msg, &agent, &AdmitAll, &nats).await;
    (nats, agent)
}

async fn dispatch_raw(subject: &str, payload: &[u8], reply: Option<&str>) -> (MockNatsClient, MockAgent) {
    let nats = MockNatsClient::new();
    let agent = MockAgent::new();
    let msg = make_nats_message(subject, payload, reply);
    dispatch_message(msg, &agent, &AdmitAll, &nats).await;
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
    InitializeRequest::new(ProtocolVersion::V0)
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
    let (nats, agent) = dispatch("acp.v1.global.agent.initialize", &init_request(), Some("_INBOX.1")).await;

    assert!(*agent.initialized.lock().unwrap());
    let response: InitializeResponse = published_response(&nats);
    assert_eq!(response.protocol_version, ProtocolVersion::V0);
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
    dispatch_message(msg, &agent, &AdmitAll, &nats).await;
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
        "acp.v1.global.agent.authenticate",
        &AuthenticateRequest::new("basic"),
        Some("_INBOX.2"),
    )
    .await;

    let error = published_wire_error(&nats);
    assert_eq!(error.code, ErrorCode::METHOD_NOT_FOUND);
}

#[tokio::test]
async fn dispatch_logout_publishes_response() {
    let (nats, _) = dispatch("acp.v1.global.agent.logout", &LogoutRequest::new(), Some("_INBOX.r")).await;
    assert_eq!(nats.published_messages(), vec!["_INBOX.r"]);
    let _: LogoutResponse = published_response(&nats);
}

#[tokio::test]
async fn dispatch_cancel_is_notification_no_reply_published() {
    let (nats, agent) = dispatch("acp.v1.session.s1.agent.cancel", &CancelNotification::new("s1"), None).await;

    assert_eq!(agent.cancelled.lock().unwrap().as_slice(), ["s1"]);
    assert!(nats.published_messages().is_empty());
}

#[tokio::test]
async fn dispatch_invalid_payload_publishes_error_reply() {
    let (nats, agent) = dispatch_raw("acp.v1.global.agent.initialize", b"not json", Some("_INBOX.err")).await;

    assert!(!*agent.initialized.lock().unwrap());
    let error = published_wire_error(&nats);
    assert_eq!(error.code, ErrorCode::INVALID_PARAMS);
}

#[tokio::test]
async fn dispatch_request_without_reply_subject_does_not_publish() {
    let (nats, _) = dispatch("acp.v1.global.agent.initialize", &init_request(), None).await;
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
        "acp.v1.session.s1.agent.prompt",
        &PromptRequest::new("s1", vec![]),
        Some("_INBOX.3"),
    )
    .await;

    let response: PromptResponse = published_response(&nats);
    assert_eq!(response.stop_reason, StopReason::EndTurn);
}

#[tokio::test]
async fn dispatch_publishes_to_correct_reply_subject() {
    let (nats, _) = dispatch(
        "acp.v1.global.agent.initialize",
        &init_request(),
        Some("_INBOX.specific"),
    )
    .await;
    assert_eq!(nats.published_messages(), vec!["_INBOX.specific"]);
    let response: InitializeResponse = published_response(&nats);
    assert_eq!(response.protocol_version, ProtocolVersion::V0);
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
        "acp.v1.global.agent.ext.my_tool",
        &agent_client_protocol::v1::schema::ExtRequest::new("my_tool", raw_value("{}")),
        Some("_INBOX.ext"),
    )
    .await;
    assert_eq!(nats.published_messages(), vec!["_INBOX.ext"]);
    let error = published_wire_error(&nats);
    assert_eq!(error.code, ErrorCode::METHOD_NOT_FOUND);
}

#[tokio::test]
async fn dispatch_ext_without_reply_calls_ext_notification() {
    let (nats, _) = dispatch(
        "acp.v1.global.agent.ext.my_tool",
        &agent_client_protocol::v1::schema::ExtNotification::new("my_tool", raw_value("{}")),
        None,
    )
    .await;
    assert!(nats.published_messages().is_empty());
}

#[tokio::test]
async fn the_bridges_session_ready_signal_never_reaches_the_agent() {
    let ready = acp_nats::nats::ExtSessionReady::new(agent_client_protocol::v1::schema::SessionId::from("s1"));
    let payload = serde_json::to_vec(&ready).unwrap();

    let (nats, agent) = dispatch_raw("acp.v1.session.s1.agent.ext.ready", &payload, None).await;

    assert!(nats.published_messages().is_empty());
    assert!(agent.schema_calls.lock().unwrap().is_empty());
}

#[tokio::test]
async fn other_session_extension_notifications_still_reach_the_agent() {
    let (_, agent) = dispatch_raw("acp.v1.session.s1.agent.ext.my_tool", b"{}", None).await;
    assert_eq!(agent.schema_calls.lock().unwrap().len(), 1);

    let (_, agent) = dispatch_raw("acp.v1.global.agent.ext.ready", b"{}", None).await;
    assert_eq!(
        agent.schema_calls.lock().unwrap().len(),
        1,
        "only the session-scoped signal is the bridge's"
    );
}

async fn assert_dispatch_method_not_found<T: serde::Serialize>(subject: &str, args: &T) {
    let (nats, _) = dispatch(subject, args, Some("_INBOX.r")).await;
    assert_eq!(nats.published_messages(), vec!["_INBOX.r"]);
    let error = published_wire_error(&nats);
    assert_eq!(error.code, ErrorCode::METHOD_NOT_FOUND);
}

#[tokio::test]
async fn dispatch_new_session_publishes_response() {
    let (nats, _) = dispatch(
        "acp.v1.global.agent.session.new",
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
    let (nats, agent) = dispatch("acp.v1.global.agent.session.new", &request, Some("_INBOX.r")).await;

    assert_eq!(nats.published_messages(), vec!["_INBOX.r"]);
    let received = agent.received_new_session.lock().unwrap();
    assert_eq!(
        received.as_ref().unwrap().additional_directories,
        request.additional_directories
    );
}

#[tokio::test]
async fn dispatch_session_load_publishes_response() {
    assert_dispatch_method_not_found("acp.v1.session.s1.agent.load", &LoadSessionRequest::new("s1", "/tmp")).await;
}

#[tokio::test]
async fn dispatch_session_load_additional_directories_survive_wire_decode() {
    let request =
        LoadSessionRequest::new("s1", "/tmp").additional_directories(vec![std::path::PathBuf::from("/tmp/extra")]);
    let method = wire_method_for_subject("acp.v1.session.s1.agent.load");
    let encoded = wire_encode_request(&method, &request);
    let decoded: LoadSessionRequest =
        acp_nats::wire::decode_request_params(&method, &encoded.headers, &encoded.body).unwrap();

    assert_eq!(decoded.additional_directories, request.additional_directories);
}

#[tokio::test]
async fn dispatch_list_sessions_publishes_response() {
    assert_dispatch_method_not_found("acp.v1.global.agent.session.list", &ListSessionsRequest::new()).await;
}

#[tokio::test]
async fn dispatch_providers_list_publishes_response() {
    assert_dispatch_method_not_found("acp.v1.global.agent.providers.list", &ListProvidersRequest::new()).await;
}

#[tokio::test]
async fn dispatch_providers_set_publishes_response() {
    let request = SetProviderRequest::new(
        "anthropic",
        agent_client_protocol::v1::schema::LlmProtocol::Anthropic,
        "https://api.anthropic.com",
    );
    assert_dispatch_method_not_found("acp.v1.global.agent.providers.set", &request).await;
}

#[tokio::test]
async fn dispatch_providers_disable_publishes_response() {
    assert_dispatch_method_not_found(
        "acp.v1.global.agent.providers.disable",
        &DisableProviderRequest::new("anthropic"),
    )
    .await;
}

#[tokio::test]
async fn dispatch_set_session_mode_publishes_response() {
    assert_dispatch_method_not_found(
        "acp.v1.session.s1.agent.set_mode",
        &SetSessionModeRequest::new("s1", "code"),
    )
    .await;
}

#[tokio::test]
async fn dispatch_set_session_config_option_publishes_response() {
    assert_dispatch_method_not_found(
        "acp.v1.session.s1.agent.set_config_option",
        &SetSessionConfigOptionRequest::new("s1", "key", "val"),
    )
    .await;
}

#[tokio::test]
async fn dispatch_fork_session_publishes_response() {
    assert_dispatch_method_not_found("acp.v1.session.s1.agent.fork", &ForkSessionRequest::new("s1", "/tmp")).await;
}

#[tokio::test]
async fn dispatch_resume_session_publishes_response() {
    assert_dispatch_method_not_found(
        "acp.v1.session.s1.agent.resume",
        &ResumeSessionRequest::new("s1", "/tmp"),
    )
    .await;
}

#[tokio::test]
async fn dispatch_close_session_publishes_response() {
    assert_dispatch_method_not_found("acp.v1.session.s1.agent.close", &CloseSessionRequest::new("s1")).await;
}

#[tokio::test]
async fn dispatch_delete_session_publishes_response() {
    assert_dispatch_method_not_found("acp.v1.session.s1.agent.delete", &DeleteSessionRequest::new("s1")).await;
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
    headers.insert(jsonrpc_nats::HEADER_ID, "\"req-1\"");
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
    let encoded = if method == "session/cancel" {
        wire_encode_notification(&method, args)
    } else {
        wire_encode_request(&method, args)
    };
    let headers = encoded.headers;
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
            let _ = serve_global(
                Rc::new(agent),
                Rc::new(AdmitAll),
                nats.clone(),
                &test_prefix_custom("myprefix"),
                |fut| {
                    tokio::task::spawn_local(fut);
                },
            )
            .await;

            let subjects = nats.subscribed_to();
            assert_eq!(subjects.len(), 2);
            assert!(subjects.contains(&"myprefix.v1.global.agent.>".to_string()));
            assert!(subjects.contains(&"myprefix.v1.session.*.agent.ext.>".to_string()));
            assert!(!subjects.contains(&"myprefix.v1.session.*.agent.>".to_string()));
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
            let encoded = wire_encode_request("initialize", &InitializeRequest::new(ProtocolVersion::V0));
            let msg = Message {
                subject: "acp.v1.global.agent.initialize".into(),
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

            let _ = serve_global(Rc::new(agent), Rc::new(AdmitAll), nats.clone(), &test_prefix(), |fut| {
                tokio::task::spawn_local(fut);
            })
            .await;

            tokio::task::yield_now().await;
            tokio::task::yield_now().await;

            assert_eq!(nats.published_messages(), vec!["_INBOX.serve"]);
            let response: InitializeResponse = published_response(&nats);
            assert_eq!(response.protocol_version, ProtocolVersion::V0);
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
        "acp.v1.session.s1.agent.load",
        &LoadSessionRequest::new("s1", "/tmp"),
        None,
    );
    tx.unbounded_send(Ok(js_msg)).unwrap();
    drop(tx);

    let local = tokio::task::LocalSet::new();
    local
        .run_until(async {
            let _ = serve_js(
                Rc::new(agent),
                Rc::new(AdmitAll),
                nats.clone(),
                factory,
                &test_prefix(),
                |fut| {
                    tokio::task::spawn_local(fut);
                },
                std::future::pending(),
            )
            .await;

            tokio::task::yield_now().await;
            tokio::task::yield_now().await;

            assert_js_response_method_not_found(&nats, "acp.v1.session.s1.agent.response");
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
            let _ = serve_js(
                Rc::new(agent),
                Rc::new(AdmitAll),
                nats.clone(),
                factory,
                &test_prefix(),
                |fut| {
                    tokio::task::spawn_local(fut);
                },
                std::future::pending(),
            )
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
            let result = serve_js(
                Rc::new(agent),
                Rc::new(AdmitAll),
                nats.clone(),
                factory,
                &test_prefix(),
                |fut| {
                    tokio::task::spawn_local(fut);
                },
                std::future::pending(),
            )
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

    dispatch_js_message(&js_msg, &agent, &AdmitAll, &nats, &test_prefix()).await;

    assert!(nats.published_messages().is_empty());
}

#[tokio::test]
async fn dispatch_js_message_bad_payload_terms() {
    let nats = MockNatsClient::new();
    let agent = MockAgent::new();
    let js_msg = make_js_msg_raw("acp.v1.session.s1.agent.load", b"not json", None);

    dispatch_js_message(&js_msg, &agent, &AdmitAll, &nats, &test_prefix()).await;

    assert_eq!(nats.published_messages(), vec!["acp.v1.session.s1.agent.response"]);
    let error = published_wire_error(&nats);
    assert_eq!(error.code, ErrorCode::INVALID_PARAMS);
}

#[tokio::test]
async fn dispatch_js_message_missing_reply_terms() {
    let nats = MockNatsClient::new();
    let agent = MockAgent::new();
    let js_msg = make_js_msg("acp.v1.global.agent.initialize", &init_request(), None);

    dispatch_js_message(&js_msg, &agent, &AdmitAll, &nats, &test_prefix()).await;

    assert!(nats.published_messages().is_empty());
}

#[tokio::test]
async fn dispatch_js_message_missing_id_header() {
    let nats = MockNatsClient::new();
    let agent = MockAgent::new();
    let payload = serialize(&LoadSessionRequest::new("s1", "/tmp"));
    // Create message without Jsonrpc-Id header
    let js_msg = MockJsMessage::new(async_nats::Message {
        subject: "acp.v1.session.s1.agent.load".into(),
        reply: None,
        payload: Bytes::copy_from_slice(&payload),
        headers: None,
        status: None,
        description: None,
        length: payload.len(),
    });
    dispatch_js_message(&js_msg, &agent, &AdmitAll, &nats, &test_prefix()).await;
    // No reply published because no req_id → no reply subject
    assert!(nats.published_messages().is_empty());
}

#[tokio::test]
async fn dispatch_js_message_global_method_returns_early() {
    let nats = MockNatsClient::new();
    let agent = MockAgent::new();
    let js_msg = make_js_msg(
        "acp.v1.global.agent.initialize",
        &InitializeRequest::new(ProtocolVersion::V0),
        Some("_INBOX.1"),
    );
    dispatch_js_message(&js_msg, &agent, &AdmitAll, &nats, &test_prefix()).await;
    // Global methods return early — no dispatch, no reply
    assert!(nats.published_messages().is_empty());
}

#[tokio::test]
async fn dispatch_js_message_global_method_ack_failure() {
    let nats = MockNatsClient::new();
    let agent = MockAgent::new();
    let js_msg = make_failing_js_msg(
        "acp.v1.global.agent.initialize",
        &InitializeRequest::new(ProtocolVersion::V0),
    );
    dispatch_js_message(&js_msg, &agent, &AdmitAll, &nats, &test_prefix()).await;
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
    let payload = serialize(&agent_client_protocol::v1::schema::ExtNotification::new("my_tool", raw));
    // No Jsonrpc-Id → ext notification path (reply_subject is None → msg.reply is None)
    let js_msg = make_js_msg_no_headers("acp.v1.session.s1.agent.ext.my_tool", &payload);
    dispatch_js_message(&js_msg, &agent, &AdmitAll, &nats, &test_prefix()).await;
}

#[tokio::test]
async fn dispatch_js_message_ext_notification_handler_error_ack_failure() {
    let _guard = tracing_subscriber::fmt().with_test_writer().set_default();

    let nats = MockNatsClient::new();
    let agent = MockAgent::new();
    let raw = std::sync::Arc::from(serde_json::value::RawValue::from_string("{}".to_string()).unwrap());
    let payload = serialize(&agent_client_protocol::v1::schema::ExtNotification::new("my_tool", raw));
    let js_msg = MockJsMessage::with_failing_signals(async_nats::Message {
        subject: "acp.v1.session.s1.agent.ext.my_tool".into(),
        reply: None,
        payload: Bytes::copy_from_slice(&payload),
        headers: None,
        status: None,
        description: None,
        length: payload.len(),
    });
    dispatch_js_message(&js_msg, &agent, &AdmitAll, &nats, &test_prefix()).await;
}

#[tokio::test]
async fn dispatch_js_message_global_ext_no_session_id() {
    let nats = MockNatsClient::new();
    let agent = MockAgent::new();
    let raw = std::sync::Arc::from(serde_json::value::RawValue::from_string("{}".to_string()).unwrap());
    let payload = serialize(&agent_client_protocol::v1::schema::ExtNotification::new("my_tool", raw));
    let js_msg = make_js_msg_no_headers("acp.v1.global.agent.ext.my_tool", &payload);
    dispatch_js_message(&js_msg, &agent, &AdmitAll, &nats, &test_prefix()).await;
}

#[tokio::test]
async fn dispatch_js_message_prompt_uses_response_subject() {
    let nats = MockNatsClient::new();
    let agent = MockAgent::new();
    let js_msg = make_js_msg(
        "acp.v1.session.s1.agent.prompt",
        &PromptRequest::new("s1", vec![]),
        None,
    );

    dispatch_js_message(&js_msg, &agent, &AdmitAll, &nats, &test_prefix()).await;

    assert_eq!(nats.published_messages(), vec!["acp.v1.session.s1.agent.response"]);
    let response: PromptResponse = published_response(&nats);
    assert_eq!(response.stop_reason, StopReason::EndTurn);
}

#[tokio::test]
async fn dispatch_js_message_non_prompt_session_uses_response_subject() {
    let nats = MockNatsClient::new();
    let agent = MockAgent::new();
    let js_msg = make_js_msg(
        "acp.v1.session.s1.agent.load",
        &LoadSessionRequest::new("s1", "/tmp"),
        None,
    );

    dispatch_js_message(&js_msg, &agent, &AdmitAll, &nats, &test_prefix()).await;

    assert_js_response_method_not_found(&nats, "acp.v1.session.s1.agent.response");
}

#[tokio::test]
async fn dispatch_error_logs_warning_with_subscriber() {
    let _guard = tracing_subscriber::fmt().with_test_writer().set_default();

    let nats = MockNatsClient::new();
    let agent = MockAgent::new();
    let payload = serialize(&InitializeRequest::new(ProtocolVersion::V0));
    let msg = make_nats_message("acp.v1.global.agent.initialize", &payload, None);

    dispatch_message(msg, &agent, &AdmitAll, &nats).await;
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
            let encoded = wire_encode_request("initialize", &InitializeRequest::new(ProtocolVersion::V0));
            let msg = Message {
                subject: "acp.v1.global.agent.initialize".into(),
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

            let result = serve(agent, AdmitAll, nats.clone(), &test_prefix(), |fut| {
                tokio::task::spawn_local(fut);
            })
            .await;

            assert!(result.is_ok());

            tokio::task::yield_now().await;
            tokio::task::yield_now().await;

            assert_eq!(nats.published_messages(), vec!["_INBOX.serve"]);
            let response: InitializeResponse = published_response(&nats);
            assert_eq!(response.protocol_version, ProtocolVersion::V0);
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
            let result = serve(agent, AdmitAll, nats, &test_prefix(), |fut| {
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
            let _ = serve(agent, AdmitAll, nats.clone(), &test_prefix_custom("myprefix"), |fut| {
                tokio::task::spawn_local(fut);
            })
            .await;

            let subjects = nats.subscribed_to();
            assert!(subjects.contains(&"myprefix.v1.global.agent.>".to_string()));
            assert!(subjects.contains(&"myprefix.v1.session.*.agent.>".to_string()));
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
    let js_msg = make_js_msg("acp.v1.session.s1.agent.cancel", &CancelNotification::new("s1"), None);

    dispatch_js_message(&js_msg, &agent, &AdmitAll, &nats, &test_prefix()).await;

    assert_eq!(agent.cancelled.lock().unwrap().as_slice(), ["s1"]);
}

fn assert_js_response_method_not_found(nats: &MockNatsClient, expected_subject: &str) {
    assert_eq!(nats.published_messages(), vec![expected_subject]);
    let error = published_wire_error(nats);
    assert_eq!(error.code, ErrorCode::METHOD_NOT_FOUND);
}

#[tokio::test]
async fn dispatch_js_message_set_mode() {
    let nats = MockNatsClient::new();
    let agent = MockAgent::new();
    let js_msg = make_js_msg(
        "acp.v1.session.s1.agent.set_mode",
        &SetSessionModeRequest::new("s1", "code"),
        None,
    );

    dispatch_js_message(&js_msg, &agent, &AdmitAll, &nats, &test_prefix()).await;

    assert_js_response_method_not_found(&nats, "acp.v1.session.s1.agent.response");
}

#[tokio::test]
async fn dispatch_js_message_close_session() {
    let nats = MockNatsClient::new();
    let agent = MockAgent::new();
    let js_msg = make_js_msg("acp.v1.session.s1.agent.close", &CloseSessionRequest::new("s1"), None);

    dispatch_js_message(&js_msg, &agent, &AdmitAll, &nats, &test_prefix()).await;

    assert_js_response_method_not_found(&nats, "acp.v1.session.s1.agent.response");
}

#[tokio::test]
async fn dispatch_js_message_delete_session() {
    let nats = MockNatsClient::new();
    let agent = MockAgent::new();
    let js_msg = make_js_msg("acp.v1.session.s1.agent.delete", &DeleteSessionRequest::new("s1"), None);

    dispatch_js_message(&js_msg, &agent, &AdmitAll, &nats, &test_prefix()).await;

    assert_js_response_method_not_found(&nats, "acp.v1.session.s1.agent.response");
}

#[tokio::test]
async fn dispatch_js_message_fork_session() {
    let nats = MockNatsClient::new();
    let agent = MockAgent::new();
    let js_msg = make_js_msg(
        "acp.v1.session.s1.agent.fork",
        &ForkSessionRequest::new("s1", "/tmp"),
        None,
    );

    dispatch_js_message(&js_msg, &agent, &AdmitAll, &nats, &test_prefix()).await;

    assert_js_response_method_not_found(&nats, "acp.v1.session.s1.agent.response");
}

#[tokio::test]
async fn dispatch_js_message_set_config_option() {
    let nats = MockNatsClient::new();
    let agent = MockAgent::new();
    let js_msg = make_js_msg(
        "acp.v1.session.s1.agent.set_config_option",
        &SetSessionConfigOptionRequest::new("s1", "key", "val"),
        None,
    );

    dispatch_js_message(&js_msg, &agent, &AdmitAll, &nats, &test_prefix()).await;

    assert_js_response_method_not_found(&nats, "acp.v1.session.s1.agent.response");
}

#[tokio::test]
async fn dispatch_js_message_resume_session() {
    let nats = MockNatsClient::new();
    let agent = MockAgent::new();
    let js_msg = make_js_msg(
        "acp.v1.session.s1.agent.resume",
        &ResumeSessionRequest::new("s1", "/tmp"),
        None,
    );

    dispatch_js_message(&js_msg, &agent, &AdmitAll, &nats, &test_prefix()).await;

    assert_js_response_method_not_found(&nats, "acp.v1.session.s1.agent.response");
}

#[tokio::test]
async fn dispatch_js_message_reply_failure_acks() {
    let nats = trogon_nats::AdvancedMockNatsClient::new();
    nats.fail_next_publish();
    let agent = MockAgent::new();
    let js_msg = make_js_msg(
        "acp.v1.session.s1.agent.load",
        &LoadSessionRequest::new("s1", "/tmp"),
        Some("_INBOX.r"),
    );

    dispatch_js_message(&js_msg, &agent, &AdmitAll, &nats, &test_prefix()).await;
}

fn make_failing_js_msg<T: serde::Serialize>(subject: &str, args: &T) -> MockJsMessage {
    let method = wire_method_for_subject(subject);
    let encoded = if method == "session/cancel" {
        wire_encode_notification(&method, args)
    } else {
        wire_encode_request(&method, args)
    };
    let headers = encoded.headers;
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
    headers.insert(jsonrpc_nats::HEADER_ID, "\"req-1\"");
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
    let js_msg = make_failing_js_msg("acp.v1.session.s1.agent.load", &LoadSessionRequest::new("s1", "/tmp"));
    dispatch_js_message(&js_msg, &agent, &AdmitAll, &nats, &test_prefix()).await;
}

#[tokio::test]
async fn dispatch_js_message_term_failure_logs_warning() {
    let nats = MockNatsClient::new();
    let agent = MockAgent::new();
    let js_msg = make_failing_js_msg_raw("unknown.subject", b"{}");
    dispatch_js_message(&js_msg, &agent, &AdmitAll, &nats, &test_prefix()).await;
}

#[tokio::test]
async fn dispatch_js_message_term_bad_payload_failure_logs_warning() {
    let nats = MockNatsClient::new();
    let agent = MockAgent::new();
    let js_msg = make_failing_js_msg_raw("acp.v1.session.s1.agent.load", b"not json");
    dispatch_js_message(&js_msg, &agent, &AdmitAll, &nats, &test_prefix()).await;
}

#[tokio::test]
async fn dispatch_js_message_no_reply_term_failure() {
    let nats = MockNatsClient::new();
    let agent = MockAgent::new();
    let payload = serialize(&LoadSessionRequest::new("s1", "/tmp"));
    // Session message without Jsonrpc-Id → NoReplySubject → term → term fails
    let js_msg = MockJsMessage::with_failing_signals(async_nats::Message {
        subject: "acp.v1.session.s1.agent.load".into(),
        reply: None,
        payload: Bytes::copy_from_slice(&payload),
        headers: None,
        status: None,
        description: None,
        length: payload.len(),
    });
    dispatch_js_message(&js_msg, &agent, &AdmitAll, &nats, &test_prefix()).await;
}

#[tokio::test]
async fn dispatch_js_message_reply_failure_ack_failure() {
    let nats = trogon_nats::AdvancedMockNatsClient::new();
    nats.fail_next_publish();
    let agent = MockAgent::new();
    let js_msg = make_failing_js_msg("acp.v1.session.s1.agent.load", &LoadSessionRequest::new("s1", "/tmp"));
    dispatch_js_message(&js_msg, &agent, &AdmitAll, &nats, &test_prefix()).await;
}

#[tokio::test]
async fn dispatch_js_message_cancel_notification_ack_failure() {
    let nats = MockNatsClient::new();
    let agent = MockAgent::new();
    let js_msg = make_failing_js_msg("acp.v1.session.s1.agent.cancel", &CancelNotification::new("s1"));
    dispatch_js_message(&js_msg, &agent, &AdmitAll, &nats, &test_prefix()).await;
}

#[tokio::test]
async fn dispatch_js_message_cancel_notification_handler_error_ack_failure() {
    let _guard = tracing_subscriber::fmt().with_test_writer().set_default();

    let nats = MockNatsClient::new();
    let agent = MockAgent::failing_cancel();
    let encoded = wire_encode_notification("session/cancel", &CancelNotification::new("s1"));
    let js_msg = MockJsMessage::with_failing_signals(async_nats::Message {
        subject: "acp.v1.session.s1.agent.cancel".into(),
        reply: None,
        payload: Bytes::copy_from_slice(&encoded.body),
        headers: Some(encoded.headers),
        status: None,
        description: None,
        length: encoded.body.len(),
    });
    dispatch_js_message(&js_msg, &agent, &AdmitAll, &nats, &test_prefix()).await;
    assert!(
        agent.cancelled.lock().unwrap().is_empty(),
        "the failing handler must not have recorded the cancel"
    );
}

fn init_handler_error(_: InitializeRequest) -> std::future::Ready<agent_client_protocol::Result<InitializeResponse>> {
    std::future::ready(Err(AcpError::internal_error()))
}

#[tokio::test]
async fn handle_request_with_keepalive_completes_fast() {
    let nats = MockNatsClient::new();
    let init = InitializeRequest::new(ProtocolVersion::V0);
    let encoded = wire_encode_request("initialize", &init);
    let msg = make_nats_message("acp.v1.global.agent.initialize", &encoded.body, Some("_INBOX.1"));
    let mut msg = msg;
    msg.headers = Some(encoded.headers);
    let js_msg = make_js_msg("acp.v1.global.agent.initialize", &init, Some("_INBOX.1"));

    let agent = MockAgent::new();
    let result = handle_jsonrpc_request_with_keepalive(&msg, "initialize", &nats, &js_msg, |req: InitializeRequest| {
        agent.initialize(req)
    })
    .await;
    assert!(result.is_ok());
    assert_eq!(nats.published_messages(), vec!["_INBOX.1"]);
    let response: InitializeResponse = published_response(&nats);
    assert_eq!(response.protocol_version, ProtocolVersion::V0);
}

#[tokio::test]
async fn handle_request_with_keepalive_no_reply_subject() {
    let nats = MockNatsClient::new();
    let init = InitializeRequest::new(ProtocolVersion::V0);
    let encoded = wire_encode_request("initialize", &init);
    let msg = make_nats_message("acp.v1.global.agent.initialize", &encoded.body, None);
    let mut msg = msg;
    msg.headers = Some(encoded.headers);
    let js_msg = make_js_msg("acp.v1.global.agent.initialize", &init, None);
    let result = handle_jsonrpc_request_with_keepalive(&msg, "initialize", &nats, &js_msg, init_handler_error).await;
    assert!(result.is_err());
}

#[tokio::test]
async fn handle_request_with_keepalive_bad_payload() {
    let nats = MockNatsClient::new();
    let msg = make_nats_message("acp.v1.global.agent.initialize", b"not json", Some("_INBOX.1"));
    let js_msg = make_js_msg_raw("acp.v1.global.agent.initialize", b"not json", Some("_INBOX.1"));
    let result = handle_jsonrpc_request_with_keepalive(&msg, "initialize", &nats, &js_msg, init_handler_error).await;
    assert!(result.is_err());
}

#[tokio::test]
async fn handle_request_with_keepalive_handler_returns_error() {
    let nats = MockNatsClient::new();
    let init = InitializeRequest::new(ProtocolVersion::V0);
    let encoded = wire_encode_request("initialize", &init);
    let msg = make_nats_message("acp.v1.global.agent.initialize", &encoded.body, Some("_INBOX.1"));
    let mut msg = msg;
    msg.headers = Some(encoded.headers);
    let js_msg = make_js_msg("acp.v1.global.agent.initialize", &init, Some("_INBOX.1"));
    let result = handle_jsonrpc_request_with_keepalive(&msg, "initialize", &nats, &js_msg, init_handler_error).await;
    assert!(result.is_ok());
    assert_eq!(nats.published_messages(), vec!["_INBOX.1"]);
    let error = published_wire_error(&nats);
    assert_eq!(error.code, ErrorCode::INTERNAL_ERROR);
}

#[tokio::test(start_paused = true)]
async fn handle_request_with_keepalive_progress_ack_failure() {
    let _guard = tracing_subscriber::fmt().with_test_writer().set_default();

    let nats = MockNatsClient::new();
    let init = InitializeRequest::new(ProtocolVersion::V0);
    let encoded = wire_encode_request("initialize", &init);
    let msg = make_nats_message("acp.v1.global.agent.initialize", &encoded.body, Some("_INBOX.1"));
    let mut msg = msg;
    msg.headers = Some(encoded.headers.clone());

    let headers = encoded.headers;
    let js_msg = MockJsMessage::with_failing_signals(async_nats::Message {
        subject: "acp.v1.global.agent.initialize".into(),
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
    let msg = make_nats_message("acp.v1.global.agent.authenticate", &encoded.body, Some("_INBOX.1"));
    let mut msg = msg;
    msg.headers = Some(encoded.headers);
    let js_msg = make_js_msg("acp.v1.global.agent.authenticate", &auth, Some("_INBOX.1"));

    let agent = MockAgent::new();
    let result =
        handle_jsonrpc_request_with_keepalive(&msg, "authenticate", &nats, &js_msg, |req: AuthenticateRequest| {
            agent.authenticate(req)
        })
        .await;
    assert!(result.is_ok());
    assert_eq!(nats.published_messages(), vec!["_INBOX.1"]);
    let error = published_wire_error(&nats);
    assert_eq!(error.code, ErrorCode::METHOD_NOT_FOUND);
}

fn schema_1_15_requests() -> Vec<(&'static str, &'static str, serde_json::Value, serde_json::Value)> {
    vec![
        (
            "acp.v1.global.agent.auth.status",
            "auth/status",
            serde_json::json!({"_meta":{"probe":"auth"}}),
            serde_json::json!({"state":"unauthenticated"}),
        ),
        (
            "acp.v1.session.s1.agent.archive",
            "session/archive",
            serde_json::json!({"sessionId":"s1","_meta":{"probe":"archive"}}),
            serde_json::json!({}),
        ),
        (
            "acp.v1.session.s1.agent.unarchive",
            "session/unarchive",
            serde_json::json!({"sessionId":"s1","_meta":{"probe":"unarchive"}}),
            serde_json::json!({}),
        ),
        (
            "acp.v1.session.s1.agent.status",
            "session/status",
            serde_json::json!({"sessionId":"s1","_meta":{"probe":"status"}}),
            serde_json::json!({"status":"saved"}),
        ),
        (
            "acp.v1.session.s1.agent.set_title",
            "session/set_title",
            serde_json::json!({"sessionId":"s1","title":"Renamed","_meta":{"probe":"title"}}),
            serde_json::json!({"title":"Renamed"}),
        ),
    ]
}

#[tokio::test]
async fn schema_1_15_core_dispatch_preserves_request_and_response() {
    for (subject, method, params, result) in schema_1_15_requests() {
        let (nats, agent) = dispatch(subject, &params, Some("_INBOX.schema-upgrade")).await;
        assert_eq!(*agent.schema_calls.lock().unwrap(), vec![(method.to_owned(), params)]);
        assert_eq!(published_response::<serde_json::Value>(&nats), result);
        assert_eq!(nats.published_messages(), vec!["_INBOX.schema-upgrade"]);
    }
}

#[tokio::test]
async fn schema_1_15_durable_dispatch_preserves_request_and_response() {
    for (subject, method, params, result) in schema_1_15_requests().into_iter().skip(1) {
        let nats = MockNatsClient::new();
        let agent = MockAgent::new();
        let msg = make_js_msg(subject, &params, None);
        dispatch_js_message(&msg, &agent, &AdmitAll, &nats, &test_prefix()).await;
        assert_eq!(*agent.schema_calls.lock().unwrap(), vec![(method.to_owned(), params)]);
        assert_eq!(published_response::<serde_json::Value>(&nats), result);
        assert_eq!(nats.published_messages(), vec!["acp.v1.session.s1.agent.response"]);
    }
}

/// Serves only `discord`, as `acp-host` does once that client has claimed it.
struct OnlyDiscord;

impl ClientGate for OnlyDiscord {
    fn admit(&self, client: Option<&str>) -> agent_client_protocol::Result<()> {
        match client {
            None | Some("discord") => Ok(()),
            Some(other) => Err(AcpError::new(
                ErrorCode::INVALID_REQUEST,
                format!("not serving {other}"),
            )),
        }
    }
}

fn from_client(mut msg: Message, client: Option<&str>) -> Message {
    if let Some(client) = client {
        msg.headers
            .get_or_insert_with(async_nats::HeaderMap::new)
            .insert(acp_nats::ACP_CLIENT_HEADER, client);
    }
    msg
}

async fn dispatch_from<T: serde::Serialize>(
    client: Option<&str>,
    subject: &str,
    args: &T,
    reply: Option<&str>,
) -> (MockNatsClient, MockAgent) {
    let nats = MockNatsClient::new();
    let agent = MockAgent::new();
    let method = wire_method_for_subject(subject);
    let encoded = wire_encode_request(&method, args);
    let mut msg = make_nats_message(subject, &encoded.body, reply);
    msg.headers = Some(encoded.headers);
    dispatch_message(from_client(msg, client), &agent, &OnlyDiscord, &nats).await;
    (nats, agent)
}

#[tokio::test]
async fn a_refused_client_gets_the_gates_error_and_the_agent_never_sees_the_request() {
    let (nats, agent) = dispatch_from(
        Some("zed"),
        "acp.v1.global.agent.session.new",
        &NewSessionRequest::new("/tmp"),
        Some("_INBOX.zed"),
    )
    .await;

    assert!(agent.received_new_session.lock().unwrap().is_none());
    assert_eq!(nats.published_messages(), vec!["_INBOX.zed"]);
    let error = published_wire_error(&nats);
    assert_eq!(error.code, ErrorCode::INVALID_REQUEST);
    assert_eq!(error.message, "not serving zed");
}

#[tokio::test]
async fn a_refused_initialize_never_reaches_the_agent() {
    let (nats, agent) = dispatch_from(
        Some("zed"),
        "acp.v1.global.agent.initialize",
        &init_request(),
        Some("_INBOX.zed"),
    )
    .await;

    assert!(!*agent.initialized.lock().unwrap());
    assert_eq!(published_wire_error(&nats).code, ErrorCode::INVALID_REQUEST);
}

#[tokio::test]
async fn the_admitted_client_and_unlabelled_requests_are_served() {
    for client in [Some("discord"), None] {
        let (nats, agent) = dispatch_from(
            client,
            "acp.v1.global.agent.session.new",
            &NewSessionRequest::new("/tmp"),
            Some("_INBOX.1"),
        )
        .await;

        assert!(agent.received_new_session.lock().unwrap().is_some(), "{client:?}");
        let response: NewSessionResponse = published_response(&nats);
        assert_eq!(response.session_id.to_string(), "sess-1");
    }
}

#[tokio::test]
async fn a_refused_notification_is_dropped_without_a_reply() {
    let nats = MockNatsClient::new();
    let agent = MockAgent::new();
    let encoded = wire_encode_notification("_note", &serde_json::json!({}));
    let mut msg = make_nats_message("acp.v1.global.agent.ext.note", &encoded.body, None);
    msg.headers = Some(encoded.headers);

    dispatch_message(from_client(msg, Some("zed")), &agent, &OnlyDiscord, &nats).await;

    assert!(agent.schema_calls.lock().unwrap().is_empty());
    assert!(nats.published_messages().is_empty());
}

fn js_from<T: serde::Serialize>(client: Option<&str>, subject: &str, args: &T) -> MockJsMessage {
    let msg = make_js_msg(subject, args, None);
    use trogon_nats::jetstream::JsMessageRef as _;
    MockJsMessage::new(from_client(msg.message().clone(), client))
}

#[tokio::test]
async fn a_refused_clients_session_command_is_answered_with_the_refusal_and_consumed() {
    let nats = MockNatsClient::new();
    let agent = MockAgent::new();
    let js_msg = js_from(
        Some("zed"),
        "acp.v1.session.s1.agent.prompt",
        &PromptRequest::new("s1", vec![]),
    );
    let signals = js_msg.clone();

    dispatch_js_message(&js_msg, &agent, &OnlyDiscord, &nats, &test_prefix()).await;

    assert_eq!(nats.published_messages(), vec!["acp.v1.session.s1.agent.response"]);
    let error = published_wire_error(&nats);
    assert_eq!(error.code, ErrorCode::INVALID_REQUEST);
    assert_eq!(signals.signals(), vec![AckKindSnapshot::Ack]);
}

#[tokio::test]
async fn a_refused_clients_cancel_never_reaches_the_agent() {
    let nats = MockNatsClient::new();
    let agent = MockAgent::new();
    let js_msg = js_from(
        Some("zed"),
        "acp.v1.session.s1.agent.cancel",
        &CancelNotification::new("s1"),
    );

    dispatch_js_message(&js_msg, &agent, &OnlyDiscord, &nats, &test_prefix()).await;

    assert!(agent.cancelled.lock().unwrap().is_empty());
    assert!(nats.published_messages().is_empty());
}

#[tokio::test]
async fn the_admitted_clients_session_command_is_served() {
    let nats = MockNatsClient::new();
    let agent = MockAgent::new();
    let js_msg = js_from(
        Some("discord"),
        "acp.v1.session.s1.agent.cancel",
        &CancelNotification::new("s1"),
    );

    dispatch_js_message(&js_msg, &agent, &OnlyDiscord, &nats, &test_prefix()).await;

    assert_eq!(*agent.cancelled.lock().unwrap(), vec!["s1".to_string()]);
}

/// An agent whose `session/load` never finishes, like a command the host is
/// still working on when it is told to stop.
struct StuckLoadAgent {
    loads: Mutex<u32>,
}

#[async_trait::async_trait]
impl AgentHandler for StuckLoadAgent {
    async fn initialize(&self, args: InitializeRequest) -> agent_client_protocol::Result<InitializeResponse> {
        Ok(InitializeResponse::new(args.protocol_version))
    }

    async fn authenticate(&self, _args: AuthenticateRequest) -> agent_client_protocol::Result<AuthenticateResponse> {
        Ok(AuthenticateResponse::new())
    }

    async fn new_session(&self, _args: NewSessionRequest) -> agent_client_protocol::Result<NewSessionResponse> {
        Ok(NewSessionResponse::new("s1"))
    }

    async fn prompt(&self, _args: PromptRequest) -> agent_client_protocol::Result<PromptResponse> {
        Ok(PromptResponse::new(StopReason::EndTurn))
    }

    async fn cancel(&self, _args: CancelNotification) -> agent_client_protocol::Result<()> {
        Ok(())
    }

    async fn load_session(
        &self,
        _args: LoadSessionRequest,
    ) -> agent_client_protocol::Result<agent_client_protocol::v1::schema::LoadSessionResponse> {
        *self.loads.lock().unwrap() += 1;
        std::future::pending().await
    }
}

#[tokio::test]
async fn stopping_nacks_in_flight_and_undispatched_commands_and_returns() {
    let nats = MockNatsClient::new();
    let factory = MockJetStreamConsumerFactory::new();
    let (consumer, tx) = MockJetStreamConsumer::new();
    factory.add_consumer(consumer);
    let agent = Rc::new(StuckLoadAgent { loads: Mutex::new(0) });
    let in_flight = make_js_msg(
        "acp.v1.session.s1.agent.load",
        &LoadSessionRequest::new("s1", "/tmp"),
        None,
    );
    let undispatched = make_js_msg(
        "acp.v1.session.s2.agent.load",
        &LoadSessionRequest::new("s2", "/tmp"),
        None,
    );
    let stop = Rc::new(tokio::sync::Notify::new());

    let local = tokio::task::LocalSet::new();
    local
        .run_until(async {
            tx.unbounded_send(Ok(in_flight.clone())).unwrap();
            let serving = tokio::task::spawn_local({
                let stop = stop.clone();
                let agent = agent.clone();
                async move {
                    serve_js(
                        agent,
                        Rc::new(AdmitAll),
                        nats.clone(),
                        factory,
                        &test_prefix(),
                        |fut| {
                            tokio::task::spawn_local(fut);
                        },
                        async move { stop.notified().await },
                    )
                    .await
                }
            });
            while *agent.loads.lock().unwrap() == 0 {
                tokio::task::yield_now().await;
            }

            // The next command is delivered as the stop arrives: it is read
            // but never dispatched.
            tx.unbounded_send(Ok(undispatched.clone())).unwrap();
            stop.notify_one();
            let result = tokio::time::timeout(Duration::from_secs(5), serving)
                .await
                .expect("serving returns once every command is settled")
                .unwrap();

            assert!(result.is_ok());
            let nak = vec![AckKindSnapshot::AckWith(AckKindValue::Nak(None))];
            assert_eq!(in_flight.signals(), nak);
            assert_eq!(undispatched.signals(), nak);
            assert_eq!(*agent.loads.lock().unwrap(), 1);
        })
        .await;
}

/// An agent that relays the handshake: it keeps `initialize` params as sent.
#[derive(Default)]
struct RelayingAgent {
    initialize_params: Mutex<Vec<serde_json::Value>>,
}

#[async_trait::async_trait]
impl AgentHandler for RelayingAgent {
    async fn initialize(&self, _args: InitializeRequest) -> agent_client_protocol::Result<InitializeResponse> {
        panic!("a relaying agent is initialized with the params as sent")
    }

    async fn initialize_raw(&self, params: serde_json::Value) -> agent_client_protocol::Result<InitializeResponse> {
        self.initialize_params.lock().unwrap().push(params);
        Ok(InitializeResponse::new(ProtocolVersion::V1))
    }

    async fn authenticate(&self, _args: AuthenticateRequest) -> agent_client_protocol::Result<AuthenticateResponse> {
        Ok(AuthenticateResponse::new())
    }

    async fn new_session(&self, _args: NewSessionRequest) -> agent_client_protocol::Result<NewSessionResponse> {
        Ok(NewSessionResponse::new("s1"))
    }

    async fn prompt(&self, _args: PromptRequest) -> agent_client_protocol::Result<PromptResponse> {
        Ok(PromptResponse::new(StopReason::EndTurn))
    }

    async fn cancel(&self, _args: CancelNotification) -> agent_client_protocol::Result<()> {
        Ok(())
    }
}

#[tokio::test]
async fn initialize_reaches_the_agent_with_members_the_schema_does_not_name() {
    let nats = MockNatsClient::new();
    let agent = RelayingAgent::default();
    let sent = serde_json::json!({
        "protocolVersion": 1,
        "clientCapabilities": {"libtrogonai": {"tools": [{"name": "send_message"}]}},
    });
    let encoded = wire_encode_request("initialize", &sent);
    let mut msg = make_nats_message("acp.v1.global.agent.initialize", &encoded.body, Some("_INBOX.1"));
    msg.headers = Some(encoded.headers);

    dispatch_message(msg, &agent, &AdmitAll, &nats).await;

    assert_eq!(*agent.initialize_params.lock().unwrap(), vec![sent]);
    let response: InitializeResponse = published_response(&nats);
    assert_eq!(response.protocol_version, ProtocolVersion::V1);
}
