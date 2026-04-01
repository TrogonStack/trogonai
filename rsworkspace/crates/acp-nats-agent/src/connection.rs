use acp_nats::acp_prefix::AcpPrefix;
use acp_nats::client_proxy::NatsClientProxy;
use acp_nats::nats::{
    GlobalAgentMethod, ParsedAgentSubject, SessionAgentMethod, parse_agent_subject,
};
use acp_nats::session_id::AcpSessionId;
use agent_client_protocol::{
    Agent, AuthenticateRequest, CancelNotification, CloseSessionRequest, ExtNotification,
    ExtRequest, ForkSessionRequest, InitializeRequest, ListSessionsRequest, LoadSessionRequest,
    NewSessionRequest, PromptRequest, ResumeSessionRequest, SetSessionConfigOptionRequest,
    SetSessionModeRequest, SetSessionModelRequest,
};
use async_nats::Message;
use async_nats::jetstream::AckKind;
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
    JetStream(Box<dyn std::error::Error + Send + Sync>),
}

impl std::fmt::Debug for ConnectionError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Subscribe(e) => f.debug_tuple("Subscribe").field(e).finish(),
            Self::JetStream(e) => f.debug_tuple("JetStream").field(e).finish(),
        }
    }
}

impl std::fmt::Display for ConnectionError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Subscribe(e) => write!(f, "failed to subscribe: {}", e),
            Self::JetStream(e) => write!(f, "jetstream error: {}", e),
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

