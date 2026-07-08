use super::boundary_exit::BoundaryExit;
use super::eof_signal_reader::EofSignalReader;
use crate::agent_handler::AgentHandler;
use agent_client_protocol::schema::v1::{
    AuthenticateRequest, AuthenticateResponse, CancelNotification, CancelRequestNotification, ClientNotification,
    ClientRequest, CloseSessionRequest, CloseSessionResponse, DeleteSessionRequest, DeleteSessionResponse,
    ForkSessionRequest, ForkSessionResponse, InitializeRequest, InitializeResponse, ListSessionsRequest,
    ListSessionsResponse, LoadSessionRequest, LoadSessionResponse, LogoutRequest, LogoutResponse, NewSessionRequest,
    NewSessionResponse, PromptRequest, PromptResponse, ResumeSessionRequest, ResumeSessionResponse,
    SetSessionConfigOptionRequest, SetSessionConfigOptionResponse, SetSessionModeRequest, SetSessionModeResponse,
};
use agent_client_protocol::{Agent, ByteStreams, Client, ConnectionTo, Error, JsonRpcResponse, Responder, Result};
use std::future::Future;
use std::sync::Arc;

/// Spawns `work` onto the connection's task actor and responds when it
/// settles, honoring `$/cancel_request`.
///
/// Handler callbacks run inline on the SDK dispatch loop, so awaiting bridge
/// work there would block every subsequent message on the connection,
/// including the cancellation notification itself. If the peer cancels first,
/// `work` is dropped and the response is the SDK's standard
/// `Error::request_cancelled`.
fn respond_in_task<T: JsonRpcResponse + Send + 'static>(
    cx: &ConnectionTo<Client>,
    responder: Responder<T>,
    work: impl Future<Output = Result<T>> + Send + 'static,
) -> Result<()> {
    cx.spawn(async move {
        let cancellation = responder.cancellation();
        responder.respond_with_result(cancellation.run_until_cancelled(work).await)
    })
}

async fn initialize<A: AgentHandler + Send + Sync + 'static>(
    agent: Arc<A>,
    req: InitializeRequest,
    responder: Responder<InitializeResponse>,
    cx: ConnectionTo<Client>,
) -> Result<()> {
    respond_in_task(&cx, responder, async move { agent.initialize(req).await })
}

async fn authenticate<A: AgentHandler + Send + Sync + 'static>(
    agent: Arc<A>,
    req: AuthenticateRequest,
    responder: Responder<AuthenticateResponse>,
    cx: ConnectionTo<Client>,
) -> Result<()> {
    respond_in_task(&cx, responder, async move { agent.authenticate(req).await })
}

async fn logout<A: AgentHandler + Send + Sync + 'static>(
    agent: Arc<A>,
    req: LogoutRequest,
    responder: Responder<LogoutResponse>,
    cx: ConnectionTo<Client>,
) -> Result<()> {
    respond_in_task(&cx, responder, async move { agent.logout(req).await })
}

async fn new_session<A: AgentHandler + Send + Sync + 'static>(
    agent: Arc<A>,
    req: NewSessionRequest,
    responder: Responder<NewSessionResponse>,
    cx: ConnectionTo<Client>,
) -> Result<()> {
    respond_in_task(&cx, responder, async move { agent.new_session(req).await })
}

async fn load_session<A: AgentHandler + Send + Sync + 'static>(
    agent: Arc<A>,
    req: LoadSessionRequest,
    responder: Responder<LoadSessionResponse>,
    cx: ConnectionTo<Client>,
) -> Result<()> {
    respond_in_task(&cx, responder, async move { agent.load_session(req).await })
}

async fn prompt<A: AgentHandler + Send + Sync + 'static>(
    agent: Arc<A>,
    req: PromptRequest,
    responder: Responder<PromptResponse>,
    cx: ConnectionTo<Client>,
) -> Result<()> {
    respond_in_task(&cx, responder, async move { agent.prompt(req).await })
}

async fn set_session_mode<A: AgentHandler + Send + Sync + 'static>(
    agent: Arc<A>,
    req: SetSessionModeRequest,
    responder: Responder<SetSessionModeResponse>,
    cx: ConnectionTo<Client>,
) -> Result<()> {
    respond_in_task(&cx, responder, async move { agent.set_session_mode(req).await })
}

async fn set_session_config_option<A: AgentHandler + Send + Sync + 'static>(
    agent: Arc<A>,
    req: SetSessionConfigOptionRequest,
    responder: Responder<SetSessionConfigOptionResponse>,
    cx: ConnectionTo<Client>,
) -> Result<()> {
    respond_in_task(
        &cx,
        responder,
        async move { agent.set_session_config_option(req).await },
    )
}

async fn fork_session<A: AgentHandler + Send + Sync + 'static>(
    agent: Arc<A>,
    req: ForkSessionRequest,
    responder: Responder<ForkSessionResponse>,
    cx: ConnectionTo<Client>,
) -> Result<()> {
    respond_in_task(&cx, responder, async move { agent.fork_session(req).await })
}

async fn resume_session<A: AgentHandler + Send + Sync + 'static>(
    agent: Arc<A>,
    req: ResumeSessionRequest,
    responder: Responder<ResumeSessionResponse>,
    cx: ConnectionTo<Client>,
) -> Result<()> {
    respond_in_task(&cx, responder, async move { agent.resume_session(req).await })
}

async fn close_session<A: AgentHandler + Send + Sync + 'static>(
    agent: Arc<A>,
    req: CloseSessionRequest,
    responder: Responder<CloseSessionResponse>,
    cx: ConnectionTo<Client>,
) -> Result<()> {
    respond_in_task(&cx, responder, async move { agent.close_session(req).await })
}

