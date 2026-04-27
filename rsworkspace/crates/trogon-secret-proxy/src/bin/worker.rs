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
//!
//! # Vault backend selection (in priority order)
//!
//! ## DualWrite mode (`INFISICAL_URL` + `VAULT_NATS_BUCKET` both set)
//!
//! Infisical is the source of truth; NatsKvVault is the encrypted NATS KV cache.
//!
//! | Variable                  | Description                                          |
//! |---------------------------|------------------------------------------------------|
//! | `INFISICAL_URL`           | Infisical base URL                                   |
//! | `INFISICAL_PROJECT_ID`    | Infisical workspace ID                               |
//! | `INFISICAL_SERVICE_TOKEN` | Infisical service token                              |
//! | `VAULT_NATS_BUCKET`       | KV bucket name (e.g. `prod`)                        |
//! | `VAULT_MASTER_PASSWORD`   | AES master password (Argon2id KDF)                   |
//! | `VAULT_KEY_SALT`          | KDF salt (≥ 8 bytes)                                |
//! | `VAULT_GRACE_PERIOD_SECS` | Rotation grace period (default 30)                   |
//!
//! ## Infisical-only mode (`INFISICAL_URL` set, `VAULT_NATS_BUCKET` absent)
//!
//! Same `INFISICAL_*` variables as above.
//!
//! ## NATS KV mode (`VAULT_NATS_BUCKET` set, `INFISICAL_URL` absent)
//!
//! Same `VAULT_*` variables as DualWrite mode except the Infisical vars.
//!
//! ## HashiCorp Vault mode (`VAULT_ADDR` set)
//!
//! | Variable              | Default   | Description                                      |
//! |-----------------------|-----------|--------------------------------------------------|
//! | `VAULT_ADDR`          | —         | URL of the Vault server (e.g. `http://vault:8200`)|
//! | `VAULT_MOUNT`         | `secret`  | KV v2 mount name                                 |
//! | `VAULT_AUTH_METHOD`   | `token`   | Auth method: `token`, `approle`, `kubernetes`    |
//! | `VAULT_TOKEN`         | —         | Static token (method `token`)                    |
//! | `VAULT_ROLE_ID`       | —         | Role ID (method `approle`)                       |
//! | `VAULT_SECRET_ID`     | —         | Secret ID (method `approle`)                     |
//! | `VAULT_K8S_ROLE`      | —         | Kubernetes role (method `kubernetes`)            |
//! | `VAULT_K8S_JWT_PATH`  | —         | Path to JWT file (optional, has K8s default)     |
//!
//! ## MemoryVault mode (default, when none of the above are set)
//!
//! | Variable              | Description                                          |
//! |-----------------------|------------------------------------------------------|
//! | `VAULT_TOKEN_<token>` | Seed vault: key=token name, value=real API key       |
//!
//! ```
//! VAULT_TOKEN_tok_anthropic_prod_abc123=sk-ant-realkey...
//! ```

use std::sync::Arc;
use std::time::Duration;

use trogon_nats::{NatsConfig, connect};
use trogon_secret_proxy::{stream, subjects, vault_admin, worker};
use trogon_std::SystemEnv;
use trogon_vault::{ApiKeyToken, DualWriteVault, MemoryVault, VaultStore};
use trogon_vault::{HashicorpVaultConfig, HashicorpVaultStore, VaultAuth};
use trogon_vault::{InfisicalConfig, InfisicalVaultStore};
use trogon_vault_nats::{AuditPublisher, CryptoCtx, NatsKvVault, ensure_audit_stream, ensure_vault_bucket, grace_period_from_env};

async fn run_all<V: VaultStore + 'static>(
    nats: async_nats::Client,
    jetstream: async_nats::jetstream::Context,
    vault: Arc<V>,
    http_client: reqwest::Client,
    consumer_name: &str,
    stream_name: &str,
    prefix: &str,
) where
    V::Error: std::fmt::Display,
{
    tokio::spawn({
        let nats = nats.clone();
        let vault = Arc::clone(&vault);
        let prefix = prefix.to_string();
        async move {
            vault_admin::run(nats, vault, &prefix, None)
                .await
                .expect("Vault admin listener exited with error");
        }
    });

    worker::run(
        jetstream,
        nats,
        vault,
        http_client,
        consumer_name,
        stream_name,
    )
    .await
    .expect("Worker exited with error");
}

async fn build_hashicorp_vault(vault_addr: String) -> HashicorpVaultStore {
    let mount = std::env::var("VAULT_MOUNT").unwrap_or_else(|_| "secret".to_string());
    let auth = match std::env::var("VAULT_AUTH_METHOD").as_deref() {
        Ok("approle") => VaultAuth::AppRole {
            role_id:   std::env::var("VAULT_ROLE_ID").expect("VAULT_ROLE_ID required"),
            secret_id: std::env::var("VAULT_SECRET_ID").expect("VAULT_SECRET_ID required"),
        },
        Ok("kubernetes") => VaultAuth::Kubernetes {
            role:     std::env::var("VAULT_K8S_ROLE").expect("VAULT_K8S_ROLE required"),
            jwt_path: std::env::var("VAULT_K8S_JWT_PATH").ok(),
        },
        _ => VaultAuth::Token(
            std::env::var("VAULT_TOKEN").expect("VAULT_TOKEN required for token auth"),
        ),
    };
    HashicorpVaultStore::new(HashicorpVaultConfig::new(vault_addr, mount, auth))
        .await
        .expect("Failed to connect to HashiCorp Vault")
}

