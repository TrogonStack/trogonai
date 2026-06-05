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
use trogon_frontend_proxy::protected_resource::{self, GatewayDiscovery};
use trogon_frontend_proxy::proxy::{ProxyService, dispatch};
use trogon_frontend_proxy::routes::InMemoryRouteStore;
use trogon_frontend_proxy::sts_minter::{StaticSubjectProvider, StsMinter};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    tracing_subscriber_init();

    let routes = Arc::new(InMemoryRouteStore::new());
    let http = reqwest::Client::builder().build()?;
    let minter = Arc::new(build_minter_from_env(http.clone())?);
    let service = Arc::new(ProxyService::new(routes, minter, http));

    let discovery = Arc::new(load_discovery_from_env()?);

    let dispatch_router: Router = Router::new()
        .route("/{*path}", any(dispatch::<InMemoryRouteStore, StsMinter>))
        .with_state(service);

    let app: Router = protected_resource::router(discovery).merge(dispatch_router);

    let addr: std::net::SocketAddr = "0.0.0.0:8080".parse()?;
    let listener = tokio::net::TcpListener::bind(addr).await?;
    tracing::info!(%addr, "frontend proxy listening");

    axum::serve(listener, app)
        .with_graceful_shutdown(shutdown_signal())
        .await?;
    Ok(())
}

fn build_minter_from_env(http: reqwest::Client) -> Result<StsMinter, Box<dyn std::error::Error + Send + Sync>> {
    let endpoint = std::env::var("PROXY_STS_TOKEN_ENDPOINT")
        .map_err(|_| "PROXY_STS_TOKEN_ENDPOINT must be set to the STS /oauth2/token URL")?;
    let subject_token = std::env::var("PROXY_STS_SUBJECT_TOKEN")
        .map_err(|_| "PROXY_STS_SUBJECT_TOKEN must be set to the proxy's workload identity JWT")?;
    Ok(StsMinter::new(
        http,
        endpoint,
        Arc::new(StaticSubjectProvider::new(subject_token)),
    ))
}

fn load_discovery_from_env() -> Result<GatewayDiscovery, Box<dyn std::error::Error + Send + Sync>> {
    let base_url = std::env::var("PROXY_PUBLIC_BASE_URL").unwrap_or_else(|_| "https://gateway.local".into());
    let issuer = std::env::var("PROXY_ISSUER").unwrap_or_else(|_| format!("{}/aauth", base_url.trim_end_matches('/')));
    let authorization_endpoint = std::env::var("PROXY_AUTHORIZATION_ENDPOINT")
        .unwrap_or_else(|_| format!("{}/oauth/authorize", issuer.trim_end_matches('/')));
    let token_endpoint = std::env::var("PROXY_TOKEN_ENDPOINT")
        .unwrap_or_else(|_| format!("{}/oauth/token", issuer.trim_end_matches('/')));
    let jwks_uri = std::env::var("PROXY_JWKS_URI")
        .unwrap_or_else(|_| format!("{}/.well-known/jwks.json", issuer.trim_end_matches('/')));
    Ok(GatewayDiscovery {
        base_url,
        issuer,
        authorization_endpoint,
        token_endpoint,
        jwks_uri,
    })
}

fn tracing_subscriber_init() {
    let _ = tracing::subscriber::set_global_default(tracing::subscriber::NoSubscriber::default());
}

async fn shutdown_signal() {
    let _ = signal::ctrl_c().await;
}
