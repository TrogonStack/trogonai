//! Integration tests for NatsKvVault — require a live NATS server.
//!
//! Run with:
//! ```sh
//! cargo test -p trogon-vault-nats
//! ```
//! testcontainers spins up NATS automatically.

use std::sync::Arc;
use std::time::Duration;

use async_nats::jetstream;
use futures_util::StreamExt as _;
use testcontainers_modules::{
    nats::Nats,
    testcontainers::{ImageExt, runners::AsyncRunner},
};
use trogon_vault::{ApiKeyToken, VaultStore};
use trogon_vault_nats::{
    AuditPublisher, CryptoCtx, NatsKvVault, ensure_audit_stream, ensure_vault_bucket,
};

// ── Helpers ───────────────────────────────────────────────────────────────────

fn tok(s: &str) -> ApiKeyToken {
    ApiKeyToken::new(s).unwrap()
}

fn crypto() -> Arc<CryptoCtx> {
    Arc::new(CryptoCtx::derive(b"test-password", b"test-salt-16byte").unwrap())
}

async fn make_vault(js: &jetstream::Context, name: &str) -> NatsKvVault {
    let kv = ensure_vault_bucket(js, name, 1).await.expect("bucket");
    NatsKvVault::new(kv, crypto(), Duration::from_secs(30))
        .await
        .expect("vault")
}

