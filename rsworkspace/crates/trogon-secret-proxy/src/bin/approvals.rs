//! Human-in-the-loop approval service binary.
//!
//! Listens for vault credential proposals, notifies via Slack, and writes
//! approved credentials to the configured vault backend.
//!
//! # Environment variables
//!
//! | Variable                  | Default          | Description                                    |
//! |---------------------------|------------------|------------------------------------------------|
//! | `NATS_URL`                | localhost:4222   | NATS server address(es)                        |
//! | `APPROVAL_VAULT_NAME`     | `default`        | Vault name this service manages                |
//! | `VAULT_SLACK_WEBHOOK_URL` | —                | Slack incoming-webhook URL (enables Slack)     |
//! | `RUST_LOG`                | `info`           | Log filter (tracing-subscriber)                |
//!
//! # Vault backend selection (same priority order as the worker binary)
//!
//! ## DualWrite mode (`INFISICAL_URL` + `VAULT_NATS_BUCKET` both set)
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
//! ## NATS KV mode (`VAULT_NATS_BUCKET` set, `INFISICAL_URL` absent)
//!
//! ## HashiCorp Vault mode (`VAULT_ADDR` set)
//!
//! ## MemoryVault mode (default — development only)

use std::sync::Arc;
use std::time::Duration;

use trogon_nats::{NatsConfig, connect};
use trogon_std::SystemEnv;
use trogon_vault::{ApiKeyToken, DualWriteVault, MemoryVault, VaultStore};
use trogon_vault::{HashicorpVaultConfig, HashicorpVaultStore, VaultAuth};
use trogon_vault::{InfisicalConfig, InfisicalVaultStore};
use trogon_vault_approvals::{
    ApprovalService, LoggingNotifier, Notifier, SlackWebhookNotifier,
    ensure_proposals_stream,
};
use trogon_vault_nats::{AuditPublisher, CryptoCtx, NatsKvVault, ensure_audit_stream, ensure_vault_bucket, grace_period_from_env};

// ── Vault builders (shared with worker binary) ────────────────────────────────

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
            .expect("VAULT_MASTER_PASSWORD and VAULT_KEY_SALT required for NATS KV vault"),
    );
    NatsKvVault::new(kv, crypto, grace_period_from_env())
        .await
        .expect("Failed to initialise NatsKvVault")
        .with_audit(audit)
}

// ── Generic runner ────────────────────────────────────────────────────────────

async fn run_with_vault<V, N>(
    vault:      Arc<V>,
    notifier:   Arc<N>,
    js:         async_nats::jetstream::Context,
    nats:       async_nats::Client,
    vault_name: &str,
) where
    V: VaultStore + 'static,
    V::Error: std::fmt::Display,
    N: Notifier,
{
    ensure_proposals_stream(&js)
        .await
        .expect("Failed to ensure VAULT_PROPOSALS stream");
    ensure_audit_stream(&js)
        .await
        .expect("Failed to ensure VAULT_AUDIT stream");

    let audit = Arc::new(AuditPublisher::new(js.clone(), vault_name));

    tracing::info!(vault = %vault_name, "Approval service starting");

    ApprovalService::new(vault, notifier, js, nats, vault_name)
        .with_audit(audit)
        .run()
        .await
        .expect("ApprovalService exited with error");
}

// ── Notifier selection ────────────────────────────────────────────────────────

async fn run_with_vault_and_notifier<V>(
    vault:      Arc<V>,
    js:         async_nats::jetstream::Context,
    nats:       async_nats::Client,
    vault_name: &str,
) where
    V: VaultStore + 'static,
    V::Error: std::fmt::Display,
{
    match std::env::var("VAULT_SLACK_WEBHOOK_URL") {
        Ok(url) => {
            tracing::info!("Notifier: Slack webhook");
            let notifier = Arc::new(SlackWebhookNotifier::new(url));
            run_with_vault(vault, notifier, js, nats, vault_name).await;
        }
        Err(_) => {
            tracing::info!("Notifier: structured logs (set VAULT_SLACK_WEBHOOK_URL for Slack)");
            let notifier = Arc::new(LoggingNotifier);
            run_with_vault(vault, notifier, js, nats, vault_name).await;
        }
    }
}

