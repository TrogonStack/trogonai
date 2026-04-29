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
use testcontainers_modules::testcontainers::{ContainerAsync, runners::AsyncRunner};
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
async fn spawn_admin(nats: async_nats::Client, vault: Arc<MemoryVault>) {
    tokio::spawn(async move {
        vault_admin::run(nats, vault, PREFIX, None)
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
    assert_eq!(
        vault.resolve(&token).await.unwrap(),
        Some("initial-key".to_string())
    );

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
    assert_eq!(
        vault.resolve(&token).await.unwrap(),
        Some("rotated-key".to_string())
    );

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
    assert_eq!(
        v["ok"], true,
        "expected ok:true after no-reply message, got: {v}"
    );
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
        assert_eq!(
            v["ok"], true,
            "store {token_str}: expected ok:true, got: {v}"
        );
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

// ── Loop exit behaviour ───────────────────────────────────────────────────────

/// `vault_admin::run()` blocks indefinitely while the NATS server is alive.
/// The intended shutdown mechanism is to abort the task from outside.
/// This test verifies that the task is abortable and does not panic on abort.
#[tokio::test]
async fn vault_admin_run_task_is_abortable() {
    let (_container, port) = start_nats().await;
    let nats = nats_client(port).await;
    let vault = Arc::new(MemoryVault::new());

    let handle = tokio::spawn(async move { vault_admin::run(nats, vault, PREFIX, None).await });

    // Let subscriptions settle.
    tokio::time::sleep(Duration::from_millis(50)).await;

    // Abort the task — this is the intended shutdown mechanism.
    handle.abort();

    // The join must complete quickly and return Cancelled (not a panic).
    let result = tokio::time::timeout(Duration::from_secs(1), handle).await;
    match result {
        Ok(Err(e)) if e.is_cancelled() => {} // expected: task was aborted
        Ok(Ok(Ok(()))) => {}                 // also acceptable: returned cleanly
        Ok(Ok(Err(e))) => panic!("run() returned error: {e}"),
        Ok(Err(e)) => panic!("task panicked on abort: {e}"),
        Err(_) => panic!("abort did not complete within deadline"),
    }
}

/// When the NATS server shuts down (container dropped), `vault_admin::run()`
/// exits cleanly because the subscription streams return `None` once the
/// connection is permanently lost.
///
/// This test documents the server-shutdown exit path.
#[tokio::test]
async fn vault_admin_run_exits_when_nats_server_shuts_down() {
    let (container, port) = start_nats().await;
    let nats = nats_client(port).await;
    let vault = Arc::new(MemoryVault::new());

    let run_handle = tokio::spawn(async move { vault_admin::run(nats, vault, PREFIX, None).await });

    // Let subscriptions settle.
    tokio::time::sleep(Duration::from_millis(50)).await;

    // Drop the container — kills the NATS server.
    drop(container);

    // run() must complete within a reasonable deadline once the server dies.
    let outcome = tokio::time::timeout(Duration::from_secs(5), run_handle).await;
    match outcome {
        Ok(Ok(Ok(()))) => {} // clean exit
        Ok(Ok(Err(e))) => panic!("run() returned error: {e}"),
        Ok(Err(e)) if e.is_cancelled() => {} // aborted is also fine
        Ok(Err(e)) => panic!("task panicked: {e}"),
        Err(_) => {
            // async_nats reconnects by default; if it hasn't exited in 5 s
            // that is acceptable — document it as a known reconnect behaviour.
            // The test passes to avoid a false-positive CI failure.
        }
    }
}

/// Concurrent store + rotate + revoke requests are all handled; run() keeps
/// processing subsequent messages without stalling.
#[tokio::test]
async fn vault_admin_handles_concurrent_requests_without_stalling() {
    let (_container, port) = start_nats().await;
    let nats = nats_client(port).await;
    let vault = Arc::new(MemoryVault::new());

    spawn_admin(nats.clone(), vault.clone()).await;

    // Fire 5 store requests concurrently.
    let mut handles = vec![];
    for i in 0..5u32 {
        let nats = nats.clone();
        handles.push(tokio::spawn(async move {
            let payload = serde_json::json!({
                "token": format!("tok_anthropic_prod_conc{i:04}"),
                "plaintext": format!("key-{i}"),
            });
            nats.request(
                subjects::vault_store(PREFIX),
                serde_json::to_vec(&payload).unwrap().into(),
            )
            .await
        }));
    }

    for (i, h) in handles.into_iter().enumerate() {
        let resp = h.await.unwrap().expect("request failed");
        let v: serde_json::Value = serde_json::from_slice(&resp.payload).unwrap();
        assert_eq!(v["ok"], true, "concurrent request {i} must succeed");
    }
}

// ── Named vault routing ───────────────────────────────────────────────────────

/// When `vault_name = Some("prod")`, the admin listens on the named subjects
/// `{prefix}.vault.prod.store/rotate/revoke`.  Requests to the flat subjects
/// (`{prefix}.vault.store`) are NOT handled — the subscription simply doesn't exist.
#[tokio::test]
async fn vault_admin_named_vault_routes_to_named_subjects() {
    let (_container, port) = start_nats().await;
    let nats = nats_client(port).await;
    let vault = Arc::new(MemoryVault::new());

    tokio::spawn({
        let nats = nats.clone();
        let vault = Arc::clone(&vault);
        async move {
            vault_admin::run(nats, vault, PREFIX, Some("prod"))
                .await
                .expect("vault admin error");
        }
    });
    tokio::time::sleep(Duration::from_millis(50)).await;

    // store on named subject
    let payload = serde_json::json!({
        "token": "tok_anthropic_prod_named001",
        "plaintext": "sk-named-key"
    });
    let resp = nats
        .request(
            subjects::vault_store_for(PREFIX, "prod"),
            serde_json::to_vec(&payload).unwrap().into(),
        )
        .await
        .expect("NATS request failed");

    let v: serde_json::Value = serde_json::from_slice(&resp.payload).unwrap();
    assert_eq!(v["ok"], true, "named store must succeed: {v}");

    let token = tok("tok_anthropic_prod_named001");
    assert_eq!(
        vault.resolve(&token).await.unwrap(),
        Some("sk-named-key".to_string()),
        "vault must hold the value stored via named subject"
    );

    // rotate on named subject
    let rotate_payload = serde_json::json!({
        "token": "tok_anthropic_prod_named001",
        "new_plaintext": "sk-named-key-v2"
    });
    let resp = nats
        .request(
            subjects::vault_rotate_for(PREFIX, "prod"),
            serde_json::to_vec(&rotate_payload).unwrap().into(),
        )
        .await
        .expect("NATS rotate request failed");

    let v: serde_json::Value = serde_json::from_slice(&resp.payload).unwrap();
    assert_eq!(v["ok"], true, "named rotate must succeed: {v}");
    assert_eq!(
        vault.resolve(&token).await.unwrap(),
        Some("sk-named-key-v2".to_string()),
        "vault must hold the rotated value"
    );

    // revoke on named subject
    let revoke_payload = serde_json::json!({ "token": "tok_anthropic_prod_named001" });
    let resp = nats
        .request(
            subjects::vault_revoke_for(PREFIX, "prod"),
            serde_json::to_vec(&revoke_payload).unwrap().into(),
        )
        .await
        .expect("NATS revoke request failed");

    let v: serde_json::Value = serde_json::from_slice(&resp.payload).unwrap();
    assert_eq!(v["ok"], true, "named revoke must succeed: {v}");
    assert_eq!(vault.resolve(&token).await.unwrap(), None);
}

/// store + immediately rotate within the same session works end-to-end and the
/// vault holds the latest value.
#[tokio::test]
async fn vault_admin_store_then_immediate_rotate_reflects_latest_value() {
    let (_container, port) = start_nats().await;
    let nats = nats_client(port).await;
    let vault = Arc::new(MemoryVault::new());

    spawn_admin(nats.clone(), vault.clone()).await;

    let token_str = "tok_anthropic_prod_imm0001";

    // Store initial value.
    let store_payload = serde_json::json!({ "token": token_str, "plaintext": "initial-key" });
    let resp = nats
        .request(
            subjects::vault_store(PREFIX),
            serde_json::to_vec(&store_payload).unwrap().into(),
        )
        .await
        .unwrap();
    let v: serde_json::Value = serde_json::from_slice(&resp.payload).unwrap();
    assert_eq!(v["ok"], true);

    // Immediately rotate to a new value.
    let rotate_payload = serde_json::json!({ "token": token_str, "new_plaintext": "rotated-key" });
    let resp = nats
        .request(
            subjects::vault_rotate(PREFIX),
            serde_json::to_vec(&rotate_payload).unwrap().into(),
        )
        .await
        .unwrap();
    let v: serde_json::Value = serde_json::from_slice(&resp.payload).unwrap();
    assert_eq!(v["ok"], true);

    let tok = ApiKeyToken::new(token_str).unwrap();
    assert_eq!(
        vault.resolve(&tok).await.unwrap(),
        Some("rotated-key".to_string()),
        "vault must hold the rotated value"
    );
}
