use crate::initialized_agent::InitializedAgent;
use crate::streams;
use acp_nats::jetstream::provision::ProvisionError;
use acp_nats::{AcpPrefix, AgentHandler};
use acp_nats_agent::{AgentSideNatsConnection, ConnectionError};
use async_nats::jetstream::context::GetStreamError;
use std::future::Future;
use tracing::warn;
use trogon_nats::jetstream::{
    JetStreamContext, JetStreamGetStream, JetStreamGetStreamInfo, JsDispatchMessage, JsMessageOf,
};
use trogon_nats::{FlushClient, PublishClient, RequestClient, SubscribeClient};

/// Why a run's NATS side ended with an error.
#[derive(Debug, thiserror::Error)]
pub enum ServeError<P> {
    #[error("could not provision the host's JetStream streams: {0}")]
    Provision(#[source] P),
    #[error(transparent)]
    Connection(#[from] ConnectionError),
}

/// [`serve`]s the run and, if serving ends with an error of any kind,
/// provisions the host's streams again before reporting it, so the next run
/// consumes from streams a NATS server lost with its storage (see "Losing
/// JetStream storage" in the README). A run that serves normally provisions
/// nothing, to subscribe as soon as it can.
pub async fn serve_and_reprovision<A, N, J>(
    agent: InitializedAgent<A>,
    nats: N,
    js: J,
    prefix: AcpPrefix,
    stop: impl Future<Output = ()> + 'static,
) -> Result<(), ServeError<ProvisionError<<J as JetStreamContext>::Error>>>
where
    A: AgentHandler + Send + Sync + 'static,
    N: SubscribeClient + RequestClient + PublishClient + FlushClient + Clone + 'static,
    J: JetStreamContext + JetStreamGetStream<Error = GetStreamError> + 'static,
    <J as JetStreamGetStream>::Stream: JetStreamGetStreamInfo,
    JsMessageOf<J>: JsDispatchMessage,
{
    let Err(error) = serve(agent, nats, js.clone(), prefix.clone(), stop).await else {
        return Ok(());
    };
    warn!(error = %error, "NATS serving failed; provisioning the streams again for the next run");
    streams::provision(&js, &prefix).await.map_err(ServeError::Provision)?;
    Err(error.into())
}

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
/// [`AgentSideNatsConnection::with_jetstream_gated_until`]), so a run that
/// ends (the host shutting down, a relaunch, the child dying; see
/// [`crate::supervisor::run_once`]) leaves no command waiting out `ack_wait`.
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
