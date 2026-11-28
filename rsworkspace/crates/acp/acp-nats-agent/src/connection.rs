use crate::client_gate::{AdmitAll, ClientGate};
use crate::constants::SESSION_READY_SUFFIX;
use acp_nats::jetstream::consumers::commands_observer;
use acp_nats::jetstream::streams::commands_stream_name;
use acp_nats::nats::subscriptions::{AllAgentExtSubject, AllAgentSubject, GlobalAllSubject};
use acp_nats::nats::{GlobalAgentMethod, ParsedAgentSubject, SessionAgentMethod, parse_agent_subject};
use acp_nats::wire::{
    encode_agent_error, encode_success, req_id_from_request_headers, response_id_from_request_headers,
};
use acp_nats::{AcpPrefix, AcpSessionId, AgentHandler, NatsClientProxy, ResponseSubject, requesting_client};
use agent_client_protocol::v1::schema::{
    ArchiveSessionRequest, AuthStatusRequest, AuthenticateRequest, CancelNotification, CloseSessionRequest,
    DeleteSessionRequest, DisableProviderRequest, ExtNotification, ExtRequest, ForkSessionRequest,
    ListProvidersRequest, ListSessionsRequest, LoadSessionRequest, LogoutRequest, NewSessionRequest, PromptRequest,
    ResumeSessionRequest, SessionStatusRequest, SetProviderRequest, SetSessionConfigOptionRequest,
    SetSessionModeRequest, SetSessionTitleRequest, UnarchiveSessionRequest,
};
use async_nats::Message;
use async_nats::jetstream::AckKind;
#[cfg(test)]
use bytes::Bytes;
use futures::future::LocalBoxFuture;
use futures::{FutureExt, StreamExt};
use serde::de::Error;
use std::rc::Rc;
use std::time::Duration;
use tracing::{info, warn};
use trogon_nats::{FlushClient, PublishClient, RequestClient, SubscribeClient};