async fn start_nats() -> (jetstream::Context, impl Drop) {
    let container = Nats::default()
        .with_cmd(["--jetstream"])
        .start()
        .await
        .expect("NATS container");
    let port = container.get_host_port_ipv4(4222).await.expect("port");
    let nats = async_nats::connect(format!("nats://127.0.0.1:{port}"))
        .await
        .expect("NATS connect");
    let js = jetstream::new(nats);
    (js, container)
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[tokio::test]
async fn store_and_resolve_roundtrip() {
    let (js, _c) = start_nats().await;
    let vault = make_vault(&js, "default").await;
    let token = tok("tok_anthropic_prod_abc123");

    vault.store(&token, "sk-ant-realkey").await.unwrap();

    // Allow watcher to propagate the write to cache
    tokio::time::sleep(Duration::from_millis(100)).await;

    let resolved = vault.resolve(&token).await.unwrap();
    assert_eq!(resolved, Some("sk-ant-realkey".to_string()));
}

#[tokio::test]
async fn resolve_unknown_token_returns_none() {
    let (js, _c) = start_nats().await;
    let vault = make_vault(&js, "default").await;
    let token = tok("tok_openai_staging_xyz789");

    let result = vault.resolve(&token).await.unwrap();
    assert_eq!(result, None);
}

#[tokio::test]
async fn revoke_makes_token_unresolvable() {
    let (js, _c) = start_nats().await;
    let vault = make_vault(&js, "default").await;
    let token = tok("tok_anthropic_prod_abc123");

    vault.store(&token, "sk-ant-key").await.unwrap();
    tokio::time::sleep(Duration::from_millis(100)).await;

    vault.revoke(&token).await.unwrap();
    tokio::time::sleep(Duration::from_millis(100)).await;

    assert_eq!(vault.resolve(&token).await.unwrap(), None);
}

#[tokio::test]
async fn revoke_nonexistent_token_is_ok() {
    let (js, _c) = start_nats().await;
    let vault = make_vault(&js, "default").await;
    let token = tok("tok_anthropic_prod_never11");

    let result = vault.revoke(&token).await;
    assert!(result.is_ok(), "revoke of unknown token must be idempotent: {result:?}");
}

#[tokio::test]
async fn rotate_updates_current_and_preserves_previous_in_slot() {
    let (js, _c) = start_nats().await;
    let vault = make_vault(&js, "default").await;
    let token = tok("tok_anthropic_prod_abc123");

    vault.store(&token, "sk-ant-v1").await.unwrap();
    tokio::time::sleep(Duration::from_millis(100)).await;

    vault.rotate(&token, "sk-ant-v2").await.unwrap();
    tokio::time::sleep(Duration::from_millis(100)).await;

    // resolve() returns the current key
    assert_eq!(
        vault.resolve(&token).await.unwrap(),
        Some("sk-ant-v2".to_string())
    );

    // slot() exposes both current and previous for fallback-on-401
    let (current, previous) = vault.slot(&token).expect("slot must exist");
    assert_eq!(current, "sk-ant-v2");
    assert_eq!(previous, Some("sk-ant-v1".to_string()));
}

#[tokio::test]
async fn named_vaults_are_isolated() {
    let (js, _c) = start_nats().await;
    let prod    = make_vault(&js, "prod").await;
    let staging = make_vault(&js, "staging").await;
    let token   = tok("tok_anthropic_prod_abc123");

    prod.store(&token, "sk-prod-key").await.unwrap();
    tokio::time::sleep(Duration::from_millis(100)).await;

    // The same token in staging must be unknown
    assert_eq!(staging.resolve(&token).await.unwrap(), None);
    // The prod vault sees its own key
    assert_eq!(
        prod.resolve(&token).await.unwrap(),
        Some("sk-prod-key".to_string())
    );
}

#[tokio::test]
async fn watcher_propagates_write_from_second_client() {
    let (js, _c) = start_nats().await;

    let vault_a = make_vault(&js, "default").await;
    let vault_b = make_vault(&js, "default").await;

    let token = tok("tok_openai_prod_aaa111");

    // vault_a writes
    vault_a.store(&token, "sk-openai-key").await.unwrap();

    // vault_b's watcher must pick it up
    tokio::time::sleep(Duration::from_millis(200)).await;
    assert_eq!(
        vault_b.resolve(&token).await.unwrap(),
        Some("sk-openai-key".to_string()),
        "vault_b must see vault_a's write via watcher"
    );
}

#[tokio::test]
async fn data_survives_vault_restart() {
    let (js, _c) = start_nats().await;
    let token = tok("tok_anthropic_staging_abc123");

    {
        let vault = make_vault(&js, "default").await;
        vault.store(&token, "sk-ant-persistent").await.unwrap();
        tokio::time::sleep(Duration::from_millis(100)).await;
        // vault dropped here
    }

    // New vault instance loads existing data from KV on startup
    let vault2 = make_vault(&js, "default").await;
    assert_eq!(
        vault2.resolve(&token).await.unwrap(),
        Some("sk-ant-persistent".to_string()),
        "data must survive vault restart (loaded from NATS KV)"
    );
}

#[tokio::test]
async fn resolve_with_previous_returns_previous_during_grace_period() {
    let (js, _c) = start_nats().await;
    let vault = make_vault(&js, "default").await;
    let token = tok("tok_anthropic_prod_abc123");

    vault.store(&token, "sk-ant-v1").await.unwrap();
    tokio::time::sleep(Duration::from_millis(100)).await;

    vault.rotate(&token, "sk-ant-v2").await.unwrap();
    tokio::time::sleep(Duration::from_millis(100)).await;

    let (current, previous) = vault.resolve_with_previous(&token).await.unwrap();
    assert_eq!(current, Some("sk-ant-v2".to_string()));
    assert_eq!(previous, Some("sk-ant-v1".to_string()));
}

#[tokio::test]
async fn store_overwrites_existing_value() {
    let (js, _c) = start_nats().await;
    let vault = make_vault(&js, "default").await;
    let token = tok("tok_anthropic_prod_abc123");

    vault.store(&token, "sk-ant-v1").await.unwrap();
    tokio::time::sleep(Duration::from_millis(100)).await;

    vault.store(&token, "sk-ant-v2").await.unwrap();
    tokio::time::sleep(Duration::from_millis(100)).await;

    assert_eq!(
        vault.resolve(&token).await.unwrap(),
        Some("sk-ant-v2".to_string())
    );
}

#[tokio::test]
async fn audit_events_published_for_store_resolve_revoke() {
    let (js, _c) = start_nats().await;

    // Subscribe on core NATS before creating the stream so we don't miss any messages.
    // JetStream publish also delivers to matching core NATS subscribers.
    let mut sub = js
        .client()
        .subscribe("vault.audit.>")
        .await
        .expect("subscribe");

    ensure_audit_stream(&js).await.expect("audit stream");

    let kv = ensure_vault_bucket(&js, "prod", 1).await.expect("bucket");
    let publisher = Arc::new(AuditPublisher::new(js.clone(), "prod"));
    let vault = NatsKvVault::new(kv, crypto(), Duration::from_secs(30))
        .await
        .expect("vault")
        .with_audit(publisher);

    let token = tok("tok_anthropic_prod_abc123");

    // store → audit.store.prod
    vault.store(&token, "sk-ant-key").await.unwrap();
    let msg = tokio::time::timeout(Duration::from_secs(2), sub.next())
        .await
        .expect("timeout waiting for store audit event")
        .expect("stream ended");
    assert_eq!(msg.subject.as_str(), "vault.audit.store.prod");
    let body: serde_json::Value = serde_json::from_slice(&msg.payload).unwrap();
    assert_eq!(body["type"], "store");
    assert_eq!(body["token"], "tok_anthropic_prod_abc123");
    assert_eq!(body["vault"], "prod");
    assert_eq!(body["actor"], "system");

    // resolve → audit.resolve.prod
    tokio::time::sleep(Duration::from_millis(100)).await;
    vault.resolve(&token).await.unwrap();
    let msg = tokio::time::timeout(Duration::from_secs(2), sub.next())
        .await
        .expect("timeout waiting for resolve audit event")
        .expect("stream ended");
    assert_eq!(msg.subject.as_str(), "vault.audit.resolve.prod");
    let body: serde_json::Value = serde_json::from_slice(&msg.payload).unwrap();
    assert_eq!(body["type"], "resolve");
    assert_eq!(body["success"], true);

    // revoke → audit.revoke.prod
    vault.revoke(&token).await.unwrap();
    let msg = tokio::time::timeout(Duration::from_secs(2), sub.next())
        .await
        .expect("timeout waiting for revoke audit event")
        .expect("stream ended");
    assert_eq!(msg.subject.as_str(), "vault.audit.revoke.prod");
    let body: serde_json::Value = serde_json::from_slice(&msg.payload).unwrap();
    assert_eq!(body["type"], "revoke");
}
