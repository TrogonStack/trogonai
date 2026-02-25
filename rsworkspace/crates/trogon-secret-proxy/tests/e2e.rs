//! End-to-end integration test for the HTTP proxy + worker pipeline.
//!
//! Requires Docker (uses testcontainers to spin up a real NATS server).
//!
//! Run with:
//!   cargo test -p trogon-secret-proxy --test e2e
//!
//! What this test verifies:
//!   1. Service sends request with proxy token (tok_...)
//!   2. Proxy publishes to JetStream
//!   3. Worker resolves tok_... → real key via MemoryVault
//!   4. Worker forwards to (mocked) AI provider using the REAL key
//!   5. Proxy returns the AI provider response to the service
//!   6. The real key was NEVER visible to the service

use std::sync::Arc;
use std::time::Duration;

use testcontainers_modules::nats::Nats;
use testcontainers_modules::testcontainers::{runners::AsyncRunner, ContainerAsync, ImageExt};
use trogon_nats::{NatsAuth, NatsConfig, connect};
use trogon_secret_proxy::{
    proxy::{ProxyState, router},
    stream, subjects, worker,
};
use trogon_vault::{ApiKeyToken, MemoryVault, VaultStore};

#[tokio::test]
async fn e2e_token_is_exchanged_for_real_key() {
    // ── 1. Start NATS with JetStream via Docker ──────────────────────────────
    let nats_container: ContainerAsync<Nats> = Nats::default()
        .with_cmd(["--jetstream"])
        .start()
        .await
        .expect("Failed to start NATS container — is Docker running?");

    let nats_port = nats_container
        .get_host_port_ipv4(4222)
        .await
        .expect("Failed to get NATS port");

    // ── 2. Mock the AI provider (simulates Anthropic) ────────────────────────
    // The mock expects the REAL key in the Authorization header, not the token.
    let mock_server = httpmock::MockServer::start_async().await;

    let ai_mock = mock_server
        .mock_async(|when, then| {
            when.method(httpmock::Method::POST)
                .path("/v1/messages")
                .header("authorization", "Bearer sk-ant-realkey");
            then.status(200)
                .header("content-type", "application/json")
                .body(r#"{"id":"msg_test_01","type":"message","content":[]}"#);
        })
        .await;

    // ── 3. Seed the vault ────────────────────────────────────────────────────
    let vault = Arc::new(MemoryVault::new());
    let token = ApiKeyToken::new("tok_anthropic_test_abc123").unwrap();
    vault.store(&token, "sk-ant-realkey").await.unwrap();

    // ── 4. Connect to NATS ───────────────────────────────────────────────────
    let nats_config = NatsConfig {
        servers: vec![format!("localhost:{}", nats_port)],
        auth: NatsAuth::None,
    };
    let nats = connect(&nats_config, Duration::from_secs(10))
        .await
        .expect("Failed to connect to NATS");
    let jetstream = Arc::new(async_nats::jetstream::new(nats.clone()));

    // ── 5. Ensure JetStream stream ───────────────────────────────────────────
    let outbound_subject = subjects::outbound("trogon");
    stream::ensure_stream(&jetstream, &outbound_subject)
        .await
        .expect("Failed to ensure stream");

    // ── 6. Start HTTP proxy on a random port ─────────────────────────────────
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .unwrap();
    let proxy_port = listener.local_addr().unwrap().port();

    let state = ProxyState {
        nats: nats.clone(),
        jetstream: jetstream.clone(),
        prefix: "trogon".to_string(),
        outbound_subject: outbound_subject.clone(),
        worker_timeout: Duration::from_secs(15),
        // Point to mock server instead of real Anthropic
        base_url_override: Some(mock_server.base_url()),
    };

    tokio::spawn(async move {
        axum::serve(listener, router(state))
            .await
            .expect("Proxy server error");
    });

    // ── 7. Start detokenization worker ───────────────────────────────────────
    let http_client = reqwest::Client::builder()
        .timeout(Duration::from_secs(10))
        .build()
        .unwrap();

    tokio::spawn(async move {
        worker::run(jetstream, nats, vault, http_client, "e2e-test-workers")
            .await
            .expect("Worker error");
    });

    // Give proxy + worker time to initialize
    tokio::time::sleep(Duration::from_millis(200)).await;

    // ── 8. Send request through proxy using the proxy token ──────────────────
    let client = reqwest::Client::new();
    let resp = client
        .post(format!(
            "http://127.0.0.1:{}/anthropic/v1/messages",
            proxy_port
        ))
        .header("Authorization", "Bearer tok_anthropic_test_abc123")
        .header("Content-Type", "application/json")
        .body(r#"{"model":"claude-3-5-sonnet-20241022","max_tokens":10,"messages":[{"role":"user","content":"Hi"}]}"#)
        .timeout(Duration::from_secs(20))
        .send()
        .await
        .expect("Request to proxy failed");

    // ── 9. Assertions ─────────────────────────────────────────────────────────
    assert_eq!(
        resp.status(),
        200,
        "Expected 200 from proxy, got {}",
        resp.status()
    );

    let body: serde_json::Value = resp.json().await.unwrap();
    assert_eq!(body["id"], "msg_test_01", "Response body mismatch");

    // The critical assertion: mock verifies the real key was used, not the token.
    // If the worker sent "tok_anthropic_test_abc123" instead of "sk-ant-realkey",
    // the mock would not have matched and this assertion fails.
    ai_mock.assert_async().await;
}

#[tokio::test]
async fn e2e_unknown_token_returns_error() {
    // ── NATS ─────────────────────────────────────────────────────────────────
    let nats_container: ContainerAsync<Nats> = Nats::default().with_cmd(["--jetstream"]).start().await.unwrap();
    let nats_port = nats_container.get_host_port_ipv4(4222).await.unwrap();

    // Mock that should NOT be called (token unknown in vault)
    let mock_server = httpmock::MockServer::start_async().await;
    let _ai_mock = mock_server
        .mock_async(|when, then| {
            when.any_request();
            then.status(200).body("should not reach here");
        })
        .await;

    // Empty vault — no tokens registered
    let vault = Arc::new(MemoryVault::new());

    let nats_config = NatsConfig {
        servers: vec![format!("localhost:{}", nats_port)],
        auth: NatsAuth::None,
    };
    let nats = connect(&nats_config, Duration::from_secs(10)).await.unwrap();
    let jetstream = Arc::new(async_nats::jetstream::new(nats.clone()));
    let outbound_subject = subjects::outbound("trogon");
    stream::ensure_stream(&jetstream, &outbound_subject).await.unwrap();

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let proxy_port = listener.local_addr().unwrap().port();

    let state = ProxyState {
        nats: nats.clone(),
        jetstream: jetstream.clone(),
        prefix: "trogon".to_string(),
        outbound_subject: outbound_subject.clone(),
        worker_timeout: Duration::from_secs(5),
        base_url_override: Some(mock_server.base_url()),
    };
    tokio::spawn(async move { axum::serve(listener, router(state)).await });

    let http_client = reqwest::Client::new();
    tokio::spawn(async move {
        worker::run(jetstream, nats, vault, http_client, "e2e-unknown-workers").await
    });

    tokio::time::sleep(Duration::from_millis(200)).await;

    let resp = reqwest::Client::new()
        .post(format!(
            "http://127.0.0.1:{}/anthropic/v1/messages",
            proxy_port
        ))
        .header("Authorization", "Bearer tok_anthropic_test_notfound")
        .body("{}")
        .timeout(Duration::from_secs(10))
        .send()
        .await
        .unwrap();

    // Worker should reply with 401 (token not found) wrapped as 502 by proxy
    assert!(
        resp.status().is_client_error() || resp.status().is_server_error(),
        "Expected error status, got {}",
        resp.status()
    );
}
