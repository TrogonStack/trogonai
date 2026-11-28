use acp_nats::jetstream::consumers::{COMMANDS_ACK_WAIT, agent_commands_consumer};
use acp_nats::jetstream::streams::commands_stream_name;
use acp_nats::nats::subscriptions::{AllAgentExtSubject, AllAgentSubject, GlobalAllSubject};
use acp_nats::nats::{GlobalAgentMethod, ParsedAgentSubject, SessionAgentMethod, parse_agent_subject};
use acp_nats::wire::{
    encode_agent_error, encode_success, req_id_from_request_headers, response_id_from_request_headers,
};
use acp_nats::{AcpPrefix, AcpSessionId, AgentHandler, NatsClientProxy, ReqId, ResponseSubject};
use agent_client_protocol::v1::schema::{
    ArchiveSessionRequest, AuthStatusRequest, AuthenticateRequest, CancelNotification, CloseSessionRequest,
    DeleteSessionRequest, DisableProviderRequest, ExtNotification, ExtRequest, ForkSessionRequest, InitializeRequest,
    ListProvidersRequest, ListSessionsRequest, LoadSessionRequest, LogoutRequest, NewSessionRequest, PromptRequest,
    ResumeSessionRequest, SessionId, SessionStatusRequest, SetProviderRequest, SetSessionConfigOptionRequest,
    SetSessionModeRequest, SetSessionTitleRequest, UnarchiveSessionRequest,
};
use async_nats::Message;
use async_nats::jetstream::AckKind;
use async_nats::jetstream::context::GetStreamError;
#[cfg(test)]
use bytes::Bytes;
use futures::StreamExt;
use futures::future::{Either, LocalBoxFuture, Ready, ready};
use serde::de::Error;
use std::cell::RefCell;
use std::collections::HashSet;
use serde_json::value::RawValue;
use std::rc::Rc;
use std::sync::Arc;
use std::time::Duration;
use tracing::{info, warn};
use trogon_nats::jetstream::{
    ConsumerLoss, JetStreamConsumer, JetStreamCreateConsumer, JetStreamGetStream, JsAckWith, JsDispatchMessage,
    JsMessageRef, is_get_stream_not_found,
};
use trogon_nats::{FlushClient, PublishClient, RequestClient, SubscribeClient};