#[derive(Debug, thiserror::Error)]
pub enum ConnectionError {
    #[error("failed to subscribe: {0}")]
    Subscribe(#[source] Box<dyn std::error::Error + Send + Sync>),
    #[error("jetstream error: {0}")]
    JetStream(#[source] Box<dyn std::error::Error + Send + Sync>),
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

        let io_task = async move { serve(agent, AdmitAll, nats_for_serve, &prefix, spawn).await };

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
        J: JetStreamGetStream + 'static,
        trogon_nats::jetstream::JsMessageOf<J>: JsDispatchMessage,
    {
        Self::with_jetstream_gated(agent, AdmitAll, nats, js, acp_prefix, spawn)
    }

    /// [`Self::with_jetstream`], with every request first put to `gate`: one
    /// it refuses never reaches the agent. For an agent that serves one
    /// client, such as `acp-host`.
    pub fn with_jetstream_gated<J>(
        agent: impl AgentHandler + Sync + 'static,
        gate: impl ClientGate + 'static,
        nats: N,
        js: J,
        acp_prefix: AcpPrefix,
        spawn: impl Fn(LocalBoxFuture<'static, ()>) + Copy + 'static,
    ) -> (Self, impl std::future::Future<Output = Result<(), ConnectionError>>)
    where
        J: JetStreamGetStream + 'static,
        trogon_nats::jetstream::JsMessageOf<J>: JsDispatchMessage,
    {
        Self::with_jetstream_gated_until(agent, gate, nats, js, acp_prefix, spawn, std::future::pending())
    }

    /// [`Self::with_jetstream_gated`], serving until `stop` resolves and then
    /// handing back every COMMANDS message it holds, so none waits out
    /// `ack_wait` on a host that is going away.
    ///
    /// On `stop` it stops pulling first (the pull subscription goes, so no
    /// command is delivered here again), then nacks every message already
    /// delivered: the ones not yet dispatched and the ones the agent is still
    /// working on, whose work is dropped. The future resolves once all of them
    /// are nacked. Nothing is acked or answered after that, so the caller can
    /// flush and close the NATS connection. A command the server sends in the
    /// instant between the last read and the unsubscribe is still only
    /// redelivered after `ack_wait`.
    ///
    pub fn with_jetstream_gated_until<J>(
        agent: impl AgentHandler + Sync + 'static,
        gate: impl ClientGate + 'static,
        nats: N,
        js: J,
        acp_prefix: AcpPrefix,
        spawn: impl Fn(LocalBoxFuture<'static, ()>) + Copy + 'static,
        stop: impl std::future::Future<Output = ()> + 'static,
    ) -> (Self, impl std::future::Future<Output = Result<(), ConnectionError>>)
    where
        J: JetStreamGetStream + 'static,
        trogon_nats::jetstream::JsMessageOf<J>: JsDispatchMessage,
    {
        let nats_for_serve = nats.clone();
        let nats_for_js = nats.clone();
        let prefix = acp_prefix.clone();
        let prefix_js = prefix.clone();

        let io_task = async move {
            let agent = Rc::new(agent);
            let gate = Rc::new(gate);

            // The JetStream side ends the run once `stop` has fired and its
            // commands are settled; the core side ends with it.
            let core = serve_global(agent.clone(), gate.clone(), nats_for_serve, &prefix, spawn);
            let jetstream = serve_js(agent, gate, nats_for_js, js, &prefix_js, spawn, stop);

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

async fn serve<N, A, G>(
    agent: A,
    gate: G,
    nats: N,
    prefix: &AcpPrefix,
    spawn: impl Fn(LocalBoxFuture<'static, ()>) + 'static,
) -> Result<(), ConnectionError>
where
    N: SubscribeClient + PublishClient + FlushClient + Clone + 'static,
    A: AgentHandler + Sync + 'static,
    G: ClientGate + 'static,
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
    let gate = Rc::new(gate);
    let nats = Rc::new(nats);

    while let Some(msg) = subscriber.next().await {
        let agent = agent.clone();
        let gate = gate.clone();
        let nats = nats.clone();
        spawn(Box::pin(async move {
            dispatch_message(msg, agent.as_ref(), gate.as_ref(), nats.as_ref()).await;
        }));
    }

    info!("Agent-side NATS connection ended");
    Ok(())
}

async fn serve_global<N, A, G>(
    agent: Rc<A>,
    gate: Rc<G>,
    nats: N,
    prefix: &AcpPrefix,
    spawn: impl Fn(LocalBoxFuture<'static, ()>) + 'static,
) -> Result<(), ConnectionError>
where
    N: SubscribeClient + PublishClient + FlushClient + Clone + 'static,
    A: AgentHandler + Sync + 'static,
    G: ClientGate + 'static,
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
        let gate = gate.clone();
        let nats = nats.clone();
        spawn(Box::pin(async move {
            dispatch_message(msg, agent.as_ref(), gate.as_ref(), nats.as_ref()).await;
        }));
    }

    info!("Global-only NATS connection ended");
    Ok(())
}

/// Whether `subject` is the bridge's `session.ready` signal. The bridge
/// publishes it on `{prefix}.v1.session.{id}.agent.ext.ready` once a session is
/// set up, for its own clients to wait on. It falls inside the
/// `session.*.agent.ext.>` space the agent serves, but it is no call to the
/// agent: forwarding it would hand the agent a `_ready` notification on every
/// new session and prompt.
fn is_session_ready_signal(subject: &str) -> bool {
    subject
        .strip_suffix(SESSION_READY_SUFFIX)
        .and_then(|rest| rest.rsplit_once('.'))
        .is_some_and(|(head, _session_id)| head.ends_with(".session"))
}

async fn dispatch_message<N: PublishClient + FlushClient, A: AgentHandler + Sync, G: ClientGate>(
    msg: Message,
    agent: &A,
    gate: &G,
    nats: &N,
) {
    let subject = msg.subject.as_str();

    if is_session_ready_signal(subject) {
        return;
    }

    let parsed = match parse_agent_subject(subject) {
        Some(p) => p,
        None => return,
    };

    if let Err(refusal) = gate.admit(requesting_client(msg.headers.as_ref())) {
        refuse(&msg, &refusal, nats).await;
        return;
    }

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
                // As sent, so members the typed request does not name reach
                // an agent that relays the handshake.
                |params: serde_json::Value| agent.initialize_raw(params),
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
        GlobalAgentMethod::Ext(_) => {
            if msg.reply.is_some() {
                handle_jsonrpc_request(msg, method.protocol_method().as_str(), nats, |req: ExtRequest| {
                    agent.ext_method(req)
                })
                .await
            } else {
                handle_notification(msg, |req: ExtNotification| agent.ext_notification(req)).await
            }
        }
    }
}

async fn dispatch_session<N: PublishClient + FlushClient, A: AgentHandler + Sync>(
    method: SessionAgentMethod,
    msg: &Message,
    agent: &A,
    nats: &N,
) -> Result<(), DispatchError> {
    match method {
        SessionAgentMethod::Load => {
            handle_jsonrpc_request(msg, method.protocol_method(), nats, |req: LoadSessionRequest| {
                agent.load_session(req)
            })
            .await
        }
        SessionAgentMethod::Prompt => {
            handle_jsonrpc_request(msg, method.protocol_method(), nats, |req: PromptRequest| {
                agent.prompt(req)
            })
            .await
        }
        SessionAgentMethod::Cancel => {
            handle_wire_notification(msg, method.protocol_method(), |req: CancelNotification| {
                agent.cancel(req)
            })
            .await
        }
        SessionAgentMethod::SetMode => {
            handle_jsonrpc_request(msg, method.protocol_method(), nats, |req: SetSessionModeRequest| {
                agent.set_session_mode(req)
            })
            .await
        }
        SessionAgentMethod::SetConfigOption => {
            handle_jsonrpc_request(
                msg,
                method.protocol_method(),
                nats,
                |req: SetSessionConfigOptionRequest| agent.set_session_config_option(req),
            )
            .await
        }
        SessionAgentMethod::Fork => {
            handle_jsonrpc_request(msg, method.protocol_method(), nats, |req: ForkSessionRequest| {
                agent.fork_session(req)
            })
            .await
        }
        SessionAgentMethod::Resume => {
            handle_jsonrpc_request(msg, method.protocol_method(), nats, |req: ResumeSessionRequest| {
                agent.resume_session(req)
            })
            .await
        }
        SessionAgentMethod::Close => {
            handle_jsonrpc_request(msg, method.protocol_method(), nats, |req: CloseSessionRequest| {
                agent.close_session(req)
            })
            .await
        }
        SessionAgentMethod::Archive => {
            handle_jsonrpc_request(msg, method.protocol_method(), nats, |req: ArchiveSessionRequest| {
                agent.archive_session(req)
            })
            .await
        }
        SessionAgentMethod::Unarchive => {
            handle_jsonrpc_request(msg, method.protocol_method(), nats, |req: UnarchiveSessionRequest| {
                agent.unarchive_session(req)
            })
            .await
        }
        SessionAgentMethod::Status => {
            handle_jsonrpc_request(msg, method.protocol_method(), nats, |req: SessionStatusRequest| {
                agent.session_status(req)
            })
            .await
        }
        SessionAgentMethod::SetTitle => {
            handle_jsonrpc_request(msg, method.protocol_method(), nats, |req: SetSessionTitleRequest| {
                agent.set_session_title(req)
            })
            .await
        }
        SessionAgentMethod::Delete => {
            handle_jsonrpc_request(msg, method.protocol_method(), nats, |req: DeleteSessionRequest| {
                agent.delete_session(req)
            })
            .await
        }
    }
}

/// Answers a request the [`ClientGate`] refused with the refusal, so its
/// sender sees why; a refused notification has no one to answer.
async fn refuse<N: PublishClient + FlushClient>(msg: &Message, refusal: &agent_client_protocol::Error, nats: &N) {
    let subject = msg.subject.as_str();
    let client = requesting_client(msg.headers.as_ref());
    warn!(subject, client, error = %refusal, "Refused a request from a client this agent does not serve");
    let Some(reply_to) = msg.reply.as_deref() else {
        return;
    };
    let id = response_id_from_request_headers(&msg.headers.clone().unwrap_or_default());
    if let Err(error) = reply_wire(nats, reply_to, encode_agent_error(id, refusal)).await {
        warn!(subject, error = %error, "Failed to send a refusal");
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

use trogon_nats::jetstream::{
    JetStreamConsumer as _, JetStreamCreateConsumer as _, JetStreamGetStream, JsAckWith, JsDispatchMessage,
};

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

async fn serve_js<N, J, A, G>(
    agent: Rc<A>,
    gate: Rc<G>,
    nats: N,
    js: J,
    prefix: &AcpPrefix,
    spawn: impl Fn(LocalBoxFuture<'static, ()>) + 'static,
    stop: impl std::future::Future<Output = ()>,
) -> Result<(), ConnectionError>
where
    N: PublishClient + FlushClient + Clone + 'static,
    J: JetStreamGetStream + 'static,
    trogon_nats::jetstream::JsMessageOf<J>: JsDispatchMessage,
    A: AgentHandler + Sync + 'static,
    G: ClientGate + 'static,
{
    let stream_name = commands_stream_name(prefix);
    let config = commands_observer();

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

    let prefix = Rc::new(prefix.clone());

    // Every dispatch task holds a `settled` sender until its message is acked
    // or nacked, and gives its message back once `nack_in_flight` is set.
    let (settled, mut all_settled) = tokio::sync::mpsc::channel::<()>(1);
    let (nack_in_flight, nack_signal) = tokio::sync::watch::channel(false);

    tokio::pin!(stop);
    loop {
        let msg_result = tokio::select! {
            biased;
            () = &mut stop => break,
            next = messages.next() => match next {
                Some(msg_result) => msg_result,
                None => {
                    info!("JetStream COMMANDS consumer ended");
                    return Ok(());
                }
            },
        };
        match msg_result {
            Ok(js_msg) => {
                let agent = agent.clone();
                let gate = gate.clone();
                let nats = nats.clone();
                let prefix = prefix.clone();
                let settled = settled.clone();
                let mut nack_signal = nack_signal.clone();
                spawn(Box::pin(async move {
                    tokio::select! {
                        biased;
                        () = dispatch_js_message(&js_msg, agent.as_ref(), gate.as_ref(), nats.as_ref(), &prefix) => {}
                        Ok(_) = nack_signal.wait_for(|nack| *nack) => nack_for_redelivery(&js_msg).await,
                    }
                    drop(settled);
                }));
            }
            Err(e) => {
                warn!(error = %e, "JetStream consumer error");
            }
        }
    }

    info!("Stopping the JetStream COMMANDS consumer; handing back undelivered commands");
    // Stop pulling before nacking anything: a nacked command is redelivered
    // at once, and it must not come back to this subscription.
    let mut undispatched = Vec::new();
    while let Some(Some(msg_result)) = messages.next().now_or_never() {
        undispatched.extend(msg_result.ok());
    }
    drop(messages);
    nack_in_flight.send_replace(true);
    for js_msg in &undispatched {
        nack_for_redelivery(js_msg).await;
    }
    drop(settled);
    let _ = all_settled.recv().await;
    info!(undispatched = undispatched.len(), "JetStream COMMANDS consumer stopped");
    Ok(())
}

/// Gives a delivered command back to the consumer for another host.
async fn nack_for_redelivery<M: JsAckWith>(js_msg: &M) {
    if let Err(e) = js_msg.ack_with(AckKind::Nak(None)).await {
        warn!(error = %e, "Failed to nack a JetStream command on shutdown");
    }
}

async fn dispatch_js_message<
    N: PublishClient + FlushClient,
    A: AgentHandler + Sync,
    G: ClientGate,
    M: JsDispatchMessage,
>(
    js_msg: &M,
    agent: &A,
    gate: &G,
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

    if let Err(refusal) = gate.admit(requesting_client(msg.headers.as_ref())) {
        refuse(&msg, &refusal, nats).await;
        // Consumed: redelivering it would only be refused again.
        if let Err(e) = js_msg.ack().await {
            warn!(subject, error = %e, "Failed to ack refused JetStream message");
        }
        return;
    }

    let result = match method {
        SessionAgentMethod::Load => {
            handle_jsonrpc_request(&msg, method.protocol_method(), nats, |req: LoadSessionRequest| {
                agent.load_session(req)
            })
            .await
        }
        SessionAgentMethod::Prompt => {
            handle_jsonrpc_request_with_keepalive(&msg, method.protocol_method(), nats, js_msg, |req: PromptRequest| {
                agent.prompt(req)
            })
            .await
        }
        SessionAgentMethod::Cancel => {
            handle_wire_notification(&msg, method.protocol_method(), |req: CancelNotification| {
                agent.cancel(req)
            })
            .await
        }
        SessionAgentMethod::SetMode => {
            handle_jsonrpc_request(&msg, method.protocol_method(), nats, |req: SetSessionModeRequest| {
                agent.set_session_mode(req)
            })
            .await
        }
        SessionAgentMethod::SetConfigOption => {
            handle_jsonrpc_request(
                &msg,
                method.protocol_method(),
                nats,
                |req: SetSessionConfigOptionRequest| agent.set_session_config_option(req),
            )
            .await
        }
        SessionAgentMethod::Fork => {
            handle_jsonrpc_request(&msg, method.protocol_method(), nats, |req: ForkSessionRequest| {
                agent.fork_session(req)
            })
            .await
        }
        SessionAgentMethod::Resume => {
            handle_jsonrpc_request(&msg, method.protocol_method(), nats, |req: ResumeSessionRequest| {
                agent.resume_session(req)
            })
            .await
        }
        SessionAgentMethod::Close => {
            handle_jsonrpc_request(&msg, method.protocol_method(), nats, |req: CloseSessionRequest| {
                agent.close_session(req)
            })
            .await
        }
        SessionAgentMethod::Archive => {
            handle_jsonrpc_request(&msg, method.protocol_method(), nats, |req: ArchiveSessionRequest| {
                agent.archive_session(req)
            })
            .await
        }
        SessionAgentMethod::Unarchive => {
            handle_jsonrpc_request(&msg, method.protocol_method(), nats, |req: UnarchiveSessionRequest| {
                agent.unarchive_session(req)
            })
            .await
        }
        SessionAgentMethod::Status => {
            handle_jsonrpc_request(&msg, method.protocol_method(), nats, |req: SessionStatusRequest| {
                agent.session_status(req)
            })
            .await
        }
        SessionAgentMethod::SetTitle => {
            handle_jsonrpc_request(&msg, method.protocol_method(), nats, |req: SetSessionTitleRequest| {
                agent.set_session_title(req)
            })
            .await
        }
        SessionAgentMethod::Delete => {
            handle_jsonrpc_request(&msg, method.protocol_method(), nats, |req: DeleteSessionRequest| {
                agent.delete_session(req)
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
