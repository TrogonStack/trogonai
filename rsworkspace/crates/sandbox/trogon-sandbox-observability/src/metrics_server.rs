use std::net::SocketAddr;

use axum::http::{StatusCode, header};
use axum::response::{IntoResponse, Response};
use prometheus::Registry;

use crate::constants::METRICS_CONTENT_TYPE;
use crate::{ObservabilityError, gather_text};

fn metrics_response(body: Result<String, ObservabilityError>) -> Response {
    match body {
        Ok(body) => ([(header::CONTENT_TYPE, METRICS_CONTENT_TYPE)], body).into_response(),
        Err(error) => (StatusCode::INTERNAL_SERVER_ERROR, error.to_string()).into_response(),
    }
}

fn router(registry: Registry) -> axum::Router {
    axum::Router::new().route(
        "/metrics",
        axum::routing::get(move || {
            let registry = registry.clone();
            async move { metrics_response(gather_text(&registry)) }
        }),
    )
}

fn listener_error(addr: SocketAddr) -> impl FnOnce(std::io::Error) -> ObservabilityError {
    move |source| ObservabilityError::Listener { addr, source }
}

/// Serve `GET /metrics` on `addr` until the listener fails.
pub async fn serve_metrics(addr: SocketAddr, registry: Registry) -> Result<(), ObservabilityError> {
    let listener = tokio::net::TcpListener::bind(addr)
        .await
        .map_err(listener_error(addr))?;
    serve_metrics_on(listener, registry, std::future::pending()).await
}

/// Serve `GET /metrics` on an already bound `listener` until it fails or `shutdown` completes.
///
/// Binding first lets a caller hold the port, for example one chosen by the OS, before serving.
/// Once `shutdown` completes the server stops accepting, drains open requests, and returns `Ok`.
pub async fn serve_metrics_on(
    listener: tokio::net::TcpListener,
    registry: Registry,
    shutdown: impl Future<Output = ()> + Send + 'static,
) -> Result<(), ObservabilityError> {
    let addr = listener.local_addr().map_err(ObservabilityError::ListenerAddress)?;
    tracing::info!(%addr, "prometheus metrics listening");
    let server = axum::serve(listener, router(registry)).with_graceful_shutdown(shutdown);
    served(addr, server.await)
}

fn served(addr: SocketAddr, result: std::io::Result<()>) -> Result<(), ObservabilityError> {
    result.map_err(listener_error(addr))
}

fn log_exit(addr: SocketAddr, result: Result<(), ObservabilityError>) {
    if let Err(error) = result {
        tracing::warn!(%error, %addr, "metrics server exited");
    }
}

/// Start the `/metrics` HTTP server in the background.
///
/// Must be called from within a Tokio runtime.
pub fn spawn_metrics_server(addr: SocketAddr, registry: Registry) {
    tokio::spawn(async move {
        log_exit(addr, serve_metrics(addr, registry).await);
    });
}

#[cfg(test)]
mod tests;
