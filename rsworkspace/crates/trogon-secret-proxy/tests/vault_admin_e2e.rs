//! Integration tests for the vault admin NATS listener.
//!
//! Requires Docker (uses testcontainers to spin up a real NATS server).
//! Core NATS only — no JetStream needed.
//!
//! Run with:
//!   cargo test -p trogon-secret-proxy --test vault_admin_e2e

use std::sync::Arc;
use std::time::Duration;

use testcontainers_modules::nats::Nats;
use testcontainers_modules::testcontainers::{runners::AsyncRunner, ContainerAsync};
use trogon_secret_proxy::{subjects, vault_admin};
use trogon_vault::{ApiKeyToken, MemoryVault, VaultStore};

// ── Shared helpers ────────────────────────────────────────────────────────────

async fn start_nats() -> (ContainerAsync<Nats>, u16) {
    let container: ContainerAsync<Nats> = Nats::default()
        .start()
        .await
        .expect("Failed to start NATS container — is Docker running?");
    let port = container.get_host_port_ipv4(4222).await.unwrap();
    (container, port)
}

async fn nats_client(port: u16) -> async_nats::Client {
    async_nats::connect(format!("nats://127.0.0.1:{port}"))
        .await
        .expect("Failed to connect to NATS")
}

fn tok(s: &str) -> ApiKeyToken {
    ApiKeyToken::new(s).unwrap()
}

const PREFIX: &str = "test";

