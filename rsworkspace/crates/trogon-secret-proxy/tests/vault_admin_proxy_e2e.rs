//! Integration test: vault_admin → proxy pipeline.
//!
//! Verifies that a token registered at runtime through the vault admin NATS
//! listener is immediately usable by the proxy+worker pipeline.
//!
//! Flow:
//!   1. Start NATS + JetStream, proxy, worker — vault starts EMPTY.
//!   2. Register a new token via NATS request to `{prefix}.vault.store`.
//!   3. Send an HTTP request through the proxy using that token.
//!   4. Assert the mock AI provider received the REAL key (not the token).
//!
//! This closes the gap where vault_admin and the proxy are tested in isolation
//! but never together in a single pipeline.
//!
//! Requires Docker. Run with:
//!   cargo test -p trogon-secret-proxy --test vault_admin_proxy_e2e

use std::sync::Arc;
use std::time::Duration;

use testcontainers_modules::nats::Nats;
use testcontainers_modules::testcontainers::{runners::AsyncRunner, ContainerAsync, ImageExt};
use trogon_nats::{NatsAuth, NatsConfig, connect};
use trogon_secret_proxy::{
    proxy::{ProxyState, router},
    stream, subjects, vault_admin, worker,
};
use trogon_vault::MemoryVault;

// ── helpers ────────────────────────────────────────────────────────────────────

async fn start_nats() -> (ContainerAsync<Nats>, u16) {
    let container = Nats::default()
        .with_cmd(["--jetstream"])
        .start()
        .await
        .expect("Failed to start NATS container — is Docker running?");
    let port = container.get_host_port_ipv4(4222).await.unwrap();
    (container, port)
}

// ── test ───────────────────────────────────────────────────────────────────────