async fn build_nats_kv_vault(
    jetstream:   &async_nats::jetstream::Context,
    bucket_name: &str,
    audit:       Arc<AuditPublisher>,
) -> NatsKvVault {
    let kv = ensure_vault_bucket(jetstream, bucket_name, 1)
        .await
        .expect("Failed to ensure vault KV bucket");
    let crypto = Arc::new(
        CryptoCtx::from_env()
            .expect("VAULT_MASTER_PASSWORD and VAULT_KEY_SALT are required for NATS KV vault"),
    );
    NatsKvVault::new(kv, crypto, grace_period_from_env())
        .await
        .expect("Failed to initialise NatsKvVault")
        .with_audit(audit)
}

#[tokio::main]
async fn main() {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .init();

    let prefix        = std::env::var("PROXY_PREFIX").unwrap_or_else(|_| "trogon".to_string());
    let consumer_name = std::env::var("WORKER_CONSUMER_NAME").unwrap_or_else(|_| "proxy-workers".to_string());

    let nats_config = NatsConfig::from_env(&SystemEnv);
    tracing::info!(servers = ?nats_config.servers, "Connecting to NATS");

    let nats = connect(&nats_config, Duration::from_secs(10))
        .await
        .expect("Failed to connect to NATS");

    let jetstream = async_nats::jetstream::new(nats.clone());

    let outbound_subject = subjects::outbound(&prefix);
    let sname = stream::stream_name(&prefix);
    stream::ensure_stream(&jetstream, &prefix, &outbound_subject)
        .await
        .expect("Failed to ensure JetStream stream");

    let http_client = reqwest::Client::builder()
        .timeout(Duration::from_secs(30))
        // Never follow HTTP redirects — the real API key must not be forwarded
        // to an uncontrolled redirect target.
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .expect("Failed to build HTTP client");

    let has_infisical = std::env::var("INFISICAL_URL").is_ok();
    let has_nats_kv   = std::env::var("VAULT_NATS_BUCKET").is_ok();
    let has_hashicorp = std::env::var("VAULT_ADDR").is_ok();

    tracing::info!(consumer = %consumer_name, "Detokenization worker starting");

    match (has_infisical, has_nats_kv, has_hashicorp) {
        (true, true, _) => {
            tracing::info!("Vault backend: DualWrite (Infisical primary + NATS KV cache)");
            let config    = InfisicalConfig::from_env().expect("Infisical env vars required");
            let infisical = InfisicalVaultStore::new(config);
            let bucket    = std::env::var("VAULT_NATS_BUCKET").unwrap();
            ensure_audit_stream(&jetstream).await.expect("Failed to ensure VAULT_AUDIT stream");
            let audit   = Arc::new(AuditPublisher::new(jetstream.clone(), &bucket));
            let nats_kv = build_nats_kv_vault(&jetstream, &bucket, audit).await;
            let vault   = Arc::new(DualWriteVault::new(infisical, nats_kv));
            run_all(nats, jetstream, vault, http_client, &consumer_name, &sname, &prefix).await;
        }
        (true, false, _) => {
            tracing::info!("Vault backend: Infisical");
            let config = InfisicalConfig::from_env().expect("Infisical env vars required");
            let vault  = Arc::new(InfisicalVaultStore::new(config));
            run_all(nats, jetstream, vault, http_client, &consumer_name, &sname, &prefix).await;
        }
        (false, true, _) => {
            let bucket = std::env::var("VAULT_NATS_BUCKET").unwrap();
            tracing::info!(bucket = %bucket, "Vault backend: NATS KV");
            ensure_audit_stream(&jetstream).await.expect("Failed to ensure VAULT_AUDIT stream");
            let audit = Arc::new(AuditPublisher::new(jetstream.clone(), &bucket));
            let vault = Arc::new(build_nats_kv_vault(&jetstream, &bucket, audit).await);
            run_all(nats, jetstream, vault, http_client, &consumer_name, &sname, &prefix).await;
        }
        (false, false, true) => {
            let vault_addr = std::env::var("VAULT_ADDR").unwrap();
            tracing::info!(addr = %vault_addr, "Vault backend: HashiCorp Vault");
            let vault = Arc::new(build_hashicorp_vault(vault_addr).await);
            run_all(nats, jetstream, vault, http_client, &consumer_name, &sname, &prefix).await;
        }
        _ => {
            tracing::info!("Vault backend: in-memory (development mode)");
            let vault = Arc::new(MemoryVault::new());
            let mut seeded = 0usize;
            for (key, value) in std::env::vars() {
                let Some(token_str) = key.strip_prefix("VAULT_TOKEN_") else { continue };
                match ApiKeyToken::new(token_str) {
                    Ok(token) => {
                        vault.store(&token, &value).await.expect("Failed to store vault token");
                        tracing::info!(token = %token, "Vault token seeded");
                        seeded += 1;
                    }
                    Err(e) => {
                        tracing::warn!(env_key = %key, error = %e, "Skipping invalid VAULT_TOKEN_* env var");
                    }
                }
            }
            tracing::info!(seeded, "Vault seeding complete");
            run_all(nats, jetstream, vault, http_client, &consumer_name, &sname, &prefix).await;
        }
    }
}
