//! `trogon-frontend-proxy` binary entry point.
//!
//! Wires the proxy library into an axum server. TLS termination,
//! JetStream-backed route store, and `trogon-sts` mesh-token mint
//! plug in here; the lib stays free of those couplings so each
//! piece is testable in isolation.

use std::sync::Arc;

use axum::Router;
use axum::routing::any;
use tokio::signal;
use trogon_frontend_proxy::proxy::{ProxyService, dispatch};
use trogon_frontend_proxy::routes::InMemoryRouteStore;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    tracing_subscriber_init();

    let routes = Arc::new(InMemoryRouteStore::new());
    let minter = Arc::new(NoopMinter);
    let http = reqwest::Client::builder().build()?;
    let service = Arc::new(ProxyService::new(routes, minter, http));

    let app: Router = Router::new()
        .route("/{*path}", any(dispatch::<InMemoryRouteStore, NoopMinter>))
        .with_state(service);

    let addr: std::net::SocketAddr = "0.0.0.0:8080".parse()?;
    let listener = tokio::net::TcpListener::bind(addr).await?;
    tracing::info!(%addr, "frontend proxy listening");

    axum::serve(listener, app)
        .with_graceful_shutdown(shutdown_signal())
        .await?;
    Ok(())
}

fn tracing_subscriber_init() {
    let _ = tracing::subscriber::set_global_default(tracing::subscriber::NoSubscriber::default());
}

async fn shutdown_signal() {
    let _ = signal::ctrl_c().await;
}

struct NoopMinter;

#[async_trait::async_trait]
impl trogon_frontend_proxy::proxy::TokenMinter for NoopMinter {
    async fn mint(&self, _audience: &str, _tenant: &str) -> Result<String, String> {
        Err("mesh token minter not configured".into())
    }
}
