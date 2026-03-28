use acp_nats::acp_prefix::AcpPrefix;
use acp_nats::client_proxy::NatsClientProxy;
use acp_nats::nats::{AgentMethod, parse_agent_subject};
use acp_nats::session_id::AcpSessionId;
use agent_client_protocol::{
    Agent, AuthenticateRequest, CancelNotification, CloseSessionRequest, ExtNotification,
    ExtRequest, ForkSessionRequest, InitializeRequest, ListSessionsRequest, LoadSessionRequest,
    NewSessionRequest, PromptRequest, ResumeSessionRequest, SetSessionConfigOptionRequest,
    SetSessionModeRequest, SetSessionModelRequest,
};
use async_nats::Message;
#[cfg(test)]
use bytes::Bytes;
use futures::StreamExt;
use futures::future::LocalBoxFuture;
use std::rc::Rc;
use std::time::Duration;
use tracing::{info, warn};
use trogon_nats::{FlushClient, PublishClient, RequestClient, SubscribeClient};

pub enum ConnectionError {
    Subscribe(Box<dyn std::error::Error + Send + Sync>),
}

impl std::fmt::Debug for ConnectionError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Subscribe(e) => f.debug_tuple("Subscribe").field(e).finish(),
        }
    }
}

impl std::fmt::Display for ConnectionError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Subscribe(e) => write!(f, "failed to subscribe: {}", e),
        }
    }
}

impl std::error::Error for ConnectionError {}

#[derive(Debug)]
enum DispatchError {
    NoReplySubject,
    DeserializeRequest(serde_json::Error),
    DeserializeNotification(serde_json::Error),
    Reply(trogon_nats::NatsError),
    NotificationHandler(agent_client_protocol::Error),
}

impl std::fmt::Display for DispatchError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NoReplySubject => write!(f, "no reply subject"),
            Self::DeserializeRequest(e) => write!(f, "deserialize request: {}", e),
            Self::DeserializeNotification(e) => write!(f, "deserialize notification: {}", e),
            Self::Reply(e) => write!(f, "reply: {}", e),
            Self::NotificationHandler(e) => write!(f, "notification handler: {}", e),
        }
    }
}

use crate::constants::DEFAULT_OPERATION_TIMEOUT;

pub struct AgentSideNatsConnection<N> {
    nats: N,
    acp_prefix: AcpPrefix,
    operation_timeout: Duration,
}

