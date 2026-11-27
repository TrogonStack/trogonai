use crate::initialized_agent::InitializedAgent;
use acp_nats::{AcpPrefix, AgentHandler};
use acp_nats_agent::{AgentSideNatsConnection, ConnectionError};
use std::future::Future;
use trogon_nats::jetstream::{JetStreamGetStream, JsDispatchMessage, JsMessageOf};
use trogon_nats::{FlushClient, PublishClient, RequestClient, SubscribeClient};

/// Serves one child run on the host's ACP prefix until NATS serving ends.
///
/// `initialize` names its client in `clientInfo`, which [`InitializedAgent`]
/// checks against the claim; every other request names it in the
/// [`ACP_CLIENT_HEADER`](acp_nats::ACP_CLIENT_HEADER), which the same claim
/// checks as the connection's gate, so a refused client's `session/new`,
/// `session/prompt` and the rest never reach the child.
///
/// Serving ends when `stop` resolves, once every COMMANDS message this run
/// holds has been handed back for redelivery (see
/// [`AgentSideNatsConnection::with_jetstream_gated_until`]), so a host that is
/// shutting down leaves no command waiting out `ack_wait`.
///
/// Runs `!Send` tasks: call it inside a `LocalSet`.
pub fn serve<A, N, J>(
    agent: InitializedAgent<A>,
    nats: N,
    js: J,
    prefix: AcpPrefix,
    stop: impl Future<Output = ()> + 'static,
) -> impl Future<Output = Result<(), ConnectionError>>
where
    A: AgentHandler + Send + Sync + 'static,
    N: SubscribeClient + RequestClient + PublishClient + FlushClient + Clone + 'static,
    J: JetStreamGetStream + 'static,
    JsMessageOf<J>: JsDispatchMessage,
{
    let gate = agent.claim();
    let (_connection, serving) = AgentSideNatsConnection::with_jetstream_gated_until(
        agent,
        gate,
        nats,
        js,
        prefix,
        |task| {
            tokio::task::spawn_local(task);
        },
        stop,
    );
    serving
}

#[cfg(test)]
mod tests;
