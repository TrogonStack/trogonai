use super::boundary_exit::BoundaryExit;
use super::eof_signal_reader::EofSignalReader;
use crate::agent_handler::AgentHandler;
use agent_client_protocol::schema::v1::{
    AuthenticateRequest, CancelNotification, CancelRequestNotification, ClientNotification, ClientRequest,
    CloseSessionRequest, DeleteSessionRequest, ForkSessionRequest, InitializeRequest, ListSessionsRequest,
    LoadSessionRequest, LogoutRequest, NewSessionRequest, PromptRequest, ResumeSessionRequest,
    SetSessionConfigOptionRequest, SetSessionModeRequest,
};
use agent_client_protocol::{Agent, ByteStreams, Client, ConnectionTo, Error, JsonRpcResponse, Responder, Result};
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
    work: impl std::future::Future<Output = Result<T>> + Send + 'static,
) -> Result<()> {
    cx.spawn(async move {
        let cancellation = responder.cancellation();
        responder.respond_with_result(cancellation.run_until_cancelled(work).await)
    })
}

/// Connects an [`AgentHandler`] implementation to a byte-stream boundary.
///
/// Registers one `on_receive_request`/`on_receive_notification` callback per
/// [`AgentHandler`] method, then falls through to the SDK's `ClientRequest`/
/// `ClientNotification` catch-all for extension methods, matching ADR 0017's
/// "SDK builder callbacks adapt boundaries to bridge traits" decision. Shared
/// by `acp-nats-server` (WebSocket and HTTP duplex) and `acp-nats-stdio`.
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
    let handler_agent = agent.clone();
    let builder = Agent
        .builder()
        .on_receive_request(
            {
                let agent = handler_agent.clone();
                async move |req: InitializeRequest, responder, cx| {
                    let agent = agent.clone();
                    respond_in_task(&cx, responder, async move { agent.initialize(req).await })
                }
            },
            agent_client_protocol::on_receive_request!(),
        )
        .on_receive_request(
            {
                let agent = handler_agent.clone();
                async move |req: AuthenticateRequest, responder, cx| {
                    let agent = agent.clone();
                    respond_in_task(&cx, responder, async move { agent.authenticate(req).await })
                }
            },
            agent_client_protocol::on_receive_request!(),
        )
        .on_receive_request(
            {
                let agent = handler_agent.clone();
                async move |req: LogoutRequest, responder, cx| {
                    let agent = agent.clone();
                    respond_in_task(&cx, responder, async move { agent.logout(req).await })
                }
            },
            agent_client_protocol::on_receive_request!(),
        )
        .on_receive_request(
            {
                let agent = handler_agent.clone();
                async move |req: NewSessionRequest, responder, cx| {
                    let agent = agent.clone();
                    respond_in_task(&cx, responder, async move { agent.new_session(req).await })
                }
            },
            agent_client_protocol::on_receive_request!(),
        )
        .on_receive_request(
            {
                let agent = handler_agent.clone();
                async move |req: LoadSessionRequest, responder, cx| {
                    let agent = agent.clone();
                    respond_in_task(&cx, responder, async move { agent.load_session(req).await })
                }
            },
            agent_client_protocol::on_receive_request!(),
        )
        .on_receive_request(
            {
                let agent = handler_agent.clone();
                async move |req: PromptRequest, responder, cx| {
                    let agent = agent.clone();
                    respond_in_task(&cx, responder, async move { agent.prompt(req).await })
                }
            },
            agent_client_protocol::on_receive_request!(),
        )
        .on_receive_request(
            {
                let agent = handler_agent.clone();
                async move |req: SetSessionModeRequest, responder, cx| {
                    let agent = agent.clone();
                    respond_in_task(&cx, responder, async move { agent.set_session_mode(req).await })
                }
            },
            agent_client_protocol::on_receive_request!(),
        )
        .on_receive_request(
            {
                let agent = handler_agent.clone();
                async move |req: SetSessionConfigOptionRequest, responder, cx| {
                    let agent = agent.clone();
                    respond_in_task(
                        &cx,
                        responder,
                        async move { agent.set_session_config_option(req).await },
                    )
                }
            },
            agent_client_protocol::on_receive_request!(),
        )
        .on_receive_request(
            {
                let agent = handler_agent.clone();
                async move |req: ForkSessionRequest, responder, cx| {
                    let agent = agent.clone();
                    respond_in_task(&cx, responder, async move { agent.fork_session(req).await })
                }
            },
            agent_client_protocol::on_receive_request!(),
        )
        .on_receive_request(
            {
                let agent = handler_agent.clone();
                async move |req: ResumeSessionRequest, responder, cx| {
                    let agent = agent.clone();
                    respond_in_task(&cx, responder, async move { agent.resume_session(req).await })
                }
            },
            agent_client_protocol::on_receive_request!(),
        )
        .on_receive_request(
            {
                let agent = handler_agent.clone();
                async move |req: CloseSessionRequest, responder, cx| {
                    let agent = agent.clone();
                    respond_in_task(&cx, responder, async move { agent.close_session(req).await })
                }
            },
            agent_client_protocol::on_receive_request!(),
        )
        .on_receive_request(
            {
                let agent = handler_agent.clone();
                async move |req: DeleteSessionRequest, responder, cx| {
                    let agent = agent.clone();
                    respond_in_task(&cx, responder, async move { agent.delete_session(req).await })
                }
            },
            agent_client_protocol::on_receive_request!(),
        )
        .on_receive_request(
            {
                let agent = handler_agent.clone();
                async move |req: ListSessionsRequest, responder, cx| {
                    let agent = agent.clone();
                    respond_in_task(&cx, responder, async move { agent.list_sessions(req).await })
                }
            },
            agent_client_protocol::on_receive_request!(),
        )
        .on_receive_notification(
            async move |_notif: CancelRequestNotification, _cx| Ok(()),
            agent_client_protocol::on_receive_notification!(),
        )
        .on_receive_notification(
            {
                let agent = handler_agent.clone();
                async move |notif: CancelNotification, cx| {
                    let agent = agent.clone();
                    cx.spawn(async move { agent.cancel(notif).await })
                }
            },
            agent_client_protocol::on_receive_notification!(),
        )
        .on_receive_request(
            {
                let agent = handler_agent.clone();
                async move |req: ClientRequest, responder, cx| {
                    let ClientRequest::ExtMethodRequest(ext_request) = req else {
                        return responder.respond_with_error(Error::method_not_found());
                    };
                    let agent = agent.clone();
                    respond_in_task(&cx, responder, async move {
                        let response = agent.ext_method(ext_request).await?;
                        serde_json::to_value(response).map_err(Error::into_internal_error)
                    })
                }
            },
            agent_client_protocol::on_receive_request!(),
        )
        .on_receive_notification(
            {
                let agent = handler_agent.clone();
                async move |notif: ClientNotification, cx| {
                    let ClientNotification::ExtNotification(ext_notification) = notif else {
                        return Ok(());
                    };
                    let agent = agent.clone();
                    cx.spawn(async move { agent.ext_notification(ext_notification).await })
                }
            },
            agent_client_protocol::on_receive_notification!(),
        );

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