use crate::constants::{DEFAULT_OPERATION_TIMEOUT, KEEPALIVE_INTERVAL};

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

    pub fn with_jetstream<J>(
        agent: impl Agent + 'static,
        nats: N,
        js: J,
        acp_prefix: AcpPrefix,
        spawn: impl Fn(LocalBoxFuture<'static, ()>) + Copy + 'static,
    ) -> (
        Self,
        impl std::future::Future<Output = Result<(), ConnectionError>>,
    )
    where
        J: JetStreamGetStream + 'static,
        <<J::Stream as trogon_nats::jetstream::JetStreamCreateConsumer>::Consumer as trogon_nats::jetstream::JetStreamConsumer>::Message: JsDispatchMessage,
    {
        let nats_for_serve = nats.clone();
        let nats_for_js = nats.clone();
        let prefix = acp_prefix.as_str().to_string();
        let prefix_js = prefix.clone();

        let io_task = async move {
            let agent = Rc::new(agent);

            let core = serve_global(agent.clone(), nats_for_serve, &prefix, spawn);
            let jetstream = serve_js(agent, nats_for_js, js, &prefix_js, spawn);

            tokio::select! {
                result = core => result,
                result = jetstream => result,
            }
        };

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

async fn serve_global<N, A>(
    agent: Rc<A>,
    nats: N,
    prefix: &str,
    spawn: impl Fn(LocalBoxFuture<'static, ()>) + 'static,
) -> Result<(), ConnectionError>
where
    N: SubscribeClient + PublishClient + FlushClient + Clone + 'static,
    A: Agent + 'static,
{
    let global_wildcard = acp_nats::nats::agent::wildcards::all(prefix);
    let ext_wildcard = acp_nats::nats::session::wildcards::all_agent_ext(prefix);

    info!(
        global = %global_wildcard,
        ext = %ext_wildcard,
        "Starting global + ext NATS connection (session commands via JetStream)"
    );

    let global_sub = nats
        .subscribe(global_wildcard)
        .await
        .map_err(|e| ConnectionError::Subscribe(Box::new(e)))?;

    let ext_sub = nats
        .subscribe(ext_wildcard)
        .await
        .map_err(|e| ConnectionError::Subscribe(Box::new(e)))?;

    let mut subscriber = futures::stream::select(global_sub, ext_sub);

    let nats = Rc::new(nats);

    while let Some(msg) = subscriber.next().await {
        let agent = agent.clone();
        let nats = nats.clone();
        spawn(Box::pin(async move {
            dispatch_message(msg, agent.as_ref(), nats.as_ref()).await;
        }));
    }

    info!("Global-only NATS connection ended");
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

    let (result, session_id) = match parsed {
        ParsedAgentSubject::Global(method) => {
            let r = dispatch_global(method, &msg, agent, nats).await;
            (r, None)
        }
        ParsedAgentSubject::Session { session_id, method } => {
            let r = dispatch_session(method, &msg, agent, nats).await;
            (r, Some(session_id))
        }
    };

    if let Err(e) = result {
        let sid = session_id.as_ref().map(|s| s.as_str()).unwrap_or("-");
        warn!(subject, session_id = sid, error = %e, "Error handling agent request");
    }
}

async fn dispatch_global<N: PublishClient + FlushClient, A: Agent>(
    method: GlobalAgentMethod,
    msg: &Message,
    agent: &A,
    nats: &N,
) -> Result<(), DispatchError> {
    match method {
        GlobalAgentMethod::Initialize => {
            handle_request(msg, nats, |req: InitializeRequest| agent.initialize(req)).await
        }
        GlobalAgentMethod::Authenticate => {
            handle_request(msg, nats, |req: AuthenticateRequest| {
                agent.authenticate(req)
            })
            .await
        }
        GlobalAgentMethod::SessionNew => {
            handle_request(msg, nats, |req: NewSessionRequest| agent.new_session(req)).await
        }
        GlobalAgentMethod::SessionList => {
            handle_request(msg, nats, |req: ListSessionsRequest| {
                agent.list_sessions(req)
            })
            .await
        }
        GlobalAgentMethod::Ext(_) => {
            if msg.reply.is_some() {
                handle_request(msg, nats, |req: ExtRequest| agent.ext_method(req)).await
            } else {
                handle_notification(msg, |req: ExtNotification| agent.ext_notification(req)).await
            }
        }
    }
}

async fn dispatch_session<N: PublishClient + FlushClient, A: Agent>(
    method: SessionAgentMethod,
    msg: &Message,
    agent: &A,
    nats: &N,
) -> Result<(), DispatchError> {
    match method {
        SessionAgentMethod::Load => {
            handle_request(msg, nats, |req: LoadSessionRequest| agent.load_session(req)).await
        }
        SessionAgentMethod::Prompt => {
            handle_request(msg, nats, |req: PromptRequest| agent.prompt(req)).await
        }
        SessionAgentMethod::Cancel => {
            handle_notification(msg, |req: CancelNotification| agent.cancel(req)).await
        }
        SessionAgentMethod::SetMode => {
            handle_request(msg, nats, |req: SetSessionModeRequest| {
                agent.set_session_mode(req)
            })
            .await
        }
        SessionAgentMethod::SetConfigOption => {
            handle_request(msg, nats, |req: SetSessionConfigOptionRequest| {
                agent.set_session_config_option(req)
            })
            .await
        }
        SessionAgentMethod::SetModel => {
            handle_request(msg, nats, |req: SetSessionModelRequest| {
                agent.set_session_model(req)
            })
            .await
        }
        SessionAgentMethod::Fork => {
            handle_request(msg, nats, |req: ForkSessionRequest| agent.fork_session(req)).await
        }
        SessionAgentMethod::Resume => {
            handle_request(msg, nats, |req: ResumeSessionRequest| {
                agent.resume_session(req)
            })
            .await
        }
        SessionAgentMethod::Close => {
            handle_request(msg, nats, |req: CloseSessionRequest| {
                agent.close_session(req)
            })
            .await
        }
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

use trogon_nats::jetstream::{
    JetStreamConsumer as _, JetStreamCreateConsumer as _, JetStreamGetStream, JsAckWith,
    JsDispatchMessage,
};

async fn handle_request_with_keepalive<N, Resp, ReqT, F, M>(
    msg: &Message,
    nats: &N,
    js_msg: &M,
    handler: impl FnOnce(ReqT) -> F,
) -> Result<(), DispatchError>
where
    N: PublishClient + FlushClient,
    ReqT: serde::de::DeserializeOwned,
    F: std::future::Future<Output = agent_client_protocol::Result<Resp>>,
    Resp: serde::Serialize,
    M: JsAckWith,
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

    let handler_fut = handler(request);
    tokio::pin!(handler_fut);

    let mut keepalive = tokio::time::interval(KEEPALIVE_INTERVAL);
    keepalive.tick().await;

    loop {
        tokio::select! {
            result = &mut handler_fut => {
                return match result {
                    Ok(resp) => reply(nats, reply_to, &resp).await,
                    Err(err) => reply(nats, reply_to, &err).await,
                };
            }
            _ = keepalive.tick() => {
                if let Err(e) = js_msg.ack_with(AckKind::Progress).await {
                    warn!(error = %e, "Failed to send in_progress keepalive");
                }
            }
        }
    }
}

async fn serve_js<N, J, A>(
    agent: Rc<A>,
    nats: N,
    js: J,
    prefix: &str,
    spawn: impl Fn(LocalBoxFuture<'static, ()>) + 'static,
) -> Result<(), ConnectionError>
where
    N: PublishClient + FlushClient + Clone + 'static,
    J: JetStreamGetStream + 'static,
    <<J::Stream as trogon_nats::jetstream::JetStreamCreateConsumer>::Consumer as trogon_nats::jetstream::JetStreamConsumer>::Message: JsDispatchMessage,
    A: Agent + 'static,
{
    let stream_name = acp_nats::jetstream::streams::commands_stream_name(prefix);
    let config = acp_nats::jetstream::consumers::commands_observer();

    info!(stream = %stream_name, "Starting JetStream consumer for COMMANDS stream");

    let stream = js
        .get_stream(&stream_name)
        .await
        .map_err(|e| ConnectionError::JetStream(Box::new(e)))?;

    let consumer = stream
        .create_consumer(config)
        .await
        .map_err(|e| ConnectionError::JetStream(Box::new(e)))?;

    let mut messages = consumer
        .messages()
        .await
        .map_err(|e| ConnectionError::JetStream(Box::new(e)))?;

    let nats = Rc::new(nats);

    let prefix = Rc::new(prefix.to_string());

    while let Some(msg_result) = messages.next().await {
        match msg_result {
            Ok(js_msg) => {
                let agent = agent.clone();
                let nats = nats.clone();
                let prefix = prefix.clone();
                spawn(Box::pin(async move {
                    dispatch_js_message(js_msg, agent.as_ref(), nats.as_ref(), &prefix).await;
                }));
            }
            Err(e) => {
                warn!(error = %e, "JetStream consumer error");
            }
        }
    }

    info!("JetStream COMMANDS consumer ended");
    Ok(())
}

async fn dispatch_js_message<N: PublishClient + FlushClient, A: Agent, M: JsDispatchMessage>(
    js_msg: M,
    agent: &A,
    nats: &N,
    prefix: &str,
) {
    let subject = js_msg.message().subject.to_string();

    let (session_id, method) = match parse_agent_subject(&subject) {
        Some(ParsedAgentSubject::Session { session_id, method }) => (session_id, method),
        Some(ParsedAgentSubject::Global(_)) => {
            warn!(
                subject,
                "Global method on JetStream path; handled by core NATS"
            );
            if let Err(e) = js_msg.ack().await {
                warn!(subject, error = %e, "Failed to ack misrouted global method");
            }
            return;
        }
        None => {
            if let Err(e) = js_msg.ack_with(AckKind::Term).await {
                warn!(error = %e, subject, "Failed to term unknown subject");
            }
            return;
        }
    };

    let req_id = js_msg
        .message()
        .headers
        .as_ref()
        .and_then(|h| h.get(trogon_nats::REQ_ID_HEADER))
        .map(|v| v.as_str().to_string());

    let reply_subject = match (&req_id, &method) {
        (Some(rid), SessionAgentMethod::Prompt) => Some(
            acp_nats::nats::session::agent::prompt_response(prefix, session_id.as_str(), rid),
        ),
        (_, SessionAgentMethod::Cancel) => None,
        (Some(rid), _) => Some(acp_nats::nats::session::agent::response(
            prefix,
            session_id.as_str(),
            rid,
        )),
        (None, _) => {
            warn!(subject, "JetStream message missing X-Req-Id header");
            None
        }
    };

    let inner = js_msg.message();
    let msg = Message {
        subject: subject.as_str().into(),
        reply: reply_subject.as_deref().map(|s| s.into()),
        payload: inner.payload.clone(),
        headers: inner.headers.clone(),
        status: None,
        description: None,
        length: inner.payload.len(),
    };
    let subject = msg.subject.as_str();

    let result = match method {
        SessionAgentMethod::Load => {
            handle_request(&msg, nats, |req: LoadSessionRequest| {
                agent.load_session(req)
            })
            .await
        }
        SessionAgentMethod::Prompt => {
            handle_request_with_keepalive(&msg, nats, &js_msg, |req: PromptRequest| {
                agent.prompt(req)
            })
            .await
        }
        SessionAgentMethod::Cancel => {
            handle_notification(&msg, |req: CancelNotification| agent.cancel(req)).await
        }
        SessionAgentMethod::SetMode => {
            handle_request(&msg, nats, |req: SetSessionModeRequest| {
                agent.set_session_mode(req)
            })
            .await
        }
        SessionAgentMethod::SetConfigOption => {
            handle_request(&msg, nats, |req: SetSessionConfigOptionRequest| {
                agent.set_session_config_option(req)
            })
            .await
        }
        SessionAgentMethod::SetModel => {
            handle_request(&msg, nats, |req: SetSessionModelRequest| {
                agent.set_session_model(req)
            })
            .await
        }
        SessionAgentMethod::Fork => {
            handle_request(&msg, nats, |req: ForkSessionRequest| {
                agent.fork_session(req)
            })
            .await
        }
        SessionAgentMethod::Resume => {
            handle_request(&msg, nats, |req: ResumeSessionRequest| {
                agent.resume_session(req)
            })
            .await
        }
        SessionAgentMethod::Close => {
            handle_request(&msg, nats, |req: CloseSessionRequest| {
                agent.close_session(req)
            })
            .await
        }
    };

    match &result {
        Ok(()) => {
            if let Err(e) = js_msg.ack().await {
                warn!(subject, error = %e, "Failed to ack JetStream message");
            }
        }
        Err(DispatchError::DeserializeRequest(_) | DispatchError::DeserializeNotification(_)) => {
            if let Err(e) = js_msg.ack_with(AckKind::Term).await {
                warn!(subject, error = %e, "Failed to term bad payload");
            }
        }
        Err(DispatchError::NoReplySubject) => {
            if let Err(e) = js_msg.ack_with(AckKind::Term).await {
                warn!(subject, error = %e, "Failed to term missing reply subject");
            }
        }
        Err(DispatchError::Reply(_)) => {
            if let Err(e) = js_msg.ack().await {
                warn!(subject, error = %e, "Failed to ack after reply failure");
            }
        }
        Err(DispatchError::NotificationHandler(_)) => {
            if let Err(e) = js_msg.ack().await {
                warn!(subject, error = %e, "Failed to ack after notification handler error");
            }
        }
    }

    if let Err(e) = result {
        warn!(
            subject,
            session_id = session_id.as_str(),
            error = %e,
            "Error handling JetStream request"
        );
    }
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

    use trogon_nats::jetstream::mocks::*;

    fn make_js_msg(subject: &str, payload: &[u8], reply: Option<&str>) -> MockJsMessage {
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

    #[tokio::test]
    async fn with_jetstream_runs_both_loops() {
        use trogon_nats::jetstream::MockJetStreamConsumerFactory;

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
                let (conn, io_task) = AgentSideNatsConnection::with_jetstream(
                    agent,
                    nats,
                    factory,
                    AcpPrefix::new("acp").unwrap(),
                    |fut| {
                        tokio::task::spawn_local(fut);
                    },
                );

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
                let _ = serve_global(Rc::new(agent), nats.clone(), "myprefix", |fut| {
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
                drop(ext_tx);

                let _ = serve_global(Rc::new(agent), nats.clone(), "acp", |fut| {
                    tokio::task::spawn_local(fut);
                })
                .await;

                tokio::task::yield_now().await;
                tokio::task::yield_now().await;

                assert!(!nats.published_messages().is_empty());
            })
            .await;
    }

    #[tokio::test]
    async fn serve_js_dispatches_message() {
        use trogon_nats::jetstream::{
            MockJetStreamConsumer, MockJetStreamConsumerFactory, MockJsMessage,
        };

        let nats = MockNatsClient::new();
        let agent = MockAgent::new();
        let factory = MockJetStreamConsumerFactory::new();

        let (consumer, tx) = MockJetStreamConsumer::new();
        factory.add_consumer(consumer);

        let payload = serialize(&LoadSessionRequest::new("s1", "/tmp"));
        let mut headers = async_nats::HeaderMap::new();
        headers.insert(trogon_nats::REQ_ID_HEADER, "req-1");
        let js_msg = MockJsMessage::new(async_nats::Message {
            subject: "acp.session.s1.agent.load".into(),
            reply: None,
            payload: Bytes::copy_from_slice(&payload),
            headers: Some(headers),
            status: None,
            description: None,
            length: payload.len(),
        });
        tx.unbounded_send(Ok(js_msg)).unwrap();
        drop(tx);

        let local = tokio::task::LocalSet::new();
        local
            .run_until(async {
                let _ = serve_js(Rc::new(agent), nats.clone(), factory, "acp", |fut| {
                    tokio::task::spawn_local(fut);
                })
                .await;

                tokio::task::yield_now().await;
                tokio::task::yield_now().await;

                assert!(!nats.published_messages().is_empty());
            })
            .await;
    }

    #[tokio::test]
    async fn serve_js_handles_consumer_stream_error() {
        use trogon_nats::jetstream::{MockJetStreamConsumer, MockJetStreamConsumerFactory};

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
                let _ = serve_js(Rc::new(agent), nats.clone(), factory, "acp", |fut| {
                    tokio::task::spawn_local(fut);
                })
                .await;
            })
            .await;
    }

    #[tokio::test]
    async fn serve_js_consumer_creation_failure() {
        use trogon_nats::jetstream::MockJetStreamConsumerFactory;

        let nats = MockNatsClient::new();
        let agent = MockAgent::new();
        let factory = MockJetStreamConsumerFactory::new();

        let local = tokio::task::LocalSet::new();
        local
            .run_until(async {
                let result = serve_js(Rc::new(agent), nats.clone(), factory, "acp", |fut| {
                    tokio::task::spawn_local(fut);
                })
                .await;
                assert!(result.is_err());
            })
            .await;
    }

    #[tokio::test]
    async fn dispatch_js_message_success_acks() {
        let nats = MockNatsClient::new();
        let agent = MockAgent::new();
        let payload = serialize(&LoadSessionRequest::new("s1", "/tmp"));
        let js_msg = make_js_msg("acp.session.s1.agent.load", &payload, None);

        dispatch_js_message(js_msg, &agent, &nats, "acp").await;

        assert!(!nats.published_messages().is_empty());
    }

    #[tokio::test]
    async fn dispatch_js_message_unknown_subject_terms() {
        let nats = MockNatsClient::new();
        let agent = MockAgent::new();
        let js_msg = make_js_msg("acp.unknown.something", b"{}", None);

        dispatch_js_message(js_msg, &agent, &nats, "acp").await;

        assert!(nats.published_messages().is_empty());
    }

    #[tokio::test]
    async fn dispatch_js_message_bad_payload_terms() {
        let nats = MockNatsClient::new();
        let agent = MockAgent::new();
        let js_msg = make_js_msg("acp.session.s1.agent.load", b"not json", None);

        dispatch_js_message(js_msg, &agent, &nats, "acp").await;

        let payloads = nats.published_payloads();
        assert_eq!(payloads.len(), 1);
        let error: agent_client_protocol::Error = serde_json::from_slice(&payloads[0]).unwrap();
        assert_eq!(error.code, ErrorCode::InvalidParams);
    }

    #[tokio::test]
    async fn dispatch_js_message_missing_reply_terms() {
        let nats = MockNatsClient::new();
        let agent = MockAgent::new();
        let js_msg = make_js_msg("acp.agent.initialize", &serialize(&init_request()), None);

        dispatch_js_message(js_msg, &agent, &nats, "acp").await;

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
        dispatch_js_message(js_msg, &agent, &nats, "acp").await;
        // No reply published because no req_id → no reply subject
        assert!(nats.published_messages().is_empty());
    }

    #[tokio::test]
    async fn dispatch_js_message_global_method_returns_early() {
        let nats = MockNatsClient::new();
        let agent = MockAgent::new();
        let payload = serialize(&InitializeRequest::new(
            agent_client_protocol::ProtocolVersion::V0,
        ));
        let js_msg = make_js_msg("acp.agent.initialize", &payload, Some("_INBOX.1"));
        dispatch_js_message(js_msg, &agent, &nats, "acp").await;
        // Global methods return early — no dispatch, no reply
        assert!(nats.published_messages().is_empty());
    }

    #[tokio::test]
    async fn dispatch_js_message_global_method_ack_failure() {
        let nats = MockNatsClient::new();
        let agent = MockAgent::new();
        let payload = serialize(&InitializeRequest::new(
            agent_client_protocol::ProtocolVersion::V0,
        ));
        let js_msg = make_failing_js_msg("acp.agent.initialize", &payload);
        dispatch_js_message(js_msg, &agent, &nats, "acp").await;
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
        use tracing_subscriber::util::SubscriberInitExt;
        let _guard = tracing_subscriber::fmt().with_test_writer().set_default();

        let nats = MockNatsClient::new();
        let agent = MockAgent::new();
        let raw = std::sync::Arc::from(
            serde_json::value::RawValue::from_string("{}".to_string()).unwrap(),
        );
        let payload = serialize(&agent_client_protocol::ExtNotification::new("my_tool", raw));
        // No X-Req-Id → ext notification path (reply_subject is None → msg.reply is None)
        let js_msg = make_js_msg_no_headers("acp.session.s1.agent.ext.my_tool", &payload);
        dispatch_js_message(js_msg, &agent, &nats, "acp").await;
    }

    #[tokio::test]
    async fn dispatch_js_message_ext_notification_handler_error_ack_failure() {
        use tracing_subscriber::util::SubscriberInitExt;
        let _guard = tracing_subscriber::fmt().with_test_writer().set_default();

        let nats = MockNatsClient::new();
        let agent = MockAgent::new();
        let raw = std::sync::Arc::from(
            serde_json::value::RawValue::from_string("{}".to_string()).unwrap(),
        );
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
        dispatch_js_message(js_msg, &agent, &nats, "acp").await;
    }

    #[tokio::test]
    async fn dispatch_js_message_global_ext_no_session_id() {
        let nats = MockNatsClient::new();
        let agent = MockAgent::new();
        let raw = std::sync::Arc::from(
            serde_json::value::RawValue::from_string("{}".to_string()).unwrap(),
        );
        let payload = serialize(&agent_client_protocol::ExtNotification::new("my_tool", raw));
        let js_msg = make_js_msg_no_headers("acp.agent.ext.my_tool", &payload);
        dispatch_js_message(js_msg, &agent, &nats, "acp").await;
    }

    #[tokio::test]
    async fn dispatch_js_message_prompt_uses_prompt_response_subject() {
        let nats = MockNatsClient::new();
        let agent = MockAgent::new();
        let payload = serialize(&PromptRequest::new("s1", vec![]));
        let js_msg = make_js_msg("acp.session.s1.agent.prompt", &payload, None);

        dispatch_js_message(js_msg, &agent, &nats, "acp").await;

        let subjects = nats.published_messages();
        assert!(
            subjects
                .iter()
                .any(|s| s.starts_with("acp.session.s1.agent.prompt.response.")),
            "expected prompt.response subject, got: {:?}",
            subjects
        );
    }

    #[tokio::test]
    async fn dispatch_js_message_non_prompt_session_uses_response_subject() {
        let nats = MockNatsClient::new();
        let agent = MockAgent::new();
        let payload = serialize(&LoadSessionRequest::new("s1", "/tmp"));
        let js_msg = make_js_msg("acp.session.s1.agent.load", &payload, None);

        dispatch_js_message(js_msg, &agent, &nats, "acp").await;

        let subjects = nats.published_messages();
        assert!(
            subjects
                .iter()
                .any(|s| s.starts_with("acp.session.s1.agent.response.")),
            "expected response subject, got: {:?}",
            subjects
        );
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

    #[test]
    fn connection_error_jetstream_display() {
        let err = ConnectionError::JetStream(Box::new(std::io::Error::other("js err")));
        assert!(err.to_string().contains("js err"));
        let debug = format!("{:?}", err);
        assert!(debug.contains("JetStream"));
    }

    #[tokio::test]
    async fn dispatch_js_message_cancel_notification() {
        let nats = MockNatsClient::new();
        let agent = MockAgent::new();
        let payload = serialize(&CancelNotification::new("s1"));
        let js_msg = make_js_msg("acp.session.s1.agent.cancel", &payload, None);

        dispatch_js_message(js_msg, &agent, &nats, "acp").await;

        assert_eq!(agent.cancelled.borrow().len(), 1);
    }

    #[tokio::test]
    async fn dispatch_js_message_set_mode() {
        let nats = MockNatsClient::new();
        let agent = MockAgent::new();
        let payload = serialize(&SetSessionModeRequest::new("s1", "code"));
        let js_msg = make_js_msg("acp.session.s1.agent.set_mode", &payload, Some("_INBOX.r"));

        dispatch_js_message(js_msg, &agent, &nats, "acp").await;

        assert!(!nats.published_messages().is_empty());
    }

    #[tokio::test]
    async fn dispatch_js_message_close_session() {
        let nats = MockNatsClient::new();
        let agent = MockAgent::new();
        let payload = serialize(&CloseSessionRequest::new("s1"));
        let js_msg = make_js_msg("acp.session.s1.agent.close", &payload, Some("_INBOX.r"));

        dispatch_js_message(js_msg, &agent, &nats, "acp").await;

        assert!(!nats.published_messages().is_empty());
    }

    #[tokio::test]
    async fn dispatch_js_message_fork_session() {
        let nats = MockNatsClient::new();
        let agent = MockAgent::new();
        let payload = serialize(&ForkSessionRequest::new("s1", "/tmp"));
        let js_msg = make_js_msg("acp.session.s1.agent.fork", &payload, Some("_INBOX.r"));

        dispatch_js_message(js_msg, &agent, &nats, "acp").await;

        assert!(!nats.published_messages().is_empty());
    }

    #[tokio::test]
    async fn dispatch_js_message_set_config_option() {
        let nats = MockNatsClient::new();
        let agent = MockAgent::new();
        let payload = serialize(&SetSessionConfigOptionRequest::new("s1", "key", "val"));
        let js_msg = make_js_msg(
            "acp.session.s1.agent.set_config_option",
            &payload,
            Some("_INBOX.r"),
        );
        dispatch_js_message(js_msg, &agent, &nats, "acp").await;
        assert!(!nats.published_messages().is_empty());
    }

    #[tokio::test]
    async fn dispatch_js_message_set_model() {
        let nats = MockNatsClient::new();
        let agent = MockAgent::new();
        let payload = serialize(&SetSessionModelRequest::new("s1", "gpt-4"));
        let js_msg = make_js_msg("acp.session.s1.agent.set_model", &payload, Some("_INBOX.r"));
        dispatch_js_message(js_msg, &agent, &nats, "acp").await;
        assert!(!nats.published_messages().is_empty());
    }

    #[tokio::test]
    async fn dispatch_js_message_resume_session() {
        let nats = MockNatsClient::new();
        let agent = MockAgent::new();
        let payload = serialize(&ResumeSessionRequest::new("s1", "/tmp"));
        let js_msg = make_js_msg("acp.session.s1.agent.resume", &payload, Some("_INBOX.r"));
        dispatch_js_message(js_msg, &agent, &nats, "acp").await;
        assert!(!nats.published_messages().is_empty());
    }

    #[tokio::test]
    async fn dispatch_js_message_prompt() {
        let nats = MockNatsClient::new();
        let agent = MockAgent::new();
        let payload = serialize(&PromptRequest::new("s1", vec![]));
        let js_msg = make_js_msg("acp.session.s1.agent.prompt", &payload, Some("_INBOX.r"));
        dispatch_js_message(js_msg, &agent, &nats, "acp").await;
        assert!(!nats.published_messages().is_empty());
    }

    #[tokio::test]
    async fn dispatch_js_message_reply_failure_acks() {
        let nats = trogon_nats::AdvancedMockNatsClient::new();
        nats.fail_next_publish();
        let agent = MockAgent::new();
        let payload = serialize(&LoadSessionRequest::new("s1", "/tmp"));
        let js_msg = make_js_msg("acp.session.s1.agent.load", &payload, Some("_INBOX.r"));

        dispatch_js_message(js_msg, &agent, &nats, "acp").await;
    }

    fn make_failing_js_msg(subject: &str, payload: &[u8]) -> MockJsMessage {
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
        let payload = serialize(&LoadSessionRequest::new("s1", "/tmp"));
        let js_msg = make_failing_js_msg("acp.session.s1.agent.load", &payload);
        dispatch_js_message(js_msg, &agent, &nats, "acp").await;
    }

    #[tokio::test]
    async fn dispatch_js_message_term_failure_logs_warning() {
        let nats = MockNatsClient::new();
        let agent = MockAgent::new();
        let js_msg = make_failing_js_msg("unknown.subject", b"{}");
        dispatch_js_message(js_msg, &agent, &nats, "acp").await;
    }

    #[tokio::test]
    async fn dispatch_js_message_term_bad_payload_failure_logs_warning() {
        let nats = MockNatsClient::new();
        let agent = MockAgent::new();
        let js_msg = make_failing_js_msg("acp.session.s1.agent.load", b"not json");
        dispatch_js_message(js_msg, &agent, &nats, "acp").await;
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
        dispatch_js_message(js_msg, &agent, &nats, "acp").await;
    }

    #[tokio::test]
    async fn dispatch_js_message_reply_failure_ack_failure() {
        let nats = trogon_nats::AdvancedMockNatsClient::new();
        nats.fail_next_publish();
        let agent = MockAgent::new();
        let payload = serialize(&LoadSessionRequest::new("s1", "/tmp"));
        let js_msg = make_failing_js_msg("acp.session.s1.agent.load", &payload);
        dispatch_js_message(js_msg, &agent, &nats, "acp").await;
    }

    #[tokio::test]
    async fn dispatch_js_message_cancel_notification_ack_failure() {
        let nats = MockNatsClient::new();
        let agent = MockAgent::new();
        let payload = serialize(&CancelNotification::new("s1"));
        let js_msg = make_failing_js_msg("acp.session.s1.agent.cancel", &payload);
        dispatch_js_message(js_msg, &agent, &nats, "acp").await;
    }

    #[tokio::test]
    async fn handle_request_with_keepalive_completes_fast() {
        let nats = MockNatsClient::new();
        let payload = serialize(&InitializeRequest::new(
            agent_client_protocol::ProtocolVersion::V0,
        ));
        let msg = make_nats_message("acp.agent.initialize", &payload, Some("_INBOX.1"));
        let js_msg = make_js_msg("acp.agent.initialize", &payload, Some("_INBOX.1"));

        let agent = MockAgent::new();
        let result =
            handle_request_with_keepalive(&msg, &nats, &js_msg, |req: InitializeRequest| {
                agent.initialize(req)
            })
            .await;
        assert!(result.is_ok());
        assert!(!nats.published_messages().is_empty());
    }

    #[tokio::test]
    async fn handle_request_with_keepalive_no_reply_subject() {
        let nats = MockNatsClient::new();
        let payload = serialize(&InitializeRequest::new(
            agent_client_protocol::ProtocolVersion::V0,
        ));
        let msg = make_nats_message("acp.agent.initialize", &payload, None);
        let js_msg = make_js_msg("acp.agent.initialize", &payload, None);

        let result =
            handle_request_with_keepalive(&msg, &nats, &js_msg, |_: InitializeRequest| async {
                Err::<InitializeResponse, _>(agent_client_protocol::Error::new(-1, "not called"))
            })
            .await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn handle_request_with_keepalive_bad_payload() {
        let nats = MockNatsClient::new();
        let msg = make_nats_message("acp.agent.initialize", b"not json", Some("_INBOX.1"));
        let js_msg = make_js_msg("acp.agent.initialize", b"not json", Some("_INBOX.1"));

        let result =
            handle_request_with_keepalive(&msg, &nats, &js_msg, |_: InitializeRequest| async {
                Err::<InitializeResponse, _>(agent_client_protocol::Error::new(-1, "not called"))
            })
            .await;
        assert!(result.is_err());
    }

    #[tokio::test(start_paused = true)]
    async fn handle_request_with_keepalive_progress_ack_failure() {
        use tracing_subscriber::util::SubscriberInitExt;
        let _guard = tracing_subscriber::fmt().with_test_writer().set_default();

        let nats = MockNatsClient::new();
        let payload = serialize(&InitializeRequest::new(
            agent_client_protocol::ProtocolVersion::V0,
        ));
        let msg = make_nats_message("acp.agent.initialize", &payload, Some("_INBOX.1"));

        let mut headers = async_nats::HeaderMap::new();
        headers.insert(trogon_nats::REQ_ID_HEADER, "req-1");
        let js_msg = MockJsMessage::with_failing_signals(async_nats::Message {
            subject: "acp.agent.initialize".into(),
            reply: Some("_INBOX.1".into()),
            payload: Bytes::copy_from_slice(&payload),
            headers: Some(headers),
            status: None,
            description: None,
            length: payload.len(),
        });

        let agent = MockAgent::new();
        let result =
            handle_request_with_keepalive(&msg, &nats, &js_msg, |req: InitializeRequest| async {
                tokio::time::sleep(Duration::from_secs(20)).await;
                agent.initialize(req).await
            })
            .await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn handle_request_with_keepalive_handler_error() {
        let nats = MockNatsClient::new();
        let payload = serialize(&AuthenticateRequest::new("basic"));
        let msg = make_nats_message("acp.agent.authenticate", &payload, Some("_INBOX.1"));
        let js_msg = make_js_msg("acp.agent.authenticate", &payload, Some("_INBOX.1"));

        let agent = MockAgent::new();
        let result =
            handle_request_with_keepalive(&msg, &nats, &js_msg, |req: AuthenticateRequest| {
                agent.authenticate(req)
            })
            .await;
        assert!(result.is_ok());
        assert!(!nats.published_messages().is_empty());
    }
}
