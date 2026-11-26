use std::net::SocketAddr;

use axum::http::{StatusCode, header};
use axum::response::{IntoResponse, Response};
use prometheus::Registry;

use crate::{ObservabilityError, gather_text};

const METRICS_CONTENT_TYPE: &str = "text/plain; charset=utf-8";

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

/// Serve `GET /metrics` on `addr` until the listener fails.
pub async fn serve_metrics(addr: SocketAddr, registry: Registry) -> Result<(), ObservabilityError> {
    let listener = tokio::net::TcpListener::bind(addr)
        .await
        .map_err(|source| ObservabilityError::Listener { addr, source })?;
    tracing::info!(%addr, "prometheus metrics listening");
    axum::serve(listener, router(registry))
        .await
        .map_err(|source| ObservabilityError::Listener { addr, source })
}

/// Start the `/metrics` HTTP server in the background.
///
/// Must be called from within a Tokio runtime.
pub fn spawn_metrics_server(addr: SocketAddr, registry: Registry) {
    tokio::spawn(async move {
        if let Err(error) = serve_metrics(addr, registry).await {
            tracing::warn!(%error, %addr, "metrics server exited");
        }
    });
}

#[cfg(test)]
mod tests;
