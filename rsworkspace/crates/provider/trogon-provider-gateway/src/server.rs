use std::future::Future;
use std::net::SocketAddr;

use axum::Router;
use tokio::net::TcpListener;

#[derive(Debug, thiserror::Error)]
pub(crate) enum ServerError {
    #[error("failed to bind HTTP listener at {address}")]
    Bind {
        address: SocketAddr,
        #[source]
        source: std::io::Error,
    },
    #[error("failed to inspect HTTP listener address")]
    LocalAddress(#[source] std::io::Error),
    #[error("HTTP server failed")]
    Serve(#[source] std::io::Error),
}

pub(crate) async fn bind(address: SocketAddr) -> Result<TcpListener, ServerError> {
    TcpListener::bind(address)
        .await
        .map_err(|source| ServerError::Bind { address, source })
}

pub(crate) async fn serve(
    listener: TcpListener,
    router: Router,
    health: trogon_provider_gateway::Health,
    shutdown: impl Future<Output = ()> + Send + 'static,
) -> Result<(), ServerError> {
    let address = listener.local_addr().map_err(ServerError::LocalAddress)?;
    tracing::info!(%address, "provider gateway listening");

    let shutdown = async move {
        shutdown.await;
        health.begin_shutdown();
    };
    axum::serve(listener, router)
        .with_graceful_shutdown(shutdown)
        .await
        .map_err(ServerError::Serve)
}

#[cfg(test)]
mod tests;
