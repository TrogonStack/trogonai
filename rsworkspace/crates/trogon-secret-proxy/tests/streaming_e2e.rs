//! End-to-end integration tests for the streaming path (phases 3–4).
//!
//! These tests exercise the full pipeline:
//!
//!   Client → Proxy (axum) → JetStream → Worker → AI provider (mock)
//!                                                ↓ SSE chunks
//!   Client ← Proxy (axum) ← Core NATS  ← Worker  (Start/Chunk/End frames)
//!
//! Requires Docker (testcontainers spins up a real NATS server).
//!
//! Run with:
//!   cargo test -p trogon-secret-proxy --test streaming_e2e

use std::sync::Arc;
use std::time::Duration;

use testcontainers_modules::nats::Nats;
use testcontainers_modules::testcontainers::{ContainerAsync, ImageExt, runners::AsyncRunner};
use trogon_nats::{NatsAuth, NatsConfig, connect};
use trogon_secret_proxy::{proxy::{ProxyState, router}, stream, subjects, worker};
use trogon_vault::{ApiKeyToken, MemoryVault, VaultStore};

// ── Helpers ───────────────────────────────────────────────────────────────────

async fn start_nats() -> (ContainerAsync<Nats>, u16) {
    let container: ContainerAsync<Nats> = Nats::default()
        .with_cmd(["--jetstream"])
        .start()
        .await
        .expect("Failed to start NATS container — is Docker running?");
    let port = container.get_host_port_ipv4(4222).await.unwrap();
    (container, port)
}

/// Spin up the full proxy+worker pipeline and return the proxy port.
///
/// The vault is pre-seeded with `tok_anthropic_test_abc123 → sk-ant-realkey`.
/// The AI provider URL is overridden to `ai_base_url` (mock server).
async fn start_pipeline(
    nats_port: u16,
    ai_base_url: &str,
    consumer_name: &str,
    vault: Arc<MemoryVault>,
) -> u16 {
    let nats_config = NatsConfig {
        servers: vec![format!("localhost:{}", nats_port)],
        auth: NatsAuth::None,
    };
    let nats = connect(&nats_config, Duration::from_secs(10))
        .await
        .expect("Failed to connect to NATS");
    let jetstream = Arc::new(async_nats::jetstream::new(nats.clone()));

    let outbound_subject = subjects::outbound("trogon");
    stream::ensure_stream(&jetstream, "trogon", &outbound_subject)
        .await
        .expect("Failed to ensure stream");

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let proxy_port = listener.local_addr().unwrap().port();

    let state = ProxyState {
        nats: nats.clone(),
        jetstream: jetstream.clone(),
        prefix: "trogon".to_string(),
        outbound_subject: outbound_subject.clone(),
        worker_timeout: Duration::from_secs(15),
        base_url_override: Some(ai_base_url.to_string()),
    };
    tokio::spawn(async move { axum::serve(listener, router(state)).await });

    let http_client = reqwest::Client::builder()
        .timeout(Duration::from_secs(10))
        .build()
        .unwrap();
    let consumer = consumer_name.to_string();
    let stream_name = stream::stream_name("trogon");
    tokio::spawn(async move {
        worker::run(jetstream, nats, vault, http_client, &consumer, &stream_name)
            .await
            .expect("Worker error")
    });

    // Let proxy + worker initialise before sending requests.
    tokio::time::sleep(Duration::from_millis(300)).await;
    proxy_port
}

/// SSE body used in happy-path tests: three events.
const SSE_BODY: &str = concat!(
    "event: message_start\n",
    "data: {\"type\":\"message_start\",\"message\":{\"id\":\"msg_01\"}}\n\n",
    "event: content_block_delta\n",
    "data: {\"type\":\"content_block_delta\",\"index\":0,\"delta\":{\"type\":\"text_delta\",\"text\":\"Hello\"}}\n\n",
    "event: message_stop\n",
    "data: {\"type\":\"message_stop\"}\n\n",
);

// ── Tests ─────────────────────────────────────────────────────────────────────

