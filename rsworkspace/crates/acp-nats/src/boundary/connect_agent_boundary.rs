use super::boundary_exit::BoundaryExit;
use super::eof_signal_reader::EofSignalReader;
use crate::agent_handler::AgentHandler;
use agent_client_protocol::schema::v1::{CancelRequestNotification, ClientNotification, ClientRequest};
use agent_client_protocol::{Agent, ByteStreams, Client, ConnectionTo, Dispatch, Error, Responder, Result};
use std::future::Future;
use std::sync::Arc;

/// Spawns `work` onto the connection's task actor and responds with the
/// serialized result when it settles, honoring `$/cancel_request`.
///
/// Handler callbacks run inline on the SDK dispatch loop, so awaiting bridge
/// work there would block every subsequent message on the connection,
/// including the cancellation notification itself. If the peer cancels first,
/// `work` is dropped and the response is the SDK's standard
/// `Error::request_cancelled`.
///
/// The responder is typed `serde_json::Value` because the [`ClientRequest`]
/// enum's response type is `serde_json::Value`; typed responses serialize to
/// the same JSON the SDK's per-type `into_json` produces.
fn respond_in_task<T: serde::Serialize + Send + 'static>(
    cx: &ConnectionTo<Client>,
    responder: Responder<serde_json::Value>,
    work: impl Future<Output = Result<T>> + Send + 'static,
) -> Result<()> {
    cx.spawn(async move {
        let cancellation = responder.cancellation();
        let work = async move {
            let response = work.await?;
            serde_json::to_value(response).map_err(Error::into_internal_error)
        };
        responder.respond_with_result(cancellation.run_until_cancelled(work).await)
    })
}

fn log_notification_error(method: &'static str, result: Result<()>) {
    if let Err(error) = result {
        tracing::warn!(error = %error, method, "notification handler failed");
    }
}

async fn handle_client_dispatch<A: AgentHandler + Send + Sync + 'static>(
    agent: Arc<A>,
    dispatch: Dispatch<ClientRequest, ClientNotification>,
    cx: ConnectionTo<Client>,
) -> Result<()> {
    match dispatch {
        Dispatch::Request(request, responder) => match request {
            ClientRequest::InitializeRequest(req) => {
                respond_in_task(&cx, responder, async move { agent.initialize(req).await })
            }
            ClientRequest::AuthenticateRequest(req) => {
                respond_in_task(&cx, responder, async move { agent.authenticate(req).await })
            }
            ClientRequest::LogoutRequest(req) => {
                respond_in_task(&cx, responder, async move { agent.logout(req).await })
            }
            ClientRequest::NewSessionRequest(req) => {
                respond_in_task(&cx, responder, async move { agent.new_session(req).await })
            }
            ClientRequest::LoadSessionRequest(req) => {
                respond_in_task(&cx, responder, async move { agent.load_session(req).await })
            }
            ClientRequest::PromptRequest(req) => {
                respond_in_task(&cx, responder, async move { agent.prompt(req).await })
            }
            ClientRequest::SetSessionModeRequest(req) => {
                respond_in_task(&cx, responder, async move { agent.set_session_mode(req).await })
            }
            ClientRequest::SetSessionConfigOptionRequest(req) => {
                respond_in_task(
                    &cx,
                    responder,
                    async move { agent.set_session_config_option(req).await },
                )
            }
            ClientRequest::ForkSessionRequest(req) => {
                respond_in_task(&cx, responder, async move { agent.fork_session(req).await })
            }
            ClientRequest::ResumeSessionRequest(req) => {
                respond_in_task(&cx, responder, async move { agent.resume_session(req).await })
            }
            ClientRequest::CloseSessionRequest(req) => {
                respond_in_task(&cx, responder, async move { agent.close_session(req).await })
            }
            ClientRequest::DeleteSessionRequest(req) => {
                respond_in_task(&cx, responder, async move { agent.delete_session(req).await })
            }
            ClientRequest::ListSessionsRequest(req) => {
                respond_in_task(&cx, responder, async move { agent.list_sessions(req).await })
            }
            ClientRequest::ExtMethodRequest(req) => {
                respond_in_task(&cx, responder, async move { agent.ext_method(req).await })
            }
            _ => responder.respond_with_error(Error::method_not_found()),
        },
        Dispatch::Notification(notification) => match notification {
            ClientNotification::CancelNotification(notif) => cx.spawn(async move {
                log_notification_error("session/cancel", agent.cancel(notif).await);
                Ok(())
            }),
            ClientNotification::ExtNotification(notif) => cx.spawn(async move {
                log_notification_error("ext notification", agent.ext_notification(notif).await);
                Ok(())
            }),
            _ => Ok(()),
        },
        Dispatch::Response(result, router) => router.respond_with_result(result),
    }
}

/// Connects an [`AgentHandler`] implementation to a byte-stream boundary.
///
/// Registers a single typed dispatch handler over the SDK's
/// [`ClientRequest`]/[`ClientNotification`] enums, whose method table already
/// covers every routed method, and delegates each variant to the
/// [`AgentHandler`] implementation, matching ADR 0020's "SDK builder
/// callbacks adapt boundaries to bridge traits" decision. Methods the bridge
/// does not route answer `method_not_found`; responses to the bridge's own
/// outbound requests pass through to their waiting callers. Shared by
/// `acp-nats-server` (WebSocket and HTTP duplex) and `acp-nats-stdio`.
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

    let builder = Agent
        .builder()
        .on_receive_notification(
            async move |_notif: CancelRequestNotification, _cx| Ok(()),
            agent_client_protocol::on_receive_notification!(),
        )
        .on_receive_dispatch(
            {
                let agent = agent.clone();
                async move |dispatch: Dispatch<ClientRequest, ClientNotification>, cx| {
                    handle_client_dispatch(agent.clone(), dispatch, cx).await
                }
            },
            agent_client_protocol::on_receive_dispatch!(),
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
