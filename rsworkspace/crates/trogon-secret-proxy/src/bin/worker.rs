//! Detokenization worker binary.
//!
//! Pulls requests from JetStream, resolves tokens via the vault,
//! forwards to the AI provider, and replies via Core NATS.
//!
//! # Environment variables
//!
//! | Variable                  | Default          | Description                                    |
//! |---------------------------|------------------|------------------------------------------------|
//! | `NATS_URL`                | localhost:4222   | NATS server address(es)                        |
//! | `PROXY_PREFIX`            | `trogon`         | NATS subject prefix                            |
//! | `WORKER_CONSUMER_NAME`    | `proxy-workers`  | Durable JetStream consumer name                |
//! | `RUST_LOG`                | `info`           | Log filter (tracing-subscriber)                |
//! | `VAULT_TOKEN_<token>`     | —                | Seed vault: key=token name, value=real API key |
//!
//! ## Vault seeding via environment
//!
//! Set one env var per token:
//!
//! ```
//! VAULT_TOKEN_tok_anthropic_prod_abc123=sk-ant-realkey...
//! VAULT_TOKEN_tok_openai_prod_xyz789=sk-realkey...
//! ```

use std::sync::Arc;
use std::time::Duration;

use trogon_nats::{NatsConfig, connect};
use trogon_std::SystemEnv;
use trogon_secret_proxy::{stream, subjects, vault_admin, worker};
use trogon_vault::{ApiKeyToken, MemoryVault, VaultStore};

#[tokio::main]
async fn main() {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .init();

    let prefix = std::env::var("PROXY_PREFIX").unwrap_or_else(|_| "trogon".to_string());
    let consumer_name = std::env::var("WORKER_CONSUMER_NAME")
        .unwrap_or_else(|_| "proxy-workers".to_string());

    // Seed vault from VAULT_TOKEN_* env vars.
    let vault = Arc::new(MemoryVault::new());
    let mut seeded = 0usize;
    for (key, value) in std::env::vars() {
        let Some(token_str) = key.strip_prefix("VAULT_TOKEN_") else {
            continue;
        };
        match ApiKeyToken::new(token_str) {
            Ok(token) => {
                vault
                    .store(&token, &value)
                    .await
                    .expect("Failed to store vault token");
                tracing::info!(token = %token, "Vault token seeded");
                seeded += 1;
            }
            Err(e) => {
                tracing::warn!(env_key = %key, error = %e, "Skipping invalid VAULT_TOKEN_* env var");
            }
        }
    }
    tracing::info!(seeded, "Vault seeding complete");

    let nats_config = NatsConfig::from_env(&SystemEnv);
    tracing::info!(servers = ?nats_config.servers, "Connecting to NATS");

    let nats = connect(&nats_config, Duration::from_secs(10))
        .await
        .expect("Failed to connect to NATS");

    let jetstream = Arc::new(async_nats::jetstream::new(nats.clone()));

    let outbound_subject = subjects::outbound(&prefix);
    let sname = stream::stream_name(&prefix);
    stream::ensure_stream(&jetstream, &prefix, &outbound_subject)
        .await
        .expect("Failed to ensure JetStream stream");

    let http_client = reqwest::Client::builder()
        .timeout(Duration::from_secs(30))
        // Never follow HTTP redirects.  In a VGS-like system the real API key
        // must not be forwarded to an uncontrolled redirect target.  Any 3xx
        // response is returned as-is to the caller via the proxy.
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .expect("Failed to build HTTP client");

    tracing::info!(consumer = %consumer_name, "Detokenization worker starting");

    tokio::spawn({
        let nats = nats.clone();
        let vault = Arc::clone(&vault);
        let prefix = prefix.clone();
        async move {
            vault_admin::run(nats, vault, &prefix)
                .await
                .expect("Vault admin listener exited with error");
        }
    });

    worker::run(jetstream, nats, vault, http_client, &consumer_name, &sname)
        .await
        .expect("Worker exited with error");
}