impl<N> AgentSideNatsConnection<N>
where
    N: SubscribeClient + RequestClient + PublishClient + FlushClient + Clone + 'static,
{
    pub fn new(
        agent: impl Agent + 'static,
        nats: N,
        acp_prefix: AcpPrefix,
        spawn: impl Fn(LocalBoxFuture<'static, ()>) + 'static,
    ) -> (
        Self,
        impl std::future::Future<Output = Result<(), ConnectionError>>,
    ) {
        let nats_for_serve = nats.clone();
        let prefix = acp_prefix.as_str().to_string();

        let io_task = async move { serve(agent, nats_for_serve, &prefix, spawn).await };

        let conn = Self {
            nats,
            acp_prefix,
            operation_timeout: DEFAULT_OPERATION_TIMEOUT,
        };
        (conn, io_task)
    }

    pub fn client_for_session(&self, session_id: AcpSessionId) -> NatsClientProxy<N> {
        NatsClientProxy::new(
            self.nats.clone(),
            session_id,
            self.acp_prefix.clone(),
            self.operation_timeout,
        )
    }
}

async fn serve<N, A>(
    agent: A,
    nats: N,
    prefix: &str,
    spawn: impl Fn(LocalBoxFuture<'static, ()>) + 'static,
) -> Result<(), ConnectionError>
where
    N: SubscribeClient + PublishClient + FlushClient + Clone + 'static,
    A: Agent + 'static,
{
    let global_wildcard = acp_nats::nats::agent::wildcards::all(prefix);
    let session_wildcard = acp_nats::nats::session::wildcards::all_agent(prefix);

    info!(
        global = %global_wildcard,
        session = %session_wildcard,
        "Starting agent-side NATS connection"
    );

    let global_sub = nats
        .subscribe(global_wildcard)
        .await
        .map_err(|e| ConnectionError::Subscribe(Box::new(e)))?;

    let session_sub = nats
        .subscribe(session_wildcard)
        .await
        .map_err(|e| ConnectionError::Subscribe(Box::new(e)))?;

    let mut subscriber = futures::stream::select(global_sub, session_sub);

    let agent = Rc::new(agent);
    let nats = Rc::new(nats);

    while let Some(msg) = subscriber.next().await {
        let agent = agent.clone();
        let nats = nats.clone();
        spawn(Box::pin(async move {
            dispatch_message(msg, agent.as_ref(), nats.as_ref()).await;
        }));
    }

    info!("Agent-side NATS connection ended");
    Ok(())
}

async fn dispatch_message<N: PublishClient + FlushClient, A: Agent>(
    msg: Message,
    agent: &A,
    nats: &N,
) {
    let subject = msg.subject.as_str();

    let parsed = match parse_agent_subject(subject) {
        Some(p) => p,
        None => return,
    };

    let result = match parsed.method {
        AgentMethod::Initialize => {
            handle_request(&msg, nats, |req: InitializeRequest| agent.initialize(req)).await
        }
        AgentMethod::Authenticate => {
            handle_request(&msg, nats, |req: AuthenticateRequest| {
                agent.authenticate(req)
            })
            .await
        }
        AgentMethod::SessionNew => {
            handle_request(&msg, nats, |req: NewSessionRequest| agent.new_session(req)).await
        }
        AgentMethod::SessionList => {
            handle_request(&msg, nats, |req: ListSessionsRequest| {
                agent.list_sessions(req)
            })
            .await
        }
        AgentMethod::SessionLoad => {
            handle_request(&msg, nats, |req: LoadSessionRequest| {
                agent.load_session(req)
            })
            .await
        }
        AgentMethod::SessionPrompt => {
            handle_request(&msg, nats, |req: PromptRequest| agent.prompt(req)).await
        }
        AgentMethod::SessionCancel => {
            handle_notification(&msg, |req: CancelNotification| agent.cancel(req)).await
        }
        AgentMethod::SessionSetMode => {
            handle_request(&msg, nats, |req: SetSessionModeRequest| {
                agent.set_session_mode(req)
            })
            .await
        }
        AgentMethod::SessionSetConfigOption => {
            handle_request(&msg, nats, |req: SetSessionConfigOptionRequest| {
                agent.set_session_config_option(req)
            })
            .await
        }
        AgentMethod::SessionSetModel => {
            handle_request(&msg, nats, |req: SetSessionModelRequest| {
                agent.set_session_model(req)
            })
            .await
        }
        AgentMethod::SessionFork => {
            handle_request(&msg, nats, |req: ForkSessionRequest| {
                agent.fork_session(req)
            })
            .await
        }
        AgentMethod::SessionResume => {
            handle_request(&msg, nats, |req: ResumeSessionRequest| {
                agent.resume_session(req)
            })
            .await
        }
        AgentMethod::SessionClose => {
            handle_request(&msg, nats, |req: CloseSessionRequest| {
                agent.close_session(req)
            })
            .await
        }
        AgentMethod::Ext(_) => {
            if msg.reply.is_some() {
                handle_request(&msg, nats, |req: ExtRequest| agent.ext_method(req)).await
            } else {
                handle_notification(&msg, |req: ExtNotification| agent.ext_notification(req)).await
            }
        }
    };

    if let Err(e) = result {
        let sid = parsed
            .session_id
            .as_ref()
            .map(|s| s.as_str())
            .unwrap_or("-");
        warn!(subject, session_id = sid, error = %e, "Error handling agent request");
    }
}

async fn reply<N: PublishClient + FlushClient, T: serde::Serialize>(
    nats: &N,
    reply_to: &str,
    value: &T,
) -> Result<(), DispatchError> {
    trogon_nats::publish(
        nats,
        reply_to,
        value,
        trogon_nats::PublishOptions::builder()
            .flush_policy(trogon_nats::FlushPolicy::standard())
            .build(),
    )
    .await
    .map_err(DispatchError::Reply)
}

async fn handle_request<N, Resp, ReqT, F>(
    msg: &Message,
    nats: &N,
    handler: impl FnOnce(ReqT) -> F,
) -> Result<(), DispatchError>
where
    N: PublishClient + FlushClient,
    ReqT: serde::de::DeserializeOwned,
    F: std::future::Future<Output = agent_client_protocol::Result<Resp>>,
    Resp: serde::Serialize,
{
    let reply_to = msg.reply.as_deref().ok_or(DispatchError::NoReplySubject)?;

    let request: ReqT = match serde_json::from_slice(&msg.payload) {
        Ok(req) => req,
        Err(e) => {
            let error = agent_client_protocol::Error::new(
                agent_client_protocol::ErrorCode::InvalidParams.into(),
                format!("Failed to deserialize request: {}", e),
            );
            let _ = reply(nats, reply_to, &error).await;
            return Err(DispatchError::DeserializeRequest(e));
        }
    };

    match handler(request).await {
        Ok(resp) => reply(nats, reply_to, &resp).await,
        Err(err) => reply(nats, reply_to, &err).await,
    }
}

async fn handle_notification<ReqT, F>(
    msg: &Message,
    handler: impl FnOnce(ReqT) -> F,
) -> Result<(), DispatchError>
where
    ReqT: serde::de::DeserializeOwned,
    F: std::future::Future<Output = agent_client_protocol::Result<()>>,
{
    let request: ReqT =
        serde_json::from_slice(&msg.payload).map_err(DispatchError::DeserializeNotification)?;

    handler(request)
        .await
        .map_err(DispatchError::NotificationHandler)
}

#[cfg(test)]
mod tests {
    use super::*;
    use agent_client_protocol::{
        AuthenticateResponse, Error as AcpError, ErrorCode, InitializeResponse, PromptResponse,
        StopReason,
    };
    use std::cell::RefCell;
    use trogon_nats::MockNatsClient;

    struct MockAgent {
        initialized: RefCell<bool>,
        cancelled: RefCell<Vec<String>>,
    }

    impl MockAgent {
        fn new() -> Self {
            Self {
                initialized: RefCell::new(false),
                cancelled: RefCell::new(Vec::new()),
            }
        }
    }

    #[async_trait::async_trait(?Send)]
    impl Agent for MockAgent {
        async fn initialize(
            &self,
            _args: InitializeRequest,
        ) -> agent_client_protocol::Result<InitializeResponse> {
            *self.initialized.borrow_mut() = true;
            Ok(InitializeResponse::new(
                agent_client_protocol::ProtocolVersion::V0,
            ))
        }

        async fn authenticate(
            &self,
            _args: AuthenticateRequest,
        ) -> agent_client_protocol::Result<AuthenticateResponse> {
            Err(AcpError::method_not_found())
        }

        async fn new_session(
            &self,
            _args: NewSessionRequest,
        ) -> agent_client_protocol::Result<agent_client_protocol::NewSessionResponse> {
            Ok(agent_client_protocol::NewSessionResponse::new("sess-1"))
        }

        async fn prompt(
            &self,
            _args: PromptRequest,
        ) -> agent_client_protocol::Result<PromptResponse> {
            Ok(PromptResponse::new(StopReason::EndTurn))
        }

        async fn cancel(&self, args: CancelNotification) -> agent_client_protocol::Result<()> {
            self.cancelled
                .borrow_mut()
                .push(args.session_id.to_string());
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

    async fn dispatch<T: serde::Serialize>(
        subject: &str,
        args: &T,
        reply: Option<&str>,
    ) -> (MockNatsClient, MockAgent) {
        let nats = MockNatsClient::new();
        let agent = MockAgent::new();
        let payload = serialize(args);
        let msg = make_nats_message(subject, &payload, reply);
        dispatch_message(msg, &agent, &nats).await;
        (nats, agent)
    }

    async fn dispatch_raw(
        subject: &str,
        payload: &[u8],
        reply: Option<&str>,
    ) -> (MockNatsClient, MockAgent) {
        let nats = MockNatsClient::new();
        let agent = MockAgent::new();
        let msg = make_nats_message(subject, payload, reply);
        dispatch_message(msg, &agent, &nats).await;
        (nats, agent)
    }

    fn published_response<T: serde::de::DeserializeOwned>(nats: &MockNatsClient) -> T {
        let payloads = nats.published_payloads();
        assert_eq!(payloads.len(), 1);
        serde_json::from_slice(&payloads[0]).unwrap()
    }

    fn init_request() -> InitializeRequest {
        InitializeRequest::new(agent_client_protocol::ProtocolVersion::V0)
    }

    #[tokio::test]
    async fn dispatch_initialize_calls_agent_and_publishes_response() {
        let (nats, agent) =
            dispatch("acp.agent.initialize", &init_request(), Some("_INBOX.1")).await;

        assert!(*agent.initialized.borrow());
        let response: InitializeResponse = published_response(&nats);
        assert_eq!(
            response.protocol_version,
            agent_client_protocol::ProtocolVersion::V0
        );
    }

    #[tokio::test]
    async fn dispatch_authenticate_error_publishes_acp_error() {
        let (nats, _) = dispatch(
            "acp.agent.authenticate",
            &AuthenticateRequest::new("basic"),
            Some("_INBOX.2"),
        )
        .await;

        let error: AcpError = published_response(&nats);
        assert_eq!(error.code, ErrorCode::MethodNotFound);
    }

    #[tokio::test]
    async fn dispatch_cancel_is_notification_no_reply_published() {
        let (nats, agent) = dispatch(
            "acp.session.s1.agent.cancel",
            &CancelNotification::new("s1"),
            None,
        )
        .await;

        assert_eq!(agent.cancelled.borrow().len(), 1);
        assert!(nats.published_messages().is_empty());
    }

    #[tokio::test]
    async fn dispatch_invalid_payload_publishes_error_reply() {
        let (nats, agent) =
            dispatch_raw("acp.agent.initialize", b"not json", Some("_INBOX.err")).await;

        assert!(!*agent.initialized.borrow());
        let error: AcpError = published_response(&nats);
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
        let (nats, _) = dispatch(
            "acp.agent.initialize",
            &init_request(),
            Some("_INBOX.specific"),
        )
        .await;
        assert_eq!(nats.published_messages(), vec!["_INBOX.specific"]);
    }

    #[test]
    fn connection_error_display() {
        let err = ConnectionError::Subscribe(Box::new(std::io::Error::other("test")));
        assert!(err.to_string().contains("failed to subscribe"));
        assert!(err.to_string().contains("test"));
    }

    #[test]
    fn connection_error_debug() {
        let err = ConnectionError::Subscribe(Box::new(std::io::Error::other("test")));
        let debug = format!("{:?}", err);
        assert!(debug.contains("Subscribe"));
    }

    #[test]
    fn dispatch_error_display_variants() {
        assert_eq!(
            DispatchError::NoReplySubject.to_string(),
            "no reply subject"
        );

        let json_err = serde_json::from_slice::<()>(b"bad").unwrap_err();
        assert!(
            DispatchError::DeserializeRequest(json_err)
                .to_string()
                .contains("deserialize request")
        );

        let json_err = serde_json::from_slice::<()>(b"bad").unwrap_err();
        assert!(
            DispatchError::DeserializeNotification(json_err)
                .to_string()
                .contains("deserialize notification")
        );

        let acp_err = agent_client_protocol::Error::internal_error();
        assert!(
            DispatchError::NotificationHandler(acp_err)
                .to_string()
                .contains("notification handler")
        );
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

    async fn assert_dispatch_publishes<T: serde::Serialize>(subject: &str, args: &T) {
        let (nats, _) = dispatch(subject, args, Some("_INBOX.r")).await;
        assert_eq!(nats.published_messages(), vec!["_INBOX.r"]);
    }

    #[tokio::test]
    async fn dispatch_new_session_publishes_response() {
        assert_dispatch_publishes("acp.agent.session.new", &NewSessionRequest::new("/tmp")).await;
    }

    #[tokio::test]
    async fn dispatch_session_load_publishes_response() {
        assert_dispatch_publishes(
            "acp.session.s1.agent.load",
            &LoadSessionRequest::new("s1", "/tmp"),
        )
        .await;
    }

    #[tokio::test]
    async fn dispatch_list_sessions_publishes_response() {
        assert_dispatch_publishes("acp.agent.session.list", &ListSessionsRequest::new()).await;
    }

    #[tokio::test]
    async fn dispatch_set_session_mode_publishes_response() {
        assert_dispatch_publishes(
            "acp.session.s1.agent.set_mode",
            &SetSessionModeRequest::new("s1", "code"),
        )
        .await;
    }

    #[tokio::test]
    async fn dispatch_set_session_config_option_publishes_response() {
        assert_dispatch_publishes(
            "acp.session.s1.agent.set_config_option",
            &SetSessionConfigOptionRequest::new("s1", "key", "val"),
        )
        .await;
    }

    #[tokio::test]
    async fn dispatch_set_session_model_publishes_response() {
        assert_dispatch_publishes(
            "acp.session.s1.agent.set_model",
            &SetSessionModelRequest::new("s1", "gpt-4"),
        )
        .await;
    }

    #[tokio::test]
    async fn dispatch_fork_session_publishes_response() {
        assert_dispatch_publishes(
            "acp.session.s1.agent.fork",
            &ForkSessionRequest::new("s1", "/tmp"),
        )
        .await;
    }

    #[tokio::test]
    async fn dispatch_resume_session_publishes_response() {
        assert_dispatch_publishes(
            "acp.session.s1.agent.resume",
            &ResumeSessionRequest::new("s1", "/tmp"),
        )
        .await;
    }

    #[tokio::test]
    async fn dispatch_close_session_publishes_response() {
        assert_dispatch_publishes(
            "acp.session.s1.agent.close",
            &CloseSessionRequest::new("s1"),
        )
        .await;
    }

    #[test]
    fn dispatch_error_display_reply_variant() {
        let err = DispatchError::Reply(trogon_nats::NatsError::Timeout {
            subject: "test".to_string(),
        });
        assert!(err.to_string().contains("reply"));
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
                let (conn, io_task) = AgentSideNatsConnection::new(
                    MockAgent::new(),
                    nats,
                    AcpPrefix::new("acp").unwrap(),
                    |fut| {
                        tokio::task::spawn_local(fut);
                    },
                );

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
                let (conn, io_task) = AgentSideNatsConnection::new(
                    MockAgent::new(),
                    nats,
                    AcpPrefix::new("acp").unwrap(),
                    |fut| {
                        tokio::task::spawn_local(fut);
                    },
                );

                let _client = conn.client_for_session(AcpSessionId::new("sess-1").unwrap());

                let result = io_task.await;
                assert!(result.is_ok());
            })
            .await;
    }

    #[tokio::test]
    async fn dispatch_error_logs_warning_with_subscriber() {
        use tracing_subscriber::util::SubscriberInitExt;
        let _guard = tracing_subscriber::fmt().with_test_writer().set_default();

        let nats = MockNatsClient::new();
        let agent = MockAgent::new();
        let payload = serialize(&InitializeRequest::new(
            agent_client_protocol::ProtocolVersion::V0,
        ));
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
                let payload = serialize(&InitializeRequest::new(
                    agent_client_protocol::ProtocolVersion::V0,
                ));
                let msg = Message {
                    subject: "acp.agent.initialize".into(),
                    reply: Some("_INBOX.serve".into()),
                    payload: Bytes::copy_from_slice(&payload),
                    headers: None,
                    status: None,
                    description: None,
                    length: 0,
                };

                global_tx.unbounded_send(msg).unwrap();
                drop(global_tx);
                drop(session_tx);

                let result = serve(agent, nats.clone(), "acp", |fut| {
                    tokio::task::spawn_local(fut);
                })
                .await;

                assert!(result.is_ok());

                tokio::task::yield_now().await;
                tokio::task::yield_now().await;

                assert_eq!(nats.published_messages().len(), 1);
                assert_eq!(nats.published_messages()[0], "_INBOX.serve");
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
                let result = serve(agent, nats, "acp", |fut| {
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
                let _ = serve(agent, nats.clone(), "myprefix", |fut| {
                    tokio::task::spawn_local(fut);
                })
                .await;

                let subjects = nats.subscribed_to();
                assert!(subjects.contains(&"myprefix.agent.>".to_string()));
                assert!(subjects.contains(&"myprefix.session.*.agent.>".to_string()));
            })
            .await;
    }
}