/// Happy path: a request body with `"stream": true` flows through the full
/// proxy → worker → AI-provider pipeline and the response body reassembles
/// correctly on the client side.
#[tokio::test]
async fn e2e_streaming_assembles_full_body() {
    let (_nats, nats_port) = start_nats().await;

    let mock = httpmock::MockServer::start_async().await;
    mock.mock_async(|when, then| {
        when.method("POST")
            .path("/v1/messages")
            .header("authorization", "Bearer sk-ant-realkey");
        then.status(200)
            .header("content-type", "text/event-stream")
            .body(SSE_BODY);
    })
    .await;

    let vault = Arc::new(MemoryVault::new());
    vault
        .store(
            &ApiKeyToken::new("tok_anthropic_test_abc123").unwrap(),
            "sk-ant-realkey",
        )
        .await
        .unwrap();

    let proxy_port = start_pipeline(nats_port, &mock.base_url(), "stream-happy-worker", vault).await;

    let body_json = serde_json::json!({
        "model": "claude-3-5-sonnet-20241022",
        "max_tokens": 10,
        "stream": true,
        "messages": [{"role": "user", "content": "Hi"}]
    });

    let resp = reqwest::Client::new()
        .post(format!("http://127.0.0.1:{}/anthropic/v1/messages", proxy_port))
        .header("Authorization", "Bearer tok_anthropic_test_abc123")
        .header("Content-Type", "application/json")
        .json(&body_json)
        .timeout(Duration::from_secs(20))
        .send()
        .await
        .expect("Request to proxy failed");

    assert_eq!(resp.status(), 200, "Expected 200, got {}", resp.status());

    let ct = resp
        .headers()
        .get("content-type")
        .and_then(|v| v.to_str().ok())
        .unwrap_or_default();
    assert!(ct.contains("text/event-stream"), "content-type must be text/event-stream, got: {ct}");

    // The real key must never appear in any response header forwarded to the client.
    for (name, value) in resp.headers() {
        assert!(
            !value.to_str().unwrap_or("").contains("sk-ant-realkey"),
            "Real API key must not leak in response header '{}'",
            name
        );
    }

    let body = resp.bytes().await.expect("Failed to read response body");
    assert_eq!(
        body.as_ref(),
        SSE_BODY.as_bytes(),
        "Response body must match SSE source byte-for-byte"
    );
}

/// The worker detects `stream: true`, so the non-streaming `OutboundHttpResponse`
/// protocol is NOT used — the proxy must NOT try to deserialize the reply as a
/// non-streaming response and hang.  This test confirms the proxy returns quickly.
#[tokio::test]
async fn e2e_streaming_does_not_use_non_streaming_protocol() {
    let (_nats, nats_port) = start_nats().await;

    let mock = httpmock::MockServer::start_async().await;
    mock.mock_async(|when, then| {
        when.method("POST").path("/v1/messages");
        then.status(200)
            .header("content-type", "text/event-stream")
            .body("event: message_stop\ndata: {}\n\n");
    })
    .await;

    let vault = Arc::new(MemoryVault::new());
    vault
        .store(
            &ApiKeyToken::new("tok_anthropic_test_abc123").unwrap(),
            "sk-ant-realkey",
        )
        .await
        .unwrap();

    let proxy_port =
        start_pipeline(nats_port, &mock.base_url(), "stream-protocol-worker", vault).await;

    let body_json = serde_json::json!({
        "stream": true,
        "messages": [{"role": "user", "content": "Hi"}]
    });

    // If the proxy falls back to the non-streaming path it would time out waiting
    // for an OutboundHttpResponse that never comes. Set a tight timeout so the
    // test fails fast rather than hanging for 15 s.
    let resp = tokio::time::timeout(
        Duration::from_secs(10),
        reqwest::Client::new()
            .post(format!("http://127.0.0.1:{}/anthropic/v1/messages", proxy_port))
            .header("Authorization", "Bearer tok_anthropic_test_abc123")
            .header("Content-Type", "application/json")
            .json(&body_json)
            .send(),
    )
    .await
    .expect("Proxy timed out — streaming path likely fell back to non-streaming protocol")
    .expect("Request failed");

    assert_eq!(resp.status(), 200);
}

/// When the proxy token is not found in the vault the worker publishes
/// Start(401) + Chunk(error_body) + End.  The proxy must forward status 401
/// to the caller rather than timing out or returning 504.
#[tokio::test]
async fn e2e_streaming_unknown_token_returns_401() {
    let (_nats, nats_port) = start_nats().await;

    // Mock that should NOT be called.
    let mock = httpmock::MockServer::start_async().await;
    let not_called = mock
        .mock_async(|when, then| {
            when.any_request();
            then.status(200).body("should not reach here");
        })
        .await;

    let vault = Arc::new(MemoryVault::new()); // empty — no tokens

    let proxy_port =
        start_pipeline(nats_port, &mock.base_url(), "stream-unknown-token-worker", vault).await;

    let body_json = serde_json::json!({
        "stream": true,
        "messages": [{"role": "user", "content": "Hi"}]
    });

    let resp = reqwest::Client::new()
        .post(format!("http://127.0.0.1:{}/anthropic/v1/messages", proxy_port))
        .header("Authorization", "Bearer tok_anthropic_test_notfound")
        .header("Content-Type", "application/json")
        .json(&body_json)
        .timeout(Duration::from_secs(20))
        .send()
        .await
        .expect("Request failed");

    assert_eq!(
        resp.status(),
        401,
        "Unknown token in streaming request must return 401, got {}",
        resp.status()
    );
    assert_eq!(not_called.hits(), 0, "AI provider must not be called");
}

