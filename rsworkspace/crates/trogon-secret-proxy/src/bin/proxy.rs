//! HTTP proxy binary.
//!
//! Listens on `PROXY_PORT` (default 8080), receives HTTP calls from services,
//! publishes them to JetStream, and waits for the worker reply via Core NATS.
//!
//! # Environment variables
//!
//! | Variable                  | Default   | Description                          |
//! |---------------------------|-----------|--------------------------------------|
//! | `NATS_URL`                | localhost:4222 | NATS server address(es)         |
//! | `PROXY_PREFIX`            | `trogon`  | NATS subject prefix                  |
//! | `PROXY_PORT`              | `8080`    | TCP port to listen on                |
//! | `PROXY_WORKER_TIMEOUT_SECS` | `60`    | Seconds to wait for worker reply     |
//! | `PROXY_BASE_URL_OVERRIDE` | —         | Override AI provider base URL (tests)|
//! | `RUST_LOG`                | `info`    | Log filter (tracing-subscriber)      |

use std::sync::Arc;
use std::time::Duration;

use trogon_nats::{NatsConfig, connect};
use trogon_std::SystemEnv;
use trogon_secret_proxy::{proxy::{ProxyState, router}, stream, subjects};

#[tokio::main]
async fn main() {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .init();

    let prefix = std::env::var("PROXY_PREFIX").unwrap_or_else(|_| "trogon".to_string());
    let port: u16 = std::env::var("PROXY_PORT")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(8080);
    let timeout_secs: u64 = std::env::var("PROXY_WORKER_TIMEOUT_SECS")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(60);

    let nats_config = NatsConfig::from_env(&SystemEnv);
    tracing::info!(servers = ?nats_config.servers, "Connecting to NATS");

    let nats = connect(&nats_config, Duration::from_secs(10))
        .await
        .expect("Failed to connect to NATS");

    let jetstream = Arc::new(async_nats::jetstream::new(nats.clone()));

    let outbound_subject = subjects::outbound(&prefix);
    stream::ensure_stream(&jetstream, &prefix, &outbound_subject)
        .await
        .expect("Failed to ensure JetStream stream");

    tracing::info!(
        port,
        prefix = %prefix,
        outbound_subject = %outbound_subject,
        worker_timeout_secs = timeout_secs,
        "HTTP proxy starting"
    );

    let base_url_override = std::env::var("PROXY_BASE_URL_OVERRIDE").ok();

    let state = ProxyState {
        nats,
        jetstream,
        prefix,
        outbound_subject,
        worker_timeout: Duration::from_secs(timeout_secs),
        base_url_override,
    };

    let app = router(state);
    let listener = tokio::net::TcpListener::bind(format!("0.0.0.0:{}", port))
        .await
        .expect("Failed to bind TCP listener");

    tracing::info!(port, "Listening");
    axum::serve(listener, app).await.expect("Server error");
}