async fn delete_session<A: AgentHandler + Send + Sync + 'static>(
    agent: Arc<A>,
    req: DeleteSessionRequest,
    responder: Responder<DeleteSessionResponse>,
    cx: ConnectionTo<Client>,
) -> Result<()> {
    respond_in_task(&cx, responder, async move { agent.delete_session(req).await })
}

async fn list_sessions<A: AgentHandler + Send + Sync + 'static>(
    agent: Arc<A>,
    req: ListSessionsRequest,
    responder: Responder<ListSessionsResponse>,
    cx: ConnectionTo<Client>,
) -> Result<()> {
    respond_in_task(&cx, responder, async move { agent.list_sessions(req).await })
}

async fn ext_method<A: AgentHandler + Send + Sync + 'static>(
    agent: Arc<A>,
    req: ClientRequest,
    responder: Responder<serde_json::Value>,
    cx: ConnectionTo<Client>,
) -> Result<()> {
    let ClientRequest::ExtMethodRequest(ext_request) = req else {
        return responder.respond_with_error(Error::method_not_found());
    };
    respond_in_task(&cx, responder, async move {
        let response = agent.ext_method(ext_request).await?;
        serde_json::to_value(response).map_err(Error::into_internal_error)
    })
}

fn log_notification_error(method: &'static str, result: Result<()>) {
    if let Err(error) = result {
        tracing::warn!(error = %error, method, "notification handler failed");
    }
}

async fn cancel<A: AgentHandler + Send + Sync + 'static>(
    agent: Arc<A>,
    notif: CancelNotification,
    cx: ConnectionTo<Client>,
) -> Result<()> {
    cx.spawn(async move {
        log_notification_error("session/cancel", agent.cancel(notif).await);
        Ok(())
    })
}

async fn ext_notification<A: AgentHandler + Send + Sync + 'static>(
    agent: Arc<A>,
    notif: ClientNotification,
    cx: ConnectionTo<Client>,
) -> Result<()> {
    let ClientNotification::ExtNotification(notification) = notif else {
        return Ok(());
    };
    cx.spawn(async move {
        log_notification_error("ext notification", agent.ext_notification(notification).await);
        Ok(())
    })
}

macro_rules! route_request {
    ($builder:expr, $agent:expr, $handler:path) => {
        $builder.on_receive_request(
            {
                let agent = $agent.clone();
                async move |req, responder, cx| $handler(agent.clone(), req, responder, cx).await
            },
            agent_client_protocol::on_receive_request!(),
        )
    };
}

macro_rules! route_notification {
    ($builder:expr, $agent:expr, $handler:path) => {
        $builder.on_receive_notification(
            {
                let agent = $agent.clone();
                async move |notif, cx| $handler(agent.clone(), notif, cx).await
            },
            agent_client_protocol::on_receive_notification!(),
        )
    };
}

/// Connects an [`AgentHandler`] implementation to a byte-stream boundary.
///
/// Registers one named handler per [`AgentHandler`] method, then falls
/// through to the SDK's `ClientRequest`/`ClientNotification` catch-all for
/// extension methods, matching ADR 0020's "SDK builder callbacks adapt
/// boundaries to bridge traits" decision. Shared by `acp-nats-server`
/// (WebSocket and HTTP duplex) and `acp-nats-stdio`.
///
/// The SDK treats incoming EOF as a graceful stream end and keeps the
/// connection alive, so the incoming stream is wrapped to detect closure:
/// when the peer closes the transport before `main_fn` completes, the
/// connection shuts down and [`BoundaryExit::TransportClosed`] is returned.
pub async fn connect_agent_boundary<A, W, R, T>(
    agent: Arc<A>,
    outgoing: W,
    incoming: R,
    main_fn: impl AsyncFnOnce(ConnectionTo<Client>) -> Result<T>,
) -> Result<BoundaryExit<T>>
where
    A: AgentHandler + Send + Sync + 'static,
    W: futures::AsyncWrite + Send + 'static,
    R: futures::AsyncRead + Send + Unpin + 'static,
{
    let (incoming, eof_rx) = EofSignalReader::new(incoming);

    let builder = Agent.builder();
    let builder = route_request!(builder, agent, initialize);
    let builder = route_request!(builder, agent, authenticate);
    let builder = route_request!(builder, agent, logout);
    let builder = route_request!(builder, agent, new_session);
    let builder = route_request!(builder, agent, load_session);
    let builder = route_request!(builder, agent, prompt);
    let builder = route_request!(builder, agent, set_session_mode);
    let builder = route_request!(builder, agent, set_session_config_option);
    let builder = route_request!(builder, agent, fork_session);
    let builder = route_request!(builder, agent, resume_session);
    let builder = route_request!(builder, agent, close_session);
    let builder = route_request!(builder, agent, delete_session);
    let builder = route_request!(builder, agent, list_sessions);
    let builder = builder.on_receive_notification(
        async move |_notif: CancelRequestNotification, _cx| Ok(()),
        agent_client_protocol::on_receive_notification!(),
    );
    let builder = route_notification!(builder, agent, cancel);
    let builder = route_request!(builder, agent, ext_method);
    let builder = route_notification!(builder, agent, ext_notification);

    builder
        .connect_with(ByteStreams::new(outgoing, incoming), async move |cx| {
            tokio::select! {
                result = main_fn(cx) => result.map(BoundaryExit::Main),
                _ = eof_rx => Ok(BoundaryExit::TransportClosed),
            }
        })
        .await
}

#[cfg(test)]
mod tests;
