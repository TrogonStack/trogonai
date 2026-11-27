use super::boundary_exit::BoundaryExit;
use crate::client_handler::ClientHandler;
use agent_client_protocol::v1::schema::{AgentNotification, AgentRequest, CancelRequestNotification};
use agent_client_protocol::v1::{ConnectionTo, Dispatch, Responder};
use agent_client_protocol::{
    Agent, Client, ConnectTo, Error, ErrorCode, JsonRpcMessage, JsonRpcRequest, Result, UntypedMessage,
};
use std::future::Future;
use std::sync::Arc;

/// An agent-originated request as the client boundary sees it: either one the
/// SDK's [`AgentRequest`] method table knows, or an unknown method kept so the
/// boundary can answer `method_not_found` instead of failing the connection.
#[derive(Debug, Clone)]
enum AgentBoundaryRequest {
    Routed(Box<AgentRequest>),
    Unsupported(UntypedMessage),
}

impl JsonRpcMessage for AgentBoundaryRequest {
    fn matches_method(_method: &str) -> bool {
        true
    }

    fn method(&self) -> &str {
        match self {
            Self::Routed(request) => request.method(),
            Self::Unsupported(request) => request.method(),
        }
    }

    fn to_untyped_message(&self) -> Result<UntypedMessage> {
        match self {
            Self::Routed(request) => request.to_untyped_message(),
            Self::Unsupported(request) => request.to_untyped_message(),
        }
    }

    fn parse_message(method: &str, params: &serde_json::Value) -> Result<Self> {
        match AgentRequest::parse_message(method, params) {
            Ok(request) => Ok(Self::Routed(Box::new(request))),
            Err(error) if error.code == ErrorCode::METHOD_NOT_FOUND => {
                UntypedMessage::parse_message(method, params).map(Self::Unsupported)
            }
            Err(error) => Err(error),
        }
    }
}

impl JsonRpcRequest for AgentBoundaryRequest {
    type Response = serde_json::Value;
}

/// Runs `work` off the dispatch loop and responds when it settles, honoring
/// `$/cancel_request`. Same reasoning as the agent boundary: awaiting inline
/// would block every later message on the connection, including the agent's
/// own `session/update` stream for the prompt that is waiting on this answer.
fn respond_in_task<T: serde::Serialize + Send + 'static>(
    cx: &ConnectionTo<Agent>,
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
        tracing::warn!(error = %error, method, "agent notification handler failed");
    }
}

async fn handle_agent_dispatch<C: ClientHandler + Send + Sync + 'static>(
    client: Arc<C>,
    dispatch: Dispatch<AgentBoundaryRequest, AgentNotification>,
    cx: ConnectionTo<Agent>,
) -> Result<()> {
    match dispatch {
        Dispatch::Request(request, responder) => {
            let request = match request {
                AgentBoundaryRequest::Routed(request) => Some(*request),
                AgentBoundaryRequest::Unsupported(_) => None,
            };
            match request {
                Some(AgentRequest::RequestPermissionRequest(req)) => {
                    respond_in_task(&cx, responder, async move { client.request_permission(req).await })
                }
                Some(AgentRequest::ReadTextFileRequest(req)) => {
                    respond_in_task(&cx, responder, async move { client.read_text_file(req).await })
                }
                Some(AgentRequest::WriteTextFileRequest(req)) => {
                    respond_in_task(&cx, responder, async move { client.write_text_file(req).await })
                }
                Some(AgentRequest::CreateTerminalRequest(req)) => {
                    respond_in_task(&cx, responder, async move { client.create_terminal(req).await })
                }
                Some(AgentRequest::TerminalOutputRequest(req)) => {
                    respond_in_task(&cx, responder, async move { client.terminal_output(req).await })
                }
                Some(AgentRequest::ReleaseTerminalRequest(req)) => {
                    respond_in_task(&cx, responder, async move { client.release_terminal(req).await })
                }
                Some(AgentRequest::WaitForTerminalExitRequest(req)) => {
                    respond_in_task(&cx, responder, async move { client.wait_for_terminal_exit(req).await })
                }
                Some(AgentRequest::KillTerminalRequest(req)) => {
                    respond_in_task(&cx, responder, async move { client.kill_terminal(req).await })
                }
                Some(AgentRequest::CreateElicitationRequest(req)) => {
                    respond_in_task(&cx, responder, async move { client.elicitation_create(req).await })
                }
                Some(AgentRequest::ExtMethodRequest(req)) => {
                    respond_in_task(&cx, responder, async move { client.ext_method(req).await })
                }
                _ => responder.respond_with_error(Error::method_not_found()),
            }
        }
        // Notifications are forwarded inline, unlike requests: the agent writes
        // a turn's `session/update`s before its `session/prompt` response, and
        // the dispatch loop only reaches that response once these return. A
        // spawned forward could still be queued when the response goes out,
        // so the client would end the turn before its text arrived and the
        // late chunks would land in the next one.
        Dispatch::Notification(notification) => {
            match notification {
                AgentNotification::SessionNotification(notif) => {
                    log_notification_error("session/update", client.session_notification(notif).await);
                }
                AgentNotification::CompleteElicitationNotification(notif) => {
                    log_notification_error("elicitation/complete", client.elicitation_complete(notif).await);
                }
                AgentNotification::ExtNotification(notif) => {
                    log_notification_error("ext notification", client.ext_notification(notif).await);
                }
                _ => {}
            }
            Ok(())
        }
        Dispatch::Response(result, router) => router.route_with_result(result),
    }
}

/// Connects a [`ClientHandler`] to an agent reached through any SDK transport.
///
/// The mirror of [`connect_agent_boundary_with`](super::connect_agent_boundary_with):
/// this side plays the ACP *client* role, so it is what a host uses in front of
/// an agent it spawned (an [`AcpAgent`](agent_client_protocol::AcpAgent)
/// subprocess is a `ConnectTo<Client>` transport). Every request and
/// notification the agent sends is delegated to `client`; `main_fn` receives
/// the handle used to call into the agent. The connection ends when `main_fn`
/// returns or the agent closes its side, which for a subprocess means it
/// exited.
pub async fn connect_client_boundary_with<C, Tr, T>(
    client: Arc<C>,
    transport: Tr,
    main_fn: impl AsyncFnOnce(ConnectionTo<Agent>) -> Result<T>,
) -> Result<BoundaryExit<T>>
where
    C: ClientHandler + Send + Sync + 'static,
    Tr: ConnectTo<Client>,
{
    Client
        .v1()
        .on_receive_notification(
            async move |_notif: CancelRequestNotification, _cx| Ok(()),
            agent_client_protocol::on_receive_notification!(),
        )
        .on_receive_dispatch(
            {
                let client = client.clone();
                async move |dispatch: Dispatch<AgentBoundaryRequest, AgentNotification>, cx| {
                    handle_agent_dispatch(client.clone(), dispatch, cx).await
                }
            },
            agent_client_protocol::on_receive_dispatch!(),
        )
        .connect_with(transport, async move |cx| {
            let closed = cx.clone();
            tokio::select! {
                result = main_fn(cx) => result.map(BoundaryExit::Main),
                () = closed.incoming_closed() => Ok(BoundaryExit::TransportClosed),
            }
        })
        .await
}

#[cfg(test)]
mod tests;
