use super::boundary_exit::BoundaryExit;
use super::eof_signal_reader::EofSignalReader;
use crate::agent_handler::AgentHandler;
use agent_client_protocol::schema::v1::{
    AuthenticateRequest, CancelNotification, ClientNotification, ClientRequest, CloseSessionRequest,
    ForkSessionRequest, InitializeRequest, ListSessionsRequest, LoadSessionRequest, LogoutRequest, NewSessionRequest,
    PromptRequest, ResumeSessionRequest, SetSessionConfigOptionRequest, SetSessionModeRequest,
};
use agent_client_protocol::{Agent, ByteStreams, Client, ConnectionTo, Error, Result};
use std::sync::Arc;

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
                async move |req: InitializeRequest, responder, _cx| responder.respond(agent.initialize(req).await?)
            },
            agent_client_protocol::on_receive_request!(),
        )
        .on_receive_request(
            {
                let agent = handler_agent.clone();
                async move |req: AuthenticateRequest, responder, _cx| responder.respond(agent.authenticate(req).await?)
            },
            agent_client_protocol::on_receive_request!(),
        )
        .on_receive_request(
            {
                let agent = handler_agent.clone();
                async move |req: LogoutRequest, responder, _cx| responder.respond(agent.logout(req).await?)
            },
            agent_client_protocol::on_receive_request!(),
        )
        .on_receive_request(
            {
                let agent = handler_agent.clone();
                async move |req: NewSessionRequest, responder, _cx| responder.respond(agent.new_session(req).await?)
            },
            agent_client_protocol::on_receive_request!(),
        )
        .on_receive_request(
            {
                let agent = handler_agent.clone();
                async move |req: LoadSessionRequest, responder, _cx| responder.respond(agent.load_session(req).await?)
            },
            agent_client_protocol::on_receive_request!(),
        )
        .on_receive_request(
            {
                let agent = handler_agent.clone();
                async move |req: PromptRequest, responder, _cx| responder.respond(agent.prompt(req).await?)
            },
            agent_client_protocol::on_receive_request!(),
        )
        .on_receive_request(
            {
                let agent = handler_agent.clone();
                async move |req: SetSessionModeRequest, responder, _cx| {
                    responder.respond(agent.set_session_mode(req).await?)
                }
            },
            agent_client_protocol::on_receive_request!(),
        )
        .on_receive_request(
            {
                let agent = handler_agent.clone();
                async move |req: SetSessionConfigOptionRequest, responder, _cx| {
                    responder.respond(agent.set_session_config_option(req).await?)
                }
            },
            agent_client_protocol::on_receive_request!(),
        )
        .on_receive_request(
            {
                let agent = handler_agent.clone();
                async move |req: ForkSessionRequest, responder, _cx| responder.respond(agent.fork_session(req).await?)
            },
            agent_client_protocol::on_receive_request!(),
        )
        .on_receive_request(
            {
                let agent = handler_agent.clone();
                async move |req: ResumeSessionRequest, responder, _cx| {
                    responder.respond(agent.resume_session(req).await?)
                }
            },
            agent_client_protocol::on_receive_request!(),
        )
        .on_receive_request(
            {
                let agent = handler_agent.clone();
                async move |req: CloseSessionRequest, responder, _cx| responder.respond(agent.close_session(req).await?)
            },
            agent_client_protocol::on_receive_request!(),
        )
        .on_receive_request(
            {
                let agent = handler_agent.clone();
                async move |req: ListSessionsRequest, responder, _cx| responder.respond(agent.list_sessions(req).await?)
            },
            agent_client_protocol::on_receive_request!(),
        )
        .on_receive_notification(
            {
                let agent = handler_agent.clone();
                async move |notif: CancelNotification, _cx| {
                    agent.cancel(notif).await?;
                    Ok(())
                }
            },
            agent_client_protocol::on_receive_notification!(),
        )
        .on_receive_request(
            {
                let agent = handler_agent.clone();
                async move |req: ClientRequest, responder, _cx| {
                    let ClientRequest::ExtMethodRequest(ext_request) = req else {
                        return responder.respond_with_error(Error::method_not_found());
                    };
                    match agent.ext_method(ext_request).await {
                        Ok(response) => {
                            let value = serde_json::to_value(response).map_err(Error::into_internal_error)?;
                            responder.respond(value)
                        }
                        Err(error) => responder.respond_with_error(error),
                    }
                }
            },
            agent_client_protocol::on_receive_request!(),
        )
        .on_receive_notification(
            {
                let agent = handler_agent.clone();
                async move |notif: ClientNotification, _cx| {
                    let ClientNotification::ExtNotification(ext_notification) = notif else {
                        return Ok(());
                    };
                    agent.ext_notification(ext_notification).await?;
                    Ok(())
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