/// When the upstream AI provider returns a non-200 status, the worker publishes
/// Start(upstream_status) + Chunk(body) + End and the proxy forwards that status.
#[tokio::test]
async fn e2e_streaming_upstream_error_status_is_forwarded() {
    let (_nats, nats_port) = start_nats().await;

    let mock = httpmock::MockServer::start_async().await;
    mock.mock_async(|when, then| {
        when.method("POST").path("/v1/messages");
        then.status(529)
            .header("content-type", "application/json")
            .body(r#"{"error":"overloaded"}"#);
    })
    .await;

    let vault = Arc::new(MemoryVault::new());
    vault
        .store(
            &ApiKeyToken::new("tok_anthropic_test_abc123").unwrap(),
            "sk-ant-realkey",
        )
        .await
        .unwrap();

    let proxy_port =
        start_pipeline(nats_port, &mock.base_url(), "stream-upstream-error-worker", vault).await;

    let body_json = serde_json::json!({
        "stream": true,
        "messages": [{"role": "user", "content": "Hi"}]
    });

    let resp = reqwest::Client::new()
        .post(format!("http://127.0.0.1:{}/anthropic/v1/messages", proxy_port))
        .header("Authorization", "Bearer tok_anthropic_test_abc123")
        .header("Content-Type", "application/json")
        .json(&body_json)
        .timeout(Duration::from_secs(20))
        .send()
        .await
        .expect("Request failed");

    assert_eq!(
        resp.status(),
        529,
        "Upstream error status must be forwarded, got {}",
        resp.status()
    );

    let body = resp.bytes().await.unwrap();
    assert_eq!(body.as_ref(), b"{\"error\":\"overloaded\"}");
}

/// A large SSE body (many events) must arrive complete and in order.
/// This verifies that chunk sequencing survives the NATS round-trip.
#[tokio::test]
async fn e2e_streaming_large_body_is_preserved_in_order() {
    let (_nats, nats_port) = start_nats().await;

    // Build a body large enough to span multiple reqwest chunks (>128 KB).
    let mut large_sse = String::new();
    for i in 0..3000 {
        large_sse.push_str(&format!(
            "event: content_block_delta\ndata: {{\"index\":{i},\"delta\":{{\"text\":\"chunk-{i:06}\"}}}}\n\n"
        ));
    }
    large_sse.push_str("event: message_stop\ndata: {}\n\n");

    let mock = httpmock::MockServer::start_async().await;
    let expected_body = large_sse.clone();
    mock.mock_async(move |when, then| {
        when.method("POST").path("/v1/messages");
        then.status(200)
            .header("content-type", "text/event-stream")
            .body(expected_body.clone());
    })
    .await;

    let vault = Arc::new(MemoryVault::new());
    vault
        .store(
            &ApiKeyToken::new("tok_anthropic_test_abc123").unwrap(),
            "sk-ant-realkey",
        )
        .await
        .unwrap();

    let proxy_port =
        start_pipeline(nats_port, &mock.base_url(), "stream-large-body-worker", vault).await;

    let body_json = serde_json::json!({
        "stream": true,
        "messages": [{"role": "user", "content": "Hi"}]
    });

    let resp = reqwest::Client::new()
        .post(format!("http://127.0.0.1:{}/anthropic/v1/messages", proxy_port))
        .header("Authorization", "Bearer tok_anthropic_test_abc123")
        .header("Content-Type", "application/json")
        .json(&body_json)
        .timeout(Duration::from_secs(30))
        .send()
        .await
        .expect("Request failed");

    assert_eq!(resp.status(), 200, "Expected 200, got {}", resp.status());

    let received = resp.bytes().await.expect("Failed to read body");
    assert_eq!(
        received.len(),
        large_sse.len(),
        "Body length mismatch: got {} bytes, expected {}",
        received.len(),
        large_sse.len()
    );
    assert_eq!(
        received.as_ref(),
        large_sse.as_bytes(),
        "Body content must match byte-for-byte"
    );
}

/// Non-streaming requests must continue to work correctly when the same proxy
/// and worker are handling both streaming and non-streaming requests.
#[tokio::test]
async fn e2e_non_streaming_still_works_alongside_streaming() {
    let (_nats, nats_port) = start_nats().await;

    let mock = httpmock::MockServer::start_async().await;
    mock.mock_async(|when, then| {
        when.method("POST").path("/v1/messages");
        then.status(200)
            .header("content-type", "application/json")
            .body(r#"{"id":"msg_nonstream","type":"message"}"#);
    })
    .await;

    let vault = Arc::new(MemoryVault::new());
    vault
        .store(
            &ApiKeyToken::new("tok_anthropic_test_abc123").unwrap(),
            "sk-ant-realkey",
        )
        .await
        .unwrap();

    let proxy_port =
        start_pipeline(nats_port, &mock.base_url(), "stream-mixed-worker", vault).await;

    // Non-streaming request — no `"stream"` key in the body.
    let body_json = serde_json::json!({
        "model": "claude-3-5-sonnet-20241022",
        "max_tokens": 10,
        "messages": [{"role": "user", "content": "Hi"}]
    });

    let resp = reqwest::Client::new()
        .post(format!("http://127.0.0.1:{}/anthropic/v1/messages", proxy_port))
        .header("Authorization", "Bearer tok_anthropic_test_abc123")
        .header("Content-Type", "application/json")
        .json(&body_json)
        .timeout(Duration::from_secs(20))
        .send()
        .await
        .expect("Request failed");

    assert_eq!(resp.status(), 200, "Non-streaming request must still work");

    let body: serde_json::Value = resp.json().await.unwrap();
    assert_eq!(body["id"], "msg_nonstream");
}