#[derive(Debug, thiserror::Error)]
pub enum ConnectionError {
    #[error("failed to subscribe: {0}")]
    Subscribe(#[source] Box<dyn std::error::Error + Send + Sync>),
    #[error("jetstream error: {0}")]
    JetStream(#[source] Box<dyn std::error::Error + Send + Sync>),
    #[error("stream {stream} no longer exists; restart the host to provision it again")]
    CommandsStreamMissing { stream: String },
}

#[derive(Debug, thiserror::Error)]
enum DispatchError {
    #[error("no reply subject")]
    NoReplySubject,
    #[error("deserialize request: {0}")]
    DeserializeRequest(#[source] serde_json::Error),
    #[error("deserialize notification: {0}")]
    DeserializeNotification(#[source] serde_json::Error),
    #[error("reply: {0}")]
    Reply(#[source] trogon_nats::NatsError),
    #[error("notification handler: {0}")]
    NotificationHandler(#[source] agent_client_protocol::Error),
    #[error("jsonrpc wire: {0}")]
    Wire(#[from] acp_nats::wire::WireError),
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
        agent: impl AgentHandler + Sync + 'static,
        nats: N,
        acp_prefix: AcpPrefix,
        spawn: impl Fn(LocalBoxFuture<'static, ()>) + 'static,
    ) -> (Self, impl std::future::Future<Output = Result<(), ConnectionError>>) {
        let nats_for_serve = nats.clone();
        let prefix = acp_prefix.clone();

        let io_task = async move { serve(agent, nats_for_serve, &prefix, spawn).await };

        let conn = Self {
            nats,
            acp_prefix,
            operation_timeout: DEFAULT_OPERATION_TIMEOUT,
        };
        (conn, io_task)
    }

    pub fn with_jetstream<J>(
        agent: impl AgentHandler + Sync + 'static,
        nats: N,
        js: J,
        acp_prefix: AcpPrefix,
        spawn: impl Fn(LocalBoxFuture<'static, ()>) + Copy + 'static,
    ) -> (Self, impl std::future::Future<Output = Result<(), ConnectionError>>)
    where
        J: JetStreamGetStream<Error = GetStreamError> + 'static,
        trogon_nats::jetstream::JsMessageOf<J>: JsDispatchMessage,
        CommandsMessagesError<J>: ConsumerLoss,
    {
        let nats_for_serve = nats.clone();
        let nats_for_js = nats.clone();
        let prefix = acp_prefix.clone();
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
    prefix: &AcpPrefix,
    spawn: impl Fn(LocalBoxFuture<'static, ()>) + 'static,
) -> Result<(), ConnectionError>
where
    N: SubscribeClient + PublishClient + FlushClient + Clone + 'static,
    A: AgentHandler + Sync + 'static,
{
    let global_wildcard = GlobalAllSubject::new(prefix);
    let session_wildcard = AllAgentSubject::new(prefix);

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
    prefix: &AcpPrefix,
    spawn: impl Fn(LocalBoxFuture<'static, ()>) + 'static,
) -> Result<(), ConnectionError>
where
    N: SubscribeClient + PublishClient + FlushClient + Clone + 'static,
    A: AgentHandler + Sync + 'static,
{
    let global_wildcard = GlobalAllSubject::new(prefix);
    let ext_wildcard = AllAgentExtSubject::new(prefix);

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

async fn dispatch_message<N: PublishClient + FlushClient, A: AgentHandler + Sync>(msg: Message, agent: &A, nats: &N) {
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
            let r = dispatch_session(method, &session_id, &msg, agent, nats).await;
            (r, Some(session_id))
        }
    };

    if let Err(e) = result {
        let sid = session_id.as_ref().map(|s| s.as_str()).unwrap_or("-");
        warn!(subject, session_id = sid, error = %e, "Error handling agent request");
    }
}

async fn dispatch_global<N: PublishClient + FlushClient, A: AgentHandler + Sync>(
    method: GlobalAgentMethod,
    msg: &Message,
    agent: &A,
    nats: &N,
) -> Result<(), DispatchError> {
    match method {
        GlobalAgentMethod::Initialize => {
            handle_jsonrpc_request(
                msg,
                method.protocol_method().as_str(),
                nats,
                |req: InitializeRequest| agent.initialize(req),
            )
            .await
        }
        GlobalAgentMethod::Authenticate => {
            handle_jsonrpc_request(
                msg,
                method.protocol_method().as_str(),
                nats,
                |req: AuthenticateRequest| agent.authenticate(req),
            )
            .await
        }
        GlobalAgentMethod::AuthStatus => {
            handle_jsonrpc_request(
                msg,
                method.protocol_method().as_str(),
                nats,
                |req: AuthStatusRequest| agent.auth_status(req),
            )
            .await
        }
        GlobalAgentMethod::Logout => {
            handle_jsonrpc_request(msg, method.protocol_method().as_str(), nats, |req: LogoutRequest| {
                agent.logout(req)
            })
            .await
        }
        GlobalAgentMethod::SessionNew => {
            handle_jsonrpc_request(
                msg,
                method.protocol_method().as_str(),
                nats,
                |req: NewSessionRequest| agent.new_session(req),
            )
            .await
        }
        GlobalAgentMethod::SessionList => {
            handle_jsonrpc_request(
                msg,
                method.protocol_method().as_str(),
                nats,
                |req: ListSessionsRequest| agent.list_sessions(req),
            )
            .await
        }
        GlobalAgentMethod::ProvidersList => {
            handle_jsonrpc_request(
                msg,
                method.protocol_method().as_str(),
                nats,
                |req: ListProvidersRequest| agent.list_providers(req),
            )
            .await
        }
        GlobalAgentMethod::ProvidersSet => {
            handle_jsonrpc_request(
                msg,
                method.protocol_method().as_str(),
                nats,
                |req: SetProviderRequest| agent.set_provider(req),
            )
            .await
        }
        GlobalAgentMethod::ProvidersDisable => {
            handle_jsonrpc_request(
                msg,
                method.protocol_method().as_str(),
                nats,
                |req: DisableProviderRequest| agent.disable_provider(req),
            )
            .await
        }
        GlobalAgentMethod::Ext(ref name) => {
            // `method` is `#[serde(skip)]` in the ACP schema, so the body carries only
            // the params; the subject token carries the exact name.
            if msg.reply.is_some() {
                handle_jsonrpc_request(msg, &name.protocol_method(), nats, |params: Arc<RawValue>| {
                    agent.ext_method(ExtRequest::new(name.as_str(), params))
                })
                .await
            } else {
                handle_notification(msg, |params: Arc<RawValue>| {
                    agent.ext_notification(ExtNotification::new(name.as_str(), params))
                })
                .await
            }
        }
    }
}

async fn dispatch_session<N: PublishClient + FlushClient, A: AgentHandler + Sync>(
    method: SessionAgentMethod,
    session_id: &AcpSessionId,
    msg: &Message,
    agent: &A,
    nats: &N,
) -> Result<(), DispatchError> {
    match method {
        SessionAgentMethod::Load => {
            handle_jsonrpc_request(
                msg,
                method.protocol_method(),
                nats,
                in_session(session_id, |req: LoadSessionRequest| agent.load_session(req)),
            )
            .await
        }
        SessionAgentMethod::Prompt => {
            handle_jsonrpc_request(
                msg,
                method.protocol_method(),
                nats,
                in_session(session_id, |req: PromptRequest| agent.prompt(req)),
            )
            .await
        }
        SessionAgentMethod::Cancel => {
            handle_wire_notification(
                msg,
                method.protocol_method(),
                in_session(session_id, |req: CancelNotification| agent.cancel(req)),
            )
            .await
        }
        SessionAgentMethod::SetMode => {
            handle_jsonrpc_request(
                msg,
                method.protocol_method(),
                nats,
                in_session(session_id, |req: SetSessionModeRequest| agent.set_session_mode(req)),
            )
            .await
        }
        SessionAgentMethod::SetConfigOption => {
            handle_jsonrpc_request(
                msg,
                method.protocol_method(),
                nats,
                in_session(session_id, |req: SetSessionConfigOptionRequest| {
                    agent.set_session_config_option(req)
                }),
            )
            .await
        }
        SessionAgentMethod::Fork => {
            handle_jsonrpc_request(
                msg,
                method.protocol_method(),
                nats,
                in_session(session_id, |req: ForkSessionRequest| agent.fork_session(req)),
            )
            .await
        }
        SessionAgentMethod::Resume => {
            handle_jsonrpc_request(
                msg,
                method.protocol_method(),
                nats,
                in_session(session_id, |req: ResumeSessionRequest| agent.resume_session(req)),
            )
            .await
        }
        SessionAgentMethod::Close => {
            handle_jsonrpc_request(
                msg,
                method.protocol_method(),
                nats,
                in_session(session_id, |req: CloseSessionRequest| agent.close_session(req)),
            )
            .await
        }
        SessionAgentMethod::Archive => {
            handle_jsonrpc_request(
                msg,
                method.protocol_method(),
                nats,
                in_session(session_id, |req: ArchiveSessionRequest| agent.archive_session(req)),
            )
            .await
        }
        SessionAgentMethod::Unarchive => {
            handle_jsonrpc_request(
                msg,
                method.protocol_method(),
                nats,
                in_session(session_id, |req: UnarchiveSessionRequest| agent.unarchive_session(req)),
            )
            .await
        }
        SessionAgentMethod::Status => {
            handle_jsonrpc_request(
                msg,
                method.protocol_method(),
                nats,
                in_session(session_id, |req: SessionStatusRequest| agent.session_status(req)),
            )
            .await
        }
        SessionAgentMethod::SetTitle => {
            handle_jsonrpc_request(
                msg,
                method.protocol_method(),
                nats,
                in_session(session_id, |req: SetSessionTitleRequest| agent.set_session_title(req)),
            )
            .await
        }
        SessionAgentMethod::Delete => {
            handle_jsonrpc_request(
                msg,
                method.protocol_method(),
                nats,
                in_session(session_id, |req: DeleteSessionRequest| agent.delete_session(req)),
            )
            .await
        }
    }
}

/// Requests and notifications that name the session they act on in their body.
trait SessionScoped {
    fn session_id(&self) -> &SessionId;
}

macro_rules! session_scoped {
    ($($request:ty),+ $(,)?) => {
        $(impl SessionScoped for $request {
            fn session_id(&self) -> &SessionId {
                &self.session_id
            }
        })+
    };
}

session_scoped!(
    LoadSessionRequest,
    PromptRequest,
    CancelNotification,
    SetSessionModeRequest,
    SetSessionConfigOptionRequest,
    ForkSessionRequest,
    ResumeSessionRequest,
    CloseSessionRequest,
    ArchiveSessionRequest,
    UnarchiveSessionRequest,
    SessionStatusRequest,
    SetSessionTitleRequest,
    DeleteSessionRequest,
);

/// Runs `handler` only when the body's `sessionId` is the session the subject
/// names. The subject is what NATS permissions authorize, so a body naming
/// another session is refused with invalid params instead of acting on it.
fn in_session<'a, ReqT, Resp, F>(
    subject_session: &'a AcpSessionId,
    handler: impl FnOnce(ReqT) -> F + 'a,
) -> impl FnOnce(ReqT) -> Either<F, Ready<agent_client_protocol::Result<Resp>>> + 'a
where
    ReqT: SessionScoped,
    F: std::future::Future<Output = agent_client_protocol::Result<Resp>>,
{
    move |request| {
        let body_session = request.session_id();
        if body_session.0.as_ref() == subject_session.as_str() {
            return Either::Left(handler(request));
        }
        warn!(
            subject_session_id = subject_session.as_str(),
            body_session_id = %body_session,
            "Rejecting session request whose body names another session"
        );
        Either::Right(ready(Err(agent_client_protocol::Error::new(
            agent_client_protocol::ErrorCode::INVALID_PARAMS,
            format!("params.sessionId ({body_session}) does not match subject session id ({subject_session})"),
        ))))
    }
}

async fn reply_wire<N: PublishClient + FlushClient>(
    nats: &N,
    reply_to: &str,
    encoded: jsonrpc_nats::Encoded,
) -> Result<(), DispatchError>
where
    N::PublishError: std::error::Error + Send + Sync + 'static,
    N::FlushError: std::error::Error + Send + Sync + 'static,
{
    nats.publish_with_headers(reply_to.to_string(), encoded.headers, encoded.body)
        .await
        .map_err(|error| DispatchError::Reply(trogon_nats::NatsError::Other(error.to_string())))?;
    nats.flush()
        .await
        .map_err(|error| DispatchError::Reply(trogon_nats::NatsError::Other(error.to_string())))
}

async fn handle_jsonrpc_request<N, Resp, ReqT, F>(
    msg: &Message,
    method: &str,
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
    let headers = msg.headers.clone().unwrap_or_default();

    let request: ReqT = match acp_nats::wire::decode_request_params(method, &headers, &msg.payload) {
        Ok(req) => req,
        Err(e) => {
            let error = agent_client_protocol::Error::new(
                agent_client_protocol::ErrorCode::INVALID_PARAMS,
                format!("Failed to deserialize request: {e}"),
            );
            let id = response_id_from_request_headers(&headers);
            let encoded = encode_agent_error(id, &error);
            let _ = reply_wire(nats, reply_to, encoded).await;
            return Err(DispatchError::DeserializeRequest(serde_json::Error::custom(format!(
                "{e}"
            ))));
        }
    };

    let response_id = response_id_from_request_headers(&headers);
    match handler(request).await {
        Ok(resp) => {
            let encoded = encode_success(response_id, &resp)?;
            reply_wire(nats, reply_to, encoded).await
        }
        Err(err) => {
            let encoded = encode_agent_error(response_id, &err);
            reply_wire(nats, reply_to, encoded).await
        }
    }
}

async fn handle_wire_notification<ReqT, F>(
    msg: &Message,
    method: &str,
    handler: impl FnOnce(ReqT) -> F,
) -> Result<(), DispatchError>
where
    ReqT: serde::de::DeserializeOwned,
    F: std::future::Future<Output = agent_client_protocol::Result<()>>,
{
    let headers = msg.headers.clone().unwrap_or_default();
    let request: ReqT = acp_nats::wire::decode_notification_params(method, &headers, &msg.payload)
        .map_err(|error| DispatchError::DeserializeNotification(serde_json::Error::custom(format!("{error}"))))?;

    handler(request).await.map_err(DispatchError::NotificationHandler)
}

async fn handle_notification<ReqT, F>(msg: &Message, handler: impl FnOnce(ReqT) -> F) -> Result<(), DispatchError>
where
    ReqT: serde::de::DeserializeOwned,
    F: std::future::Future<Output = agent_client_protocol::Result<()>>,
{
    let request: ReqT = serde_json::from_slice(&msg.payload).map_err(DispatchError::DeserializeNotification)?;

    handler(request).await.map_err(DispatchError::NotificationHandler)
}

async fn handle_jsonrpc_request_with_keepalive<N, Resp, ReqT, F, M>(
    msg: &Message,
    method: &str,
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
    let headers = msg.headers.clone().unwrap_or_default();

    let request: ReqT = match acp_nats::wire::decode_request_params(method, &headers, &msg.payload) {
        Ok(req) => req,
        Err(e) => {
            let error = agent_client_protocol::Error::new(
                agent_client_protocol::ErrorCode::INVALID_PARAMS,
                format!("Failed to deserialize request: {e}"),
            );
            let id = response_id_from_request_headers(&headers);
            let encoded = encode_agent_error(id, &error);
            let _ = reply_wire(nats, reply_to, encoded).await;
            return Err(DispatchError::DeserializeRequest(serde_json::Error::custom(format!(
                "{e}"
            ))));
        }
    };

    let response_id = response_id_from_request_headers(&headers);
    let handler_fut = handler(request);
    tokio::pin!(handler_fut);

    let mut keepalive = tokio::time::interval(KEEPALIVE_INTERVAL);
    keepalive.tick().await;

    loop {
        tokio::select! {
            result = &mut handler_fut => {
                return match result {
                    Ok(resp) => {
                        let encoded = encode_success(response_id, &resp)?;
                        reply_wire(nats, reply_to, encoded).await
                    }
                    Err(err) => {
                        let encoded = encode_agent_error(response_id, &err);
                        reply_wire(nats, reply_to, encoded).await
                    }
                };
            }
            _ = keepalive.tick() => {
                let _ = js_msg.ack_with(AckKind::Progress).await
                    .inspect_err(|e| warn!(error = %e, "Failed to send in_progress keepalive"));
            }
        }
    }
}

async fn serve_js<N, J, A>(
    agent: Rc<A>,
    nats: N,
    js: J,
    prefix: &AcpPrefix,
    spawn: impl Fn(LocalBoxFuture<'static, ()>) + 'static,
) -> Result<(), ConnectionError>
where
    N: PublishClient + FlushClient + Clone + 'static,
    J: JetStreamGetStream<Error = GetStreamError> + 'static,
    trogon_nats::jetstream::JsMessageOf<J>: JsDispatchMessage,
    CommandsMessagesError<J>: ConsumerLoss,
    A: AgentHandler + Sync + 'static,
{
    let stream_name = commands_stream_name(prefix);

    info!(stream = %stream_name, "Starting JetStream consumer for COMMANDS stream");

    let mut messages = subscribe_commands(&js, &stream_name).await?;

    let nats = Rc::new(nats);
    let prefix = Rc::new(prefix.clone());
    let in_flight = InFlightCommands::default();

    while let Some(msg_result) = messages.next().await {
        match msg_result {
            Ok(js_msg) => {
                let Ok(claim) = in_flight.claim(js_msg.message()) else {
                    // JetStream redelivered a command this host is still running
                    // (its acks were lost, e.g. in a partition). Running it again
                    // would answer the same request id twice; restarting its
                    // ack_wait leaves the original to settle it.
                    info!(subject = %js_msg.message().subject, "Command redelivered while still in flight; not dispatching it again");
                    let _ = js_msg
                        .ack_with(AckKind::Progress)
                        .await
                        .inspect_err(|e| warn!(error = %e, "Failed to mark redelivered command in progress"));
                    continue;
                };
                let agent = agent.clone();
                let nats = nats.clone();
                let prefix = prefix.clone();
                spawn(Box::pin(async move {
                    dispatch_js_message(js_msg, agent.as_ref(), nats.as_ref(), &prefix).await;
                    drop(claim);
                }));
            }
            Err(e) if e.consumer_lost() => {
                warn!(error = %e, "COMMANDS consumer is gone; recreating it");
                messages = resubscribe_commands(&js, &stream_name).await?;
            }
            Err(e) => {
                warn!(error = %e, "JetStream consumer error");
            }
        }
    }

    info!("JetStream COMMANDS consumer ended");
    Ok(())
}

type CommandsConsumer<J> = <<J as JetStreamGetStream>::Stream as JetStreamCreateConsumer>::Consumer;
type CommandsMessages<J> = <CommandsConsumer<J> as JetStreamConsumer>::Messages;
/// The error the COMMANDS consumer's message stream yields for a JetStream
/// client `J`. The JetStream constructors bound it by `ConsumerLoss`, so
/// generic callers name it in their own bounds.
pub type CommandsMessagesError<J> = <CommandsConsumer<J> as JetStreamConsumer>::MessagesError;

/// Opens the stream, creates (or resumes) the durable consumer and starts
/// pulling from it.
async fn subscribe_commands<J>(js: &J, stream_name: &str) -> Result<CommandsMessages<J>, ConnectionError>
where
    J: JetStreamGetStream<Error = GetStreamError>,
{
    let stream = js.get_stream(stream_name).await.map_err(|e| {
        if is_get_stream_not_found(&e) {
            ConnectionError::CommandsStreamMissing {
                stream: stream_name.to_owned(),
            }
        } else {
            ConnectionError::JetStream(Box::new(e))
        }
    })?;

    let consumer = stream
        .create_consumer(agent_commands_consumer())
        .await
        .map_err(|e| ConnectionError::JetStream(Box::new(e)))?;

    consumer
        .messages()
        .await
        .map_err(|e| ConnectionError::JetStream(Box::new(e)))
}

/// Recreates the consumer after it was lost.
///
/// The durable is recreated in place, since its config lives here. The stream
/// is not: its subjects, retention and limits are provisioned by the host at
/// startup, together with the other ACP streams, which were most likely lost
/// with it. So a missing stream ends serving with
/// [`ConnectionError::CommandsStreamMissing`], and the supervisor's restart
/// provisions everything again.
///
/// Other failures (NATS unreachable) are retried with backoff for up to
/// [`RESUBSCRIBE_DEADLINE`], then serving ends so the supervisor restarts the
/// host.
async fn resubscribe_commands<J>(js: &J, stream_name: &str) -> Result<CommandsMessages<J>, ConnectionError>
where
    J: JetStreamGetStream<Error = GetStreamError>,
{
    let deadline = tokio::time::Instant::now() + RESUBSCRIBE_DEADLINE;
    let mut delay = RESUBSCRIBE_MIN_DELAY;
    loop {
        match subscribe_commands(js, stream_name).await {
            Ok(messages) => {
                info!(stream = stream_name, "Recreated the COMMANDS consumer");
                return Ok(messages);
            }
            Err(e @ ConnectionError::JetStream(_)) if tokio::time::Instant::now() + delay < deadline => {
                warn!(error = %e, retry_in = ?delay, "Failed to recreate the COMMANDS consumer");
                tokio::time::sleep(delay).await;
                delay = (delay * 2).min(RESUBSCRIBE_MAX_DELAY);
            }
            Err(e) => return Err(e),
        }
    }
}

/// How long to keep retrying to recreate a lost consumer. Tied to
/// [`COMMANDS_ACK_WAIT`]: past it, JetStream has redelivered every command this
/// host held, so staying up buys nothing over a restart.
const RESUBSCRIBE_DEADLINE: Duration = COMMANDS_ACK_WAIT;
const RESUBSCRIBE_MIN_DELAY: Duration = Duration::from_millis(250);
const RESUBSCRIBE_MAX_DELAY: Duration = Duration::from_secs(10);

/// Requests this host is running from COMMANDS, keyed by subject and
/// `Jsonrpc-Id`, so a redelivery of one is not run a second time.
///
/// Per host on purpose: a command nacked at shutdown or abandoned by a crashed
/// host must run on the next host, whose set starts empty. Notifications carry
/// no id and are never tracked.
#[derive(Default)]
struct InFlightCommands(InFlightSet);

type InFlightKey = (String, ReqId);
type InFlightSet = Rc<RefCell<HashSet<InFlightKey>>>;

/// The same request is already running on this host.
struct AlreadyRunning;

impl InFlightCommands {
    /// `Ok(None)` for a notification, which is never tracked.
    fn claim(&self, msg: &Message) -> Result<Option<InFlightClaim>, AlreadyRunning> {
        let Some(req_id) = msg.headers.as_ref().and_then(req_id_from_request_headers) else {
            return Ok(None);
        };
        let key = (msg.subject.to_string(), req_id);
        if !self.0.borrow_mut().insert(key.clone()) {
            return Err(AlreadyRunning);
        }
        Ok(Some(InFlightClaim {
            set: self.0.clone(),
            key,
        }))
    }
}

/// Releases its request from [`InFlightCommands`] when dropped.
struct InFlightClaim {
    set: InFlightSet,
    key: InFlightKey,
}

impl Drop for InFlightClaim {
    fn drop(&mut self) {
        self.set.borrow_mut().remove(&self.key);
    }
}

async fn dispatch_js_message<N: PublishClient + FlushClient, A: AgentHandler + Sync, M: JsDispatchMessage>(
    js_msg: M,
    agent: &A,
    nats: &N,
    prefix: &AcpPrefix,
) {
    let subject = js_msg.message().subject.to_string();

    let (session_id, method) = match parse_agent_subject(&subject) {
        Some(ParsedAgentSubject::Session { session_id, method }) => (session_id, method),
        Some(ParsedAgentSubject::Global(_)) => {
            warn!(subject, "Global method on JetStream path; handled by core NATS");
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

    let req_id = js_msg.message().headers.as_ref().and_then(req_id_from_request_headers);

    let reply_subject: Option<String> = match (&req_id, &method) {
        (_, SessionAgentMethod::Cancel) => None,
        (Some(_), _) => Some(ResponseSubject::new(prefix, &session_id).to_string()),
        (None, _) => {
            warn!(subject, "JetStream message missing Jsonrpc-Id header");
            None
        }
    };

    let inner = js_msg.message();
    let msg = Message {
        subject: subject.as_str().into(),
        reply: reply_subject.map(|s| s.into()),
        payload: inner.payload.clone(),
        headers: inner.headers.clone(),
        status: None,
        description: None,
        length: inner.payload.len(),
    };
    let subject = msg.subject.as_str();

    let result = match method {
        SessionAgentMethod::Load => {
            handle_jsonrpc_request(
                &msg,
                method.protocol_method(),
                nats,
                in_session(&session_id, |req: LoadSessionRequest| agent.load_session(req)),
            )
            .await
        }
        SessionAgentMethod::Prompt => {
            handle_jsonrpc_request_with_keepalive(
                &msg,
                method.protocol_method(),
                nats,
                &js_msg,
                in_session(&session_id, |req: PromptRequest| agent.prompt(req)),
            )
            .await
        }
        SessionAgentMethod::Cancel => {
            handle_wire_notification(
                &msg,
                method.protocol_method(),
                in_session(&session_id, |req: CancelNotification| agent.cancel(req)),
            )
            .await
        }
        SessionAgentMethod::SetMode => {
            handle_jsonrpc_request(
                &msg,
                method.protocol_method(),
                nats,
                in_session(&session_id, |req: SetSessionModeRequest| agent.set_session_mode(req)),
            )
            .await
        }
        SessionAgentMethod::SetConfigOption => {
            handle_jsonrpc_request(
                &msg,
                method.protocol_method(),
                nats,
                in_session(&session_id, |req: SetSessionConfigOptionRequest| {
                    agent.set_session_config_option(req)
                }),
            )
            .await
        }
        SessionAgentMethod::Fork => {
            handle_jsonrpc_request(
                &msg,
                method.protocol_method(),
                nats,
                in_session(&session_id, |req: ForkSessionRequest| agent.fork_session(req)),
            )
            .await
        }
        SessionAgentMethod::Resume => {
            handle_jsonrpc_request(
                &msg,
                method.protocol_method(),
                nats,
                in_session(&session_id, |req: ResumeSessionRequest| agent.resume_session(req)),
            )
            .await
        }
        SessionAgentMethod::Close => {
            handle_jsonrpc_request(
                &msg,
                method.protocol_method(),
                nats,
                in_session(&session_id, |req: CloseSessionRequest| agent.close_session(req)),
            )
            .await
        }
        SessionAgentMethod::Archive => {
            handle_jsonrpc_request(
                &msg,
                method.protocol_method(),
                nats,
                in_session(&session_id, |req: ArchiveSessionRequest| agent.archive_session(req)),
            )
            .await
        }
        SessionAgentMethod::Unarchive => {
            handle_jsonrpc_request(
                &msg,
                method.protocol_method(),
                nats,
                in_session(&session_id, |req: UnarchiveSessionRequest| agent.unarchive_session(req)),
            )
            .await
        }
        SessionAgentMethod::Status => {
            handle_jsonrpc_request(
                &msg,
                method.protocol_method(),
                nats,
                in_session(&session_id, |req: SessionStatusRequest| agent.session_status(req)),
            )
            .await
        }
        SessionAgentMethod::SetTitle => {
            handle_jsonrpc_request(
                &msg,
                method.protocol_method(),
                nats,
                in_session(&session_id, |req: SetSessionTitleRequest| agent.set_session_title(req)),
            )
            .await
        }
        SessionAgentMethod::Delete => {
            handle_jsonrpc_request(
                &msg,
                method.protocol_method(),
                nats,
                in_session(&session_id, |req: DeleteSessionRequest| agent.delete_session(req)),
            )
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
        Err(DispatchError::Reply(_) | DispatchError::Wire(_)) => {
            if let Err(e) = js_msg.ack().await {
                warn!(subject, error = %e, "Failed to ack after reply failure");
            }
        }
        Err(DispatchError::NotificationHandler(_)) => {
            let _ = js_msg
                .ack()
                .await
                .inspect_err(|e| warn!(subject, error = %e, "Failed to ack after notification handler error"));
        }
    }

    let _ = result.inspect_err(|e| {
        warn!(
            subject,
            session_id = session_id.as_str(),
            error = %e,
            "Error handling JetStream request"
        );
    });
}

#[cfg(test)]
mod tests;