/// Register a token via vault_admin NATS, then use that token in a proxy
/// request.  The mock AI provider must receive the REAL api key — proving the
/// runtime-registered token is picked up by the worker.
#[tokio::test]
async fn vault_admin_store_then_proxy_request_succeeds() {
    // ── 1. Start NATS ──────────────────────────────────────────────────────
    let (_nats_container, nats_port) = start_nats().await;

    // ── 2. Mock AI provider (expects REAL key, not the token) ──────────────
    let mock_server = httpmock::MockServer::start_async().await;
    let ai_mock = mock_server
        .mock_async(|when, then| {
            when.method(httpmock::Method::POST)
                .path("/v1/messages")
                .header("authorization", "Bearer sk-runtime-registered-key");
            then.status(200)
                .header("content-type", "application/json")
                .body(r#"{"id":"msg_01","type":"message","content":[]}"#);
        })
        .await;

    // ── 3. Shared EMPTY vault ──────────────────────────────────────────────
    let vault = Arc::new(MemoryVault::new());

    // ── 4. Connect to NATS ────────────────────────────────────────────────
    let nats_config = NatsConfig {
        servers: vec![format!("localhost:{nats_port}")],
        auth: NatsAuth::None,
    };
    let nats = connect(&nats_config, Duration::from_secs(10))
        .await
        .expect("Failed to connect to NATS");
    let jetstream = Arc::new(async_nats::jetstream::new(nats.clone()));

    // ── 5. Ensure PROXY_REQUESTS stream ────────────────────────────────────
    let outbound_subject = subjects::outbound("trogon");
    stream::ensure_stream(&jetstream, "trogon", &outbound_subject)
        .await
        .expect("Failed to ensure PROXY_REQUESTS stream");

    // ── 6. Start proxy (empty vault — no tokens yet) ───────────────────────
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .unwrap();
    let proxy_port = listener.local_addr().unwrap().port();

    let proxy_state = ProxyState {
        nats: nats.clone(),
        jetstream: Arc::clone(&jetstream),
        prefix: "trogon".to_string(),
        outbound_subject: outbound_subject.clone(),
        worker_timeout: Duration::from_secs(15),
        base_url_override: Some(mock_server.base_url()),
    };
    tokio::spawn(async move {
        axum::serve(listener, router(proxy_state)).await.ok();
    });

    // ── 7. Start worker (shares the same vault Arc) ────────────────────────
    let http_client = reqwest::Client::builder()
        .timeout(Duration::from_secs(15))
        .build()
        .unwrap();
    let worker_js = Arc::clone(&jetstream);
    let worker_nats = nats.clone();
    let worker_vault = Arc::clone(&vault);
    let worker_stream = stream::stream_name("trogon");
    tokio::spawn(async move {
        worker::run(
            worker_js,
            worker_nats,
            worker_vault,
            http_client,
            "vault-admin-proxy-worker",
            &worker_stream,
        )
        .await
        .expect("Worker error");
    });

    // ── 8. Start vault admin listener (shares the same vault Arc) ──────────
    let admin_nats = nats.clone();
    let admin_vault = Arc::clone(&vault);
    tokio::spawn(async move {
        vault_admin::run(admin_nats, admin_vault, "trogon")
            .await
            .expect("Vault admin error");
    });

    // Give everything a moment to initialize.
    tokio::time::sleep(Duration::from_millis(300)).await;

    // ── 9. Register the token at runtime via vault_admin ───────────────────
    let store_payload = serde_json::json!({
        "token": "tok_anthropic_prod_runtimeabc",
        "plaintext": "sk-runtime-registered-key"
    });
    let store_resp = nats
        .request(
            subjects::vault_store("trogon"),
            serde_json::to_vec(&store_payload).unwrap().into(),
        )
        .await
        .expect("vault.store NATS request failed");

    let store_result: serde_json::Value =
        serde_json::from_slice(&store_resp.payload).unwrap();
    assert_eq!(
        store_result["ok"], true,
        "vault.store must return ok:true; got: {store_result}"
    );

    // ── 10. Send request through proxy using the newly registered token ────
    let client = reqwest::Client::new();
    let resp = client
        .post(format!("http://127.0.0.1:{proxy_port}/anthropic/v1/messages"))
        .header("Authorization", "Bearer tok_anthropic_prod_runtimeabc")
        .header("Content-Type", "application/json")
        .body(r#"{"model":"claude-opus-4-6","max_tokens":10,"messages":[]}"#)
        .timeout(Duration::from_secs(20))
        .send()
        .await
        .expect("Request to proxy failed");

    // ── 11. Assertions ─────────────────────────────────────────────────────
    assert_eq!(
        resp.status(),
        200,
        "Proxy must return 200 after runtime token registration; got {}",
        resp.status()
    );

    // The critical check: the mock only matches requests with the REAL key.
    // If the worker forwarded the opaque token, the mock would not match.
    ai_mock.assert_async().await;
}

/// Registering a token, then revoking it, then using it returns an error.
#[tokio::test]
async fn vault_admin_revoke_then_proxy_request_fails() {
    let (_nats_container, nats_port) = start_nats().await;

    // Mock that should NOT be called after revocation.
    let mock_server = httpmock::MockServer::start_async().await;
    let _ai_mock = mock_server
        .mock_async(|when, then| {
            when.any_request();
            then.status(200).body("should not be reached");
        })
        .await;

    let vault = Arc::new(MemoryVault::new());

    let nats_config = NatsConfig {
        servers: vec![format!("localhost:{nats_port}")],
        auth: NatsAuth::None,
    };
    let nats = connect(&nats_config, Duration::from_secs(10)).await.unwrap();
    let jetstream = Arc::new(async_nats::jetstream::new(nats.clone()));

    let outbound_subject = subjects::outbound("trogon");
    stream::ensure_stream(&jetstream, "trogon", &outbound_subject)
        .await
        .unwrap();

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let proxy_port = listener.local_addr().unwrap().port();

    let proxy_state = ProxyState {
        nats: nats.clone(),
        jetstream: Arc::clone(&jetstream),
        prefix: "trogon".to_string(),
        outbound_subject: outbound_subject.clone(),
        worker_timeout: Duration::from_secs(5),
        base_url_override: Some(mock_server.base_url()),
    };
    tokio::spawn(async move {
        axum::serve(listener, router(proxy_state)).await.ok();
    });

    let http_client = reqwest::Client::builder()
        .timeout(Duration::from_secs(10))
        .build()
        .unwrap();
    let worker_js = Arc::clone(&jetstream);
    let worker_nats = nats.clone();
    let worker_vault = Arc::clone(&vault);
    let worker_stream = stream::stream_name("trogon");
    tokio::spawn(async move {
        worker::run(worker_js, worker_nats, worker_vault, http_client, "vault-revoke-worker", &worker_stream)
            .await
            .ok();
    });

    let admin_nats = nats.clone();
    let admin_vault = Arc::clone(&vault);
    tokio::spawn(async move {
        vault_admin::run(admin_nats, admin_vault, "trogon")
            .await
            .ok();
    });

    tokio::time::sleep(Duration::from_millis(300)).await;

    // Store the token.
    let store_payload = serde_json::json!({
        "token": "tok_anthropic_prod_revokeme01",
        "plaintext": "sk-revokeable-key"
    });
    let store_resp = nats
        .request(
            subjects::vault_store("trogon"),
            serde_json::to_vec(&store_payload).unwrap().into(),
        )
        .await
        .unwrap();
    let v: serde_json::Value = serde_json::from_slice(&store_resp.payload).unwrap();
    assert_eq!(v["ok"], true);

    // Revoke the token.
    let revoke_payload = serde_json::json!({ "token": "tok_anthropic_prod_revokeme01" });
    let revoke_resp = nats
        .request(
            subjects::vault_revoke("trogon"),
            serde_json::to_vec(&revoke_payload).unwrap().into(),
        )
        .await
        .unwrap();
    let v: serde_json::Value = serde_json::from_slice(&revoke_resp.payload).unwrap();
    assert_eq!(v["ok"], true);

    // Now try to use the revoked token through the proxy — must fail.
    let client = reqwest::Client::new();
    let resp = client
        .post(format!("http://127.0.0.1:{proxy_port}/anthropic/v1/messages"))
        .header("Authorization", "Bearer tok_anthropic_prod_revokeme01")
        .header("Content-Type", "application/json")
        .body(r#"{}"#)
        .timeout(Duration::from_secs(10))
        .send()
        .await
        .expect("Request to proxy failed");

    assert!(
        resp.status().is_client_error() || resp.status().is_server_error(),
        "Proxy must return an error after token is revoked; got {}",
        resp.status()
    );
}

/// After rotating a token via vault_admin, the proxy must use the NEW plaintext
/// for subsequent requests, not the old one.
///
/// Flow:
///   1. Store `tok → old-key`.
///   2. Proxy request → mock AI receives `Bearer sk-old-key`.
///   3. Rotate `tok` to `new-key` via vault_admin.
///   4. Proxy request → mock AI receives `Bearer sk-new-key`.
#[tokio::test]
async fn vault_admin_rotate_then_proxy_uses_new_key() {
    let (_nats_container, nats_port) = start_nats().await;

    let mock_server = httpmock::MockServer::start_async().await;

    // Two distinct mocks — old key first, new key after rotation.
    let old_key_mock = mock_server
        .mock_async(|when, then| {
            when.method(httpmock::Method::POST)
                .path("/v1/messages")
                .header("authorization", "Bearer sk-rotate-old-key");
            then.status(200)
                .header("content-type", "application/json")
                .body(r#"{"id":"msg_old","type":"message","content":[]}"#);
        })
        .await;

    let new_key_mock = mock_server
        .mock_async(|when, then| {
            when.method(httpmock::Method::POST)
                .path("/v1/messages")
                .header("authorization", "Bearer sk-rotate-new-key");
            then.status(200)
                .header("content-type", "application/json")
                .body(r#"{"id":"msg_new","type":"message","content":[]}"#);
        })
        .await;

    let vault = Arc::new(MemoryVault::new());

    let nats_config = NatsConfig {
        servers: vec![format!("localhost:{nats_port}")],
        auth: NatsAuth::None,
    };
    let nats = connect(&nats_config, Duration::from_secs(10))
        .await
        .expect("Failed to connect to NATS");
    let jetstream = Arc::new(async_nats::jetstream::new(nats.clone()));

    let outbound_subject = subjects::outbound("trogon");
    stream::ensure_stream(&jetstream, "trogon", &outbound_subject)
        .await
        .expect("Failed to ensure PROXY_REQUESTS stream");

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .unwrap();
    let proxy_port = listener.local_addr().unwrap().port();

    let proxy_state = ProxyState {
        nats: nats.clone(),
        jetstream: Arc::clone(&jetstream),
        prefix: "trogon".to_string(),
        outbound_subject: outbound_subject.clone(),
        worker_timeout: Duration::from_secs(15),
        base_url_override: Some(mock_server.base_url()),
    };
    tokio::spawn(async move {
        axum::serve(listener, router(proxy_state)).await.ok();
    });

    let http_client = reqwest::Client::builder()
        .timeout(Duration::from_secs(15))
        .build()
        .unwrap();
    let worker_js = Arc::clone(&jetstream);
    let worker_nats = nats.clone();
    let worker_vault = Arc::clone(&vault);
    let worker_stream = stream::stream_name("trogon");
    tokio::spawn(async move {
        worker::run(
            worker_js,
            worker_nats,
            worker_vault,
            http_client,
            "vault-rotate-proxy-worker",
            &worker_stream,
        )
        .await
        .ok();
    });

    let admin_nats = nats.clone();
    let admin_vault = Arc::clone(&vault);
    tokio::spawn(async move {
        vault_admin::run(admin_nats, admin_vault, "trogon")
            .await
            .ok();
    });

    tokio::time::sleep(Duration::from_millis(300)).await;

    // ── Step 1: Store the token with the old plaintext ─────────────────────
    let store_payload = serde_json::json!({
        "token": "tok_anthropic_prod_rotatetest",
        "plaintext": "sk-rotate-old-key"
    });
    let store_resp = nats
        .request(
            subjects::vault_store("trogon"),
            serde_json::to_vec(&store_payload).unwrap().into(),
        )
        .await
        .expect("vault.store NATS request failed");
    let v: serde_json::Value = serde_json::from_slice(&store_resp.payload).unwrap();
    assert_eq!(v["ok"], true, "vault.store must return ok:true");

    // ── Step 2: First proxy request — must reach mock with OLD key ─────────
    let client = reqwest::Client::new();
    let resp = client
        .post(format!("http://127.0.0.1:{proxy_port}/anthropic/v1/messages"))
        .header("Authorization", "Bearer tok_anthropic_prod_rotatetest")
        .header("Content-Type", "application/json")
        .body(r#"{"model":"claude-opus-4-6","max_tokens":10,"messages":[]}"#)
        .timeout(Duration::from_secs(20))
        .send()
        .await
        .expect("First proxy request failed");
    assert_eq!(
        resp.status(),
        200,
        "First proxy request must succeed with old key; got {}",
        resp.status()
    );

    // ── Step 3: Rotate the token to a new plaintext ────────────────────────
    let rotate_payload = serde_json::json!({
        "token": "tok_anthropic_prod_rotatetest",
        "new_plaintext": "sk-rotate-new-key"
    });
    let rotate_resp = nats
        .request(
            subjects::vault_rotate("trogon"),
            serde_json::to_vec(&rotate_payload).unwrap().into(),
        )
        .await
        .expect("vault.rotate NATS request failed");
    let v: serde_json::Value = serde_json::from_slice(&rotate_resp.payload).unwrap();
    assert_eq!(v["ok"], true, "vault.rotate must return ok:true");

    // ── Step 4: Second proxy request — must reach mock with NEW key ────────
    let resp = client
        .post(format!("http://127.0.0.1:{proxy_port}/anthropic/v1/messages"))
        .header("Authorization", "Bearer tok_anthropic_prod_rotatetest")
        .header("Content-Type", "application/json")
        .body(r#"{"model":"claude-opus-4-6","max_tokens":10,"messages":[]}"#)
        .timeout(Duration::from_secs(20))
        .send()
        .await
        .expect("Second proxy request failed");
    assert_eq!(
        resp.status(),
        200,
        "Second proxy request must succeed with new key; got {}",
        resp.status()
    );

    // Critical: old mock hit once (before rotation), new mock hit once (after rotation).
    old_key_mock.assert_async().await;
    new_key_mock.assert_async().await;
}