// ── main ──────────────────────────────────────────────────────────────────────

#[tokio::main]
async fn main() {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .init();

    let vault_name = std::env::var("APPROVAL_VAULT_NAME")
        .unwrap_or_else(|_| "default".to_string());

    let nats_config = NatsConfig::from_env(&SystemEnv);
    tracing::info!(servers = ?nats_config.servers, "Connecting to NATS");

    let nats = connect(&nats_config, Duration::from_secs(10))
        .await
        .expect("Failed to connect to NATS");

    let js = async_nats::jetstream::new(nats.clone());

    let has_infisical = std::env::var("INFISICAL_URL").is_ok();
    let has_nats_kv   = std::env::var("VAULT_NATS_BUCKET").is_ok();
    let has_hashicorp = std::env::var("VAULT_ADDR").is_ok();

    match (has_infisical, has_nats_kv, has_hashicorp) {
        (true, true, _) => {
            tracing::info!("Vault backend: DualWrite (Infisical primary + NATS KV cache)");
            let config    = InfisicalConfig::from_env().expect("Infisical env vars required");
            let infisical = InfisicalVaultStore::new(config);
            let bucket    = std::env::var("VAULT_NATS_BUCKET").unwrap();
            ensure_audit_stream(&js).await.expect("Failed to ensure VAULT_AUDIT stream");
            let audit   = Arc::new(AuditPublisher::new(js.clone(), &bucket));
            let nats_kv = build_nats_kv_vault(&js, &bucket, audit).await;
            let vault   = Arc::new(DualWriteVault::new(infisical, nats_kv));
            run_with_vault_and_notifier(vault, js, nats, &vault_name).await;
        }
        (true, false, _) => {
            tracing::info!("Vault backend: Infisical");
            let config = InfisicalConfig::from_env().expect("Infisical env vars required");
            let vault  = Arc::new(InfisicalVaultStore::new(config));
            run_with_vault_and_notifier(vault, js, nats, &vault_name).await;
        }
        (false, true, _) => {
            let bucket = std::env::var("VAULT_NATS_BUCKET").unwrap();
            tracing::info!(bucket = %bucket, "Vault backend: NATS KV");
            ensure_audit_stream(&js).await.expect("Failed to ensure VAULT_AUDIT stream");
            let audit = Arc::new(AuditPublisher::new(js.clone(), &bucket));
            let vault = Arc::new(build_nats_kv_vault(&js, &bucket, audit).await);
            run_with_vault_and_notifier(vault, js, nats, &vault_name).await;
        }
        (false, false, true) => {
            let vault_addr = std::env::var("VAULT_ADDR").unwrap();
            tracing::info!(addr = %vault_addr, "Vault backend: HashiCorp Vault");
            let vault = Arc::new(build_hashicorp_vault(vault_addr).await);
            run_with_vault_and_notifier(vault, js, nats, &vault_name).await;
        }
        _ => {
            tracing::warn!(
                "No vault backend configured. Running in-memory (development mode only). \
                 Set INFISICAL_URL, VAULT_NATS_BUCKET, or VAULT_ADDR for production."
            );
            let vault = Arc::new(MemoryVault::new());
            let mut seeded = 0usize;
            for (key, value) in std::env::vars() {
                let Some(token_str) = key.strip_prefix("VAULT_TOKEN_") else { continue };
                match ApiKeyToken::new(token_str) {
                    Ok(token) => {
                        vault.store(&token, &value).await.expect("Failed to seed vault token");
                        tracing::info!(token = %token, "Vault token seeded");
                        seeded += 1;
                    }
                    Err(e) => {
                        tracing::warn!(env_key = %key, error = %e, "Skipping invalid VAULT_TOKEN_* env var");
                    }
                }
            }
            tracing::info!(seeded, "Vault seeding complete");
            run_with_vault_and_notifier(vault, js, nats, &vault_name).await;
        }
    }
}