/// Spawn the vault admin listener as a background task.
/// Returns the vault so tests can inspect its state.
async fn spawn_admin(
    nats: async_nats::Client,
    vault: Arc<MemoryVault>,
) {
    tokio::spawn(async move {
        vault_admin::run(nats, vault, PREFIX)
            .await
            .expect("vault admin error");
    });
    // Small delay to let subscriptions settle before test sends requests.
    tokio::time::sleep(Duration::from_millis(50)).await;
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[tokio::test]
async fn vault_admin_store_persists_token() {
    let (_container, port) = start_nats().await;
    let nats = nats_client(port).await;
    let vault = Arc::new(MemoryVault::new());

    spawn_admin(nats.clone(), Arc::clone(&vault)).await;

    let payload = serde_json::json!({
        "token": "tok_anthropic_prod_abc123",
        "plaintext": "sk-ant-stored-key"
    });
    let resp = nats
        .request(
            subjects::vault_store(PREFIX),
            serde_json::to_vec(&payload).unwrap().into(),
        )
        .await
        .expect("NATS request failed");

    let v: serde_json::Value = serde_json::from_slice(&resp.payload).unwrap();
    assert_eq!(v["ok"], true, "expected ok:true, got: {v}");

    let token = tok("tok_anthropic_prod_abc123");
    let resolved = vault.resolve(&token).await.unwrap();
    assert_eq!(resolved, Some("sk-ant-stored-key".to_string()));
}

#[tokio::test]
async fn vault_admin_rotate_updates_plaintext() {
    let (_container, port) = start_nats().await;
    let nats = nats_client(port).await;
    let vault = Arc::new(MemoryVault::new());

    // Pre-seed old key
    let token = tok("tok_openai_prod_xyz789");
    vault.insert(&token, "sk-old-key");

    spawn_admin(nats.clone(), Arc::clone(&vault)).await;

    let payload = serde_json::json!({
        "token": "tok_openai_prod_xyz789",
        "new_plaintext": "sk-new-rotated-key"
    });
    let resp = nats
        .request(
            subjects::vault_rotate(PREFIX),
            serde_json::to_vec(&payload).unwrap().into(),
        )
        .await
        .expect("NATS request failed");

    let v: serde_json::Value = serde_json::from_slice(&resp.payload).unwrap();
    assert_eq!(v["ok"], true, "expected ok:true, got: {v}");

    let resolved = vault.resolve(&token).await.unwrap();
    assert_eq!(resolved, Some("sk-new-rotated-key".to_string()));
}

#[tokio::test]
async fn vault_admin_revoke_removes_token() {
    let (_container, port) = start_nats().await;
    let nats = nats_client(port).await;
    let vault = Arc::new(MemoryVault::new());

    // Pre-seed token
    let token = tok("tok_gemini_staging_aabbcc");
    vault.insert(&token, "gemini-key");

    spawn_admin(nats.clone(), Arc::clone(&vault)).await;

    let payload = serde_json::json!({ "token": "tok_gemini_staging_aabbcc" });
    let resp = nats
        .request(
            subjects::vault_revoke(PREFIX),
            serde_json::to_vec(&payload).unwrap().into(),
        )
        .await
        .expect("NATS request failed");

    let v: serde_json::Value = serde_json::from_slice(&resp.payload).unwrap();
    assert_eq!(v["ok"], true, "expected ok:true, got: {v}");

    let resolved = vault.resolve(&token).await.unwrap();
    assert_eq!(resolved, None);
}

#[tokio::test]
async fn vault_admin_invalid_token_returns_error_response() {
    let (_container, port) = start_nats().await;
    let nats = nats_client(port).await;
    let vault = Arc::new(MemoryVault::new());

    spawn_admin(nats.clone(), Arc::clone(&vault)).await;

    let payload = serde_json::json!({
        "token": "not-a-valid-token",
        "plaintext": "some-key"
    });
    let resp = nats
        .request(
            subjects::vault_store(PREFIX),
            serde_json::to_vec(&payload).unwrap().into(),
        )
        .await
        .expect("NATS request failed");

    let v: serde_json::Value = serde_json::from_slice(&resp.payload).unwrap();
    assert_eq!(v["ok"], false, "expected ok:false, got: {v}");
    let error = v["error"].as_str().unwrap_or("");
    assert!(
        error.contains("Invalid token"),
        "expected 'Invalid token' in error, got: {error}"
    );
}

#[tokio::test]
async fn vault_admin_malformed_json_returns_error_response() {
    let (_container, port) = start_nats().await;
    let nats = nats_client(port).await;
    let vault = Arc::new(MemoryVault::new());

    spawn_admin(nats.clone(), Arc::clone(&vault)).await;

    let resp = nats
        .request(
            subjects::vault_store(PREFIX),
            bytes::Bytes::from_static(b"this is not json"),
        )
        .await
        .expect("NATS request failed");

    let v: serde_json::Value = serde_json::from_slice(&resp.payload).unwrap();
    assert_eq!(v["ok"], false, "expected ok:false, got: {v}");
    let error = v["error"].as_str().unwrap_or("");
    assert!(
        error.contains("Invalid JSON"),
        "expected 'Invalid JSON' in error, got: {error}"
    );
}

// ── Gap: missing required fields ──────────────────────────────────────────────

/// `vault_store` requires both `token` and `plaintext`.  Sending only `token`
/// triggers a serde deserialization error → `{ok: false, error: "Invalid JSON: ..."}`.
#[tokio::test]
async fn vault_admin_store_missing_plaintext_field_returns_error() {
    let (_container, port) = start_nats().await;
    let nats = nats_client(port).await;
    let vault = Arc::new(MemoryVault::new());

    spawn_admin(nats.clone(), Arc::clone(&vault)).await;

    let payload = serde_json::json!({ "token": "tok_anthropic_prod_abc123" });
    let resp = nats
        .request(
            subjects::vault_store(PREFIX),
            serde_json::to_vec(&payload).unwrap().into(),
        )
        .await
        .expect("NATS request failed");

    let v: serde_json::Value = serde_json::from_slice(&resp.payload).unwrap();
    assert_eq!(v["ok"], false, "expected ok:false, got: {v}");
    let error = v["error"].as_str().unwrap_or("");
    assert!(
        error.contains("Invalid JSON"),
        "expected 'Invalid JSON' in error, got: {error}"
    );
}

/// `vault_rotate` requires both `token` and `new_plaintext`.  Sending only
/// `token` triggers a serde error → `{ok: false, error: "Invalid JSON: ..."}`.
#[tokio::test]
async fn vault_admin_rotate_missing_new_plaintext_field_returns_error() {
    let (_container, port) = start_nats().await;
    let nats = nats_client(port).await;
    let vault = Arc::new(MemoryVault::new());

    spawn_admin(nats.clone(), Arc::clone(&vault)).await;

    let payload = serde_json::json!({ "token": "tok_openai_prod_xyz789" });
    let resp = nats
        .request(
            subjects::vault_rotate(PREFIX),
            serde_json::to_vec(&payload).unwrap().into(),
        )
        .await
        .expect("NATS request failed");

    let v: serde_json::Value = serde_json::from_slice(&resp.payload).unwrap();
    assert_eq!(v["ok"], false, "expected ok:false, got: {v}");
    let error = v["error"].as_str().unwrap_or("");
    assert!(
        error.contains("Invalid JSON"),
        "expected 'Invalid JSON' in error, got: {error}"
    );
}

/// `vault_revoke` requires `token`.  Sending an empty JSON object triggers a
/// serde error → `{ok: false, error: "Invalid JSON: ..."}`.
#[tokio::test]
async fn vault_admin_revoke_missing_token_field_returns_error() {
    let (_container, port) = start_nats().await;
    let nats = nats_client(port).await;
    let vault = Arc::new(MemoryVault::new());

    spawn_admin(nats.clone(), Arc::clone(&vault)).await;

    let resp = nats
        .request(
            subjects::vault_revoke(PREFIX),
            serde_json::to_vec(&serde_json::json!({})).unwrap().into(),
        )
        .await
        .expect("NATS request failed");

    let v: serde_json::Value = serde_json::from_slice(&resp.payload).unwrap();
    assert_eq!(v["ok"], false, "expected ok:false, got: {v}");
    let error = v["error"].as_str().unwrap_or("");
    assert!(
        error.contains("Invalid JSON"),
        "expected 'Invalid JSON' in error, got: {error}"
    );
}

// ── Gap: operations on non-existent tokens ────────────────────────────────────

/// Revoking a token that was never stored is a no-op in `MemoryVault` and
/// must return `{ok: true}` — idempotent delete semantics.
#[tokio::test]
async fn vault_admin_revoke_nonexistent_token_returns_ok() {
    let (_container, port) = start_nats().await;
    let nats = nats_client(port).await;
    let vault = Arc::new(MemoryVault::new());

    spawn_admin(nats.clone(), Arc::clone(&vault)).await;

    let payload = serde_json::json!({ "token": "tok_anthropic_prod_nostored1" });
    let resp = nats
        .request(
            subjects::vault_revoke(PREFIX),
            serde_json::to_vec(&payload).unwrap().into(),
        )
        .await
        .expect("NATS request failed");

    let v: serde_json::Value = serde_json::from_slice(&resp.payload).unwrap();
    assert_eq!(v["ok"], true, "expected ok:true, got: {v}");
}

/// Rotating a token that was never stored falls back to the default `rotate`
/// implementation which delegates to `store` — the mapping is created and
/// `{ok: true}` is returned.
#[tokio::test]
async fn vault_admin_rotate_nonexistent_token_creates_it() {
    let (_container, port) = start_nats().await;
    let nats = nats_client(port).await;
    let vault = Arc::new(MemoryVault::new());

    spawn_admin(nats.clone(), Arc::clone(&vault)).await;

    let token = tok("tok_openai_staging_newtoken1");

    let payload = serde_json::json!({
        "token": "tok_openai_staging_newtoken1",
        "new_plaintext": "sk-new-created-key"
    });
    let resp = nats
        .request(
            subjects::vault_rotate(PREFIX),
            serde_json::to_vec(&payload).unwrap().into(),
        )
        .await
        .expect("NATS request failed");

    let v: serde_json::Value = serde_json::from_slice(&resp.payload).unwrap();
    assert_eq!(v["ok"], true, "expected ok:true, got: {v}");

    // Verify the token was actually created.
    let resolved = vault.resolve(&token).await.unwrap();
    assert_eq!(resolved, Some("sk-new-created-key".to_string()));
}

// ── Gap: full lifecycle ───────────────────────────────────────────────────────

/// Exercise the complete store → rotate → revoke lifecycle in a single test to
/// verify that each operation observes the effect of the previous one.
#[tokio::test]
async fn vault_admin_store_rotate_revoke_lifecycle() {
    let (_container, port) = start_nats().await;
    let nats = nats_client(port).await;
    let vault = Arc::new(MemoryVault::new());

    spawn_admin(nats.clone(), Arc::clone(&vault)).await;

    let token = tok("tok_gemini_prod_lifecycle1");

    // 1. Store initial key.
    let store_payload = serde_json::json!({
        "token": "tok_gemini_prod_lifecycle1",
        "plaintext": "initial-key"
    });
    let resp = nats
        .request(
            subjects::vault_store(PREFIX),
            serde_json::to_vec(&store_payload).unwrap().into(),
        )
        .await
        .unwrap();
    let v: serde_json::Value = serde_json::from_slice(&resp.payload).unwrap();
    assert_eq!(v["ok"], true, "store: expected ok:true, got: {v}");
    assert_eq!(vault.resolve(&token).await.unwrap(), Some("initial-key".to_string()));

    // 2. Rotate to a new key.
    let rotate_payload = serde_json::json!({
        "token": "tok_gemini_prod_lifecycle1",
        "new_plaintext": "rotated-key"
    });
    let resp = nats
        .request(
            subjects::vault_rotate(PREFIX),
            serde_json::to_vec(&rotate_payload).unwrap().into(),
        )
        .await
        .unwrap();
    let v: serde_json::Value = serde_json::from_slice(&resp.payload).unwrap();
    assert_eq!(v["ok"], true, "rotate: expected ok:true, got: {v}");
    assert_eq!(vault.resolve(&token).await.unwrap(), Some("rotated-key".to_string()));

    // 3. Revoke the token.
    let revoke_payload = serde_json::json!({ "token": "tok_gemini_prod_lifecycle1" });
    let resp = nats
        .request(
            subjects::vault_revoke(PREFIX),
            serde_json::to_vec(&revoke_payload).unwrap().into(),
        )
        .await
        .unwrap();
    let v: serde_json::Value = serde_json::from_slice(&resp.payload).unwrap();
    assert_eq!(v["ok"], true, "revoke: expected ok:true, got: {v}");
    assert_eq!(vault.resolve(&token).await.unwrap(), None);
}

// ── Gap: message without reply subject ───────────────────────────────────────

/// Publishing to the store subject without a reply-to header must not crash the
/// listener.  The handler logs a warning and moves on, so a subsequent
/// request-reply still succeeds.
#[tokio::test]
async fn vault_admin_message_without_reply_subject_is_ignored_and_listener_continues() {
    let (_container, port) = start_nats().await;
    let nats = nats_client(port).await;
    let vault = Arc::new(MemoryVault::new());

    spawn_admin(nats.clone(), Arc::clone(&vault)).await;

    // Publish with no reply subject (fire-and-forget).
    let payload = serde_json::json!({
        "token": "tok_anthropic_prod_abc123",
        "plaintext": "should-be-ignored"
    });
    nats.publish(
        subjects::vault_store(PREFIX),
        serde_json::to_vec(&payload).unwrap().into(),
    )
    .await
    .expect("NATS publish failed");

    // Give the listener time to process the fire-and-forget message.
    tokio::time::sleep(Duration::from_millis(100)).await;

    // Now send a normal request to confirm the listener is still running.
    let ok_payload = serde_json::json!({
        "token": "tok_openai_prod_xyz789",
        "plaintext": "still-working-key"
    });
    let resp = nats
        .request(
            subjects::vault_store(PREFIX),
            serde_json::to_vec(&ok_payload).unwrap().into(),
        )
        .await
        .expect("NATS request failed — listener may have crashed");

    let v: serde_json::Value = serde_json::from_slice(&resp.payload).unwrap();
    assert_eq!(v["ok"], true, "expected ok:true after no-reply message, got: {v}");
}

// ── Gap: multiple tokens managed independently ────────────────────────────────

/// Storing two different tokens and then rotating one while revoking the other
/// must leave each token in the correct state — operations are isolated.
#[tokio::test]
async fn vault_admin_multiple_tokens_managed_independently() {
    let (_container, port) = start_nats().await;
    let nats = nats_client(port).await;
    let vault = Arc::new(MemoryVault::new());

    spawn_admin(nats.clone(), Arc::clone(&vault)).await;

    let tok_a = tok("tok_anthropic_prod_multi1a");
    let tok_b = tok("tok_openai_prod_multi1b");

    // Store both tokens.
    for (token_str, key) in [
        ("tok_anthropic_prod_multi1a", "key-a-v1"),
        ("tok_openai_prod_multi1b", "key-b-v1"),
    ] {
        let payload = serde_json::json!({ "token": token_str, "plaintext": key });
        let resp = nats
            .request(
                subjects::vault_store(PREFIX),
                serde_json::to_vec(&payload).unwrap().into(),
            )
            .await
            .unwrap();
        let v: serde_json::Value = serde_json::from_slice(&resp.payload).unwrap();
        assert_eq!(v["ok"], true, "store {token_str}: expected ok:true, got: {v}");
    }

    // Rotate token A.
    let rotate_payload = serde_json::json!({
        "token": "tok_anthropic_prod_multi1a",
        "new_plaintext": "key-a-v2"
    });
    let resp = nats
        .request(
            subjects::vault_rotate(PREFIX),
            serde_json::to_vec(&rotate_payload).unwrap().into(),
        )
        .await
        .unwrap();
    let v: serde_json::Value = serde_json::from_slice(&resp.payload).unwrap();
    assert_eq!(v["ok"], true, "rotate tok_a: expected ok:true, got: {v}");

    // Revoke token B.
    let revoke_payload = serde_json::json!({ "token": "tok_openai_prod_multi1b" });
    let resp = nats
        .request(
            subjects::vault_revoke(PREFIX),
            serde_json::to_vec(&revoke_payload).unwrap().into(),
        )
        .await
        .unwrap();
    let v: serde_json::Value = serde_json::from_slice(&resp.payload).unwrap();
    assert_eq!(v["ok"], true, "revoke tok_b: expected ok:true, got: {v}");

    // Token A should have the rotated value, token B should be gone.
    assert_eq!(
        vault.resolve(&tok_a).await.unwrap(),
        Some("key-a-v2".to_string()),
        "tok_a must have rotated value"
    );
    assert_eq!(
        vault.resolve(&tok_b).await.unwrap(),
        None,
        "tok_b must be revoked"
    );
}
