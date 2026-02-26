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

use futures_util::future::join_all;
use testcontainers_modules::nats::Nats;
use testcontainers_modules::testcontainers::{runners::AsyncRunner, ContainerAsync, ImageExt};
use trogon_nats::{NatsAuth, NatsConfig, connect};
use trogon_secret_proxy::{
    proxy::{ProxyState, router},
    stream, subjects, worker,
};
use trogon_vault::{ApiKeyToken, MemoryVault, VaultStore};

// ── Shared helper ─────────────────────────────────────────────────────────────

async fn start_nats() -> (ContainerAsync<Nats>, u16) {
    let container: ContainerAsync<Nats> = Nats::default()
        .with_cmd(["--jetstream"])
        .start()
        .await
        .expect("Failed to start NATS container — is Docker running?");
    let port = container.get_host_port_ipv4(4222).await.unwrap();
    (container, port)
}

#[tokio::test]
async fn e2e_token_is_exchanged_for_real_key() {
    // ── 1. Start NATS with JetStream via Docker ──────────────────────────────
    let (_nats_container, nats_port) = start_nats().await;

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
    stream::ensure_stream(&jetstream, "trogon", &outbound_subject)
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
        worker::run(jetstream, nats, vault, http_client, "e2e-test-workers", &stream::stream_name("trogon"))
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
    let (_nats_container, nats_port) = start_nats().await;

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
    stream::ensure_stream(&jetstream, "trogon", &outbound_subject).await.unwrap();

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
        worker::run(jetstream, nats, vault, http_client, "e2e-unknown-workers", &stream::stream_name("trogon")).await
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

/// When no worker is running the proxy must return 504 Gateway Timeout
/// after `worker_timeout` expires.
#[tokio::test]
async fn e2e_proxy_timeout_returns_504() {
    let (_nats_container, nats_port) = start_nats().await;

    let nats_config = NatsConfig {
        servers: vec![format!("localhost:{}", nats_port)],
        auth: NatsAuth::None,
    };
    let nats = connect(&nats_config, Duration::from_secs(10)).await.unwrap();
    let jetstream = Arc::new(async_nats::jetstream::new(nats.clone()));
    let outbound_subject = subjects::outbound("trogon");
    stream::ensure_stream(&jetstream, "trogon", &outbound_subject).await.unwrap();

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let proxy_port = listener.local_addr().unwrap().port();

    let state = ProxyState {
        nats,
        jetstream,
        prefix: "trogon".to_string(),
        outbound_subject,
        // Short timeout so the test completes quickly.
        worker_timeout: Duration::from_secs(2),
        base_url_override: Some("http://127.0.0.1:9".to_string()), // irrelevant
    };
    tokio::spawn(async move { axum::serve(listener, router(state)).await });

    tokio::time::sleep(Duration::from_millis(100)).await;

    // No worker started — proxy will time out waiting for the reply.
    let resp = reqwest::Client::new()
        .post(format!(
            "http://127.0.0.1:{}/anthropic/v1/messages",
            proxy_port
        ))
        .header("Authorization", "Bearer tok_anthropic_test_timeout1")
        .body("{}")
        .timeout(Duration::from_secs(10))
        .send()
        .await
        .unwrap();

    assert_eq!(
        resp.status(),
        504,
        "Expected 504 Gateway Timeout, got {}",
        resp.status()
    );
}

/// Five concurrent requests must all be routed independently — correlation IDs
/// must never be mixed up.  Each receives the same mocked 200 response.
#[tokio::test]
async fn e2e_concurrent_requests_all_succeed() {
    let (_nats_container, nats_port) = start_nats().await;

    let mock_server = httpmock::MockServer::start_async().await;
    let _mock = mock_server
        .mock_async(|when, then| {
            when.method(httpmock::Method::POST).path("/v1/messages");
            then.status(200)
                .header("content-type", "application/json")
                .body(r#"{"id":"msg_concurrent","type":"message"}"#);
        })
        .await;

    let vault = Arc::new(MemoryVault::new());
    let token = ApiKeyToken::new("tok_anthropic_test_conc01").unwrap();
    vault.store(&token, "sk-ant-realkey").await.unwrap();

    let nats_config = NatsConfig {
        servers: vec![format!("localhost:{}", nats_port)],
        auth: NatsAuth::None,
    };
    let nats = connect(&nats_config, Duration::from_secs(10)).await.unwrap();
    let jetstream = Arc::new(async_nats::jetstream::new(nats.clone()));
    let outbound_subject = subjects::outbound("trogon");
    stream::ensure_stream(&jetstream, "trogon", &outbound_subject).await.unwrap();

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let proxy_port = listener.local_addr().unwrap().port();

    let state = ProxyState {
        nats: nats.clone(),
        jetstream: jetstream.clone(),
        prefix: "trogon".to_string(),
        outbound_subject: outbound_subject.clone(),
        worker_timeout: Duration::from_secs(15),
        base_url_override: Some(mock_server.base_url()),
    };
    tokio::spawn(async move { axum::serve(listener, router(state)).await.unwrap() });

    let http_client = reqwest::Client::new();
    tokio::spawn(async move {
        worker::run(jetstream, nats, vault, http_client, "e2e-concurrent-workers", &stream::stream_name("trogon"))
            .await
            .unwrap()
    });

    tokio::time::sleep(Duration::from_millis(200)).await;

    // Fire 5 requests simultaneously.
    let client = reqwest::Client::new();
    let handles: Vec<_> = (0..5)
        .map(|_| {
            let c = client.clone();
            let url = format!("http://127.0.0.1:{}/anthropic/v1/messages", proxy_port);
            tokio::spawn(async move {
                c.post(url)
                    .header("Authorization", "Bearer tok_anthropic_test_conc01")
                    .header("Content-Type", "application/json")
                    .body(r#"{"model":"claude-3","messages":[]}"#)
                    .timeout(Duration::from_secs(20))
                    .send()
                    .await
                    .unwrap()
            })
        })
        .collect();

    let responses = join_all(handles).await;
    for result in responses {
        let resp = result.unwrap();
        assert_eq!(resp.status(), 200, "Expected 200, got {}", resp.status());
        let body: serde_json::Value = resp.json().await.unwrap();
        assert_eq!(body["id"], "msg_concurrent", "Response body mismatch");
    }
}

/// JetStream durability: message published by proxy survives a worker being
/// absent at publish time and is processed when the worker comes online later.
///
/// This exercises the core durability property of the hybrid design: the proxy
/// publishes to a persisted JetStream stream, so the message is not lost even
/// if no worker is running at that moment.
#[tokio::test]
async fn e2e_jetstream_message_processed_after_worker_starts_late() {
    let (_nats_container, nats_port) = start_nats().await;

    let mock_server = httpmock::MockServer::start_async().await;
    let ai_mock = mock_server
        .mock_async(|when, then| {
            when.method(httpmock::Method::POST).path("/v1/messages");
            then.status(200)
                .header("content-type", "application/json")
                .body(r#"{"id":"msg_durable_01","type":"message"}"#);
        })
        .await;

    let vault = Arc::new(MemoryVault::new());
    let token = ApiKeyToken::new("tok_anthropic_test_dur001").unwrap();
    vault.store(&token, "sk-ant-realkey").await.unwrap();

    let nats_config = NatsConfig {
        servers: vec![format!("localhost:{}", nats_port)],
        auth: NatsAuth::None,
    };
    let nats = connect(&nats_config, Duration::from_secs(10)).await.unwrap();
    let jetstream = Arc::new(async_nats::jetstream::new(nats.clone()));
    let outbound_subject = subjects::outbound("trogon");
    stream::ensure_stream(&jetstream, "trogon", &outbound_subject).await.unwrap();

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let proxy_port = listener.local_addr().unwrap().port();

    // Start proxy — no worker running yet.
    let state = ProxyState {
        nats: nats.clone(),
        jetstream: jetstream.clone(),
        prefix: "trogon".to_string(),
        outbound_subject: outbound_subject.clone(),
        worker_timeout: Duration::from_secs(20),
        base_url_override: Some(mock_server.base_url()),
    };
    tokio::spawn(async move { axum::serve(listener, router(state)).await.unwrap() });
    tokio::time::sleep(Duration::from_millis(100)).await;

    // Send request through proxy — it publishes to JetStream and blocks waiting
    // for a reply. The message is now persisted in the stream.
    let request_handle = tokio::spawn(async move {
        reqwest::Client::new()
            .post(format!(
                "http://127.0.0.1:{}/anthropic/v1/messages",
                proxy_port
            ))
            .header("Authorization", "Bearer tok_anthropic_test_dur001")
            .header("Content-Type", "application/json")
            .body(r#"{"model":"claude-3","messages":[]}"#)
            .timeout(Duration::from_secs(20))
            .send()
            .await
            .unwrap()
    });

    // Give the proxy time to publish to JetStream before the worker starts.
    tokio::time::sleep(Duration::from_millis(500)).await;

    // Start the worker now — it must find and process the persisted message.
    let http_client = reqwest::Client::builder()
        .timeout(Duration::from_secs(10))
        .build()
        .unwrap();
    tokio::spawn(async move {
        worker::run(jetstream, nats, vault, http_client, "e2e-durable-workers", &stream::stream_name("trogon"))
            .await
            .expect("Worker error");
    });

    // Proxy receives the worker reply and responds to our original HTTP request.
    let resp = request_handle.await.unwrap();
    assert_eq!(resp.status(), 200, "Expected 200, got {}", resp.status());
    let body: serde_json::Value = resp.json().await.unwrap();
    assert_eq!(body["id"], "msg_durable_01");
    ai_mock.assert_async().await;
}

/// When the worker receives a JetStream message with an invalid (non-JSON)
/// payload it must NAK it and keep running — the next valid request must be
/// processed successfully.
#[tokio::test]
async fn e2e_invalid_payload_is_nacked_and_worker_continues() {
    let (_nats_container, nats_port) = start_nats().await;

    let mock_server = httpmock::MockServer::start_async().await;
    let ai_mock = mock_server
        .mock_async(|when, then| {
            when.method(httpmock::Method::POST).path("/v1/messages");
            then.status(200)
                .header("content-type", "application/json")
                .body(r#"{"id":"msg_after_bad","type":"message"}"#);
        })
        .await;

    let vault = Arc::new(MemoryVault::new());
    let token = ApiKeyToken::new("tok_anthropic_test_nack01").unwrap();
    vault.store(&token, "sk-ant-realkey").await.unwrap();

    let nats_config = NatsConfig {
        servers: vec![format!("localhost:{}", nats_port)],
        auth: NatsAuth::None,
    };
    let nats = connect(&nats_config, Duration::from_secs(10)).await.unwrap();
    let jetstream = Arc::new(async_nats::jetstream::new(nats.clone()));
    let outbound_subject = subjects::outbound("trogon");
    stream::ensure_stream(&jetstream, "trogon", &outbound_subject).await.unwrap();

    // Inject garbage bytes directly into the stream — bypasses the proxy.
    jetstream
        .publish(outbound_subject.clone(), b"this is not valid json !!!".to_vec().into())
        .await
        .unwrap();

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let proxy_port = listener.local_addr().unwrap().port();

    let state = ProxyState {
        nats: nats.clone(),
        jetstream: jetstream.clone(),
        prefix: "trogon".to_string(),
        outbound_subject: outbound_subject.clone(),
        worker_timeout: Duration::from_secs(15),
        base_url_override: Some(mock_server.base_url()),
    };
    tokio::spawn(async move { axum::serve(listener, router(state)).await.unwrap() });

    let http_client = reqwest::Client::builder()
        .timeout(Duration::from_secs(10))
        .build()
        .unwrap();
    tokio::spawn(async move {
        worker::run(jetstream, nats, vault, http_client, "e2e-nack-workers", &stream::stream_name("trogon"))
            .await
            .expect("Worker error");
    });

    tokio::time::sleep(Duration::from_millis(300)).await;

    // Send a valid request — worker must still be alive and process it.
    let resp = reqwest::Client::new()
        .post(format!(
            "http://127.0.0.1:{}/anthropic/v1/messages",
            proxy_port
        ))
        .header("Authorization", "Bearer tok_anthropic_test_nack01")
        .header("Content-Type", "application/json")
        .body(r#"{"model":"claude-3","messages":[]}"#)
        .timeout(Duration::from_secs(20))
        .send()
        .await
        .unwrap();

    assert_eq!(resp.status(), 200, "Expected 200, got {}", resp.status());
    let body: serde_json::Value = resp.json().await.unwrap();
    assert_eq!(body["id"], "msg_after_bad");
    ai_mock.assert_async().await;
}

/// `ensure_stream` must be idempotent — calling it twice on the same subject
/// must not return an error.
#[tokio::test]
async fn e2e_ensure_stream_is_idempotent() {
    let (_nats_container, nats_port) = start_nats().await;

    let nats_config = NatsConfig {
        servers: vec![format!("localhost:{}", nats_port)],
        auth: NatsAuth::None,
    };
    let nats = connect(&nats_config, Duration::from_secs(10)).await.unwrap();
    let jetstream = Arc::new(async_nats::jetstream::new(nats));
    let outbound_subject = subjects::outbound("trogon");

    // First call — creates the stream.
    stream::ensure_stream(&jetstream, "trogon", &outbound_subject)
        .await
        .expect("First ensure_stream call failed");

    // Second call — stream already exists, must succeed without error.
    stream::ensure_stream(&jetstream, "trogon", &outbound_subject)
        .await
        .expect("Second ensure_stream call failed (not idempotent)");
}

/// ProxyError::ReadBody — the incoming request body stream errors mid-read.
///
/// `axum::body::to_bytes` fails when the body stream produces an `Err`.
/// The handler converts that into `ProxyError::ReadBody` → HTTP 500.
///
/// Uses `tower::ServiceExt::oneshot` to drive the router directly with a
/// synthetic broken body, without going through a TCP listener.  NATS is
/// still required to construct `ProxyState`, but is never actually touched
/// because the error occurs before any NATS call.
#[tokio::test]
async fn e2e_read_body_error_returns_500() {
    use tower::ServiceExt as _;

    let (_nats_container, nats_port) = start_nats().await;

    let nats_config = NatsConfig {
        servers: vec![format!("localhost:{}", nats_port)],
        auth: NatsAuth::None,
    };
    let nats = connect(&nats_config, Duration::from_secs(10)).await.unwrap();
    let jetstream = Arc::new(async_nats::jetstream::new(nats.clone()));
    let outbound_subject = subjects::outbound("trogon");
    stream::ensure_stream(&jetstream, "trogon", &outbound_subject)
        .await
        .unwrap();

    let state = ProxyState {
        nats,
        jetstream,
        prefix: "trogon".to_string(),
        outbound_subject,
        worker_timeout: Duration::from_secs(5),
        base_url_override: Some("http://127.0.0.1:1".to_string()),
    };

    // Stream that errors on the first poll — axum::body::to_bytes returns Err.
    let error_stream = futures_util::stream::once(async {
        Err::<bytes::Bytes, std::io::Error>(std::io::Error::other(
            "simulated body read failure",
        ))
    });
    let request = axum::http::Request::builder()
        .method("POST")
        .uri("/anthropic/v1/messages")
        .header("authorization", "Bearer tok_anthropic_test_readbody1")
        .body(axum::body::Body::from_stream(error_stream))
        .unwrap();

    let response = router(state).oneshot(request).await.unwrap();

    assert_eq!(
        response.status(),
        axum::http::StatusCode::INTERNAL_SERVER_ERROR,
        "Body read failure must produce 500"
    );
}

/// ProxyError::ReplyChannelClosed — the NATS connection is closed while the
/// proxy is waiting for a worker reply on its Core NATS subscription.
///
/// `reply_sub.next()` returns `None` when the underlying connection is closed,
/// which the proxy maps to HTTP 500 (InternalServerError).
///
/// The proxy has a long worker_timeout (30 s) so the close must race the
/// timeout.  We sleep 500 ms to let the proxy subscribe and publish before
/// calling `Client::close()`.
#[tokio::test]
async fn e2e_reply_channel_closed_returns_500() {
    let (_nats_container, nats_port) = start_nats().await;

    let nats_config = NatsConfig {
        servers: vec![format!("localhost:{}", nats_port)],
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

    let state = ProxyState {
        nats: nats.clone(),
        jetstream,
        prefix: "trogon".to_string(),
        outbound_subject,
        worker_timeout: Duration::from_secs(30),
        base_url_override: Some("http://127.0.0.1:1".to_string()),
    };

    tokio::spawn(async move {
        axum::serve(listener, router(state)).await.ok();
    });
    tokio::time::sleep(Duration::from_millis(100)).await;

    // Fire the request — proxy subscribes to its reply subject, publishes to
    // JetStream, then blocks waiting for a reply that will never come.
    let request_handle = tokio::spawn(async move {
        reqwest::Client::new()
            .post(format!(
                "http://127.0.0.1:{}/anthropic/v1/messages",
                proxy_port
            ))
            .header("Authorization", "Bearer tok_anthropic_test_closed1")
            .body("{}")
            .timeout(Duration::from_secs(20))
            .send()
            .await
            .unwrap()
    });

    // Give the proxy time to subscribe and publish to JetStream.
    tokio::time::sleep(Duration::from_millis(500)).await;

    // Drain all subscriptions on this connection.  Because the proxy's
    // `state.nats` shares the same underlying connection, its `reply_sub`
    // channel is also closed → `next()` returns `None` →
    // ProxyError::ReplyChannelClosed → 500.
    nats.drain().await.unwrap();

    let resp = request_handle.await.unwrap();
    assert_eq!(resp.status(), 500, "ReplyChannelClosed must return 500");
}

/// Invalid response headers from the worker are silently dropped; valid ones
/// pass through to the caller unchanged.
///
/// In proxy.rs the header-building loop uses `if let (Ok(name), Ok(value))`
/// which silently drops any header whose name or value fails HTTP parsing.
/// This test uses an inline worker that directly injects a mix of valid and
/// invalid entries into `OutboundHttpResponse.headers` to verify that branch.
#[tokio::test]
async fn e2e_invalid_response_headers_silently_dropped() {
    use futures_util::StreamExt as _;
    use tower::ServiceExt as _;
    use trogon_secret_proxy::messages::OutboundHttpResponse;

    let (_nats_container, nats_port) = start_nats().await;

    let nats_config = NatsConfig {
        servers: vec![format!("localhost:{}", nats_port)],
        auth: NatsAuth::None,
    };
    let nats = connect(&nats_config, Duration::from_secs(10)).await.unwrap();
    let jetstream = Arc::new(async_nats::jetstream::new(nats.clone()));
    let outbound_subject = subjects::outbound("trogon");
    stream::ensure_stream(&jetstream, "trogon", &outbound_subject)
        .await
        .unwrap();

    // Register an inline worker that will reply with crafted headers.
    let js_stream = jetstream
        .get_stream(&stream::stream_name("trogon"))
        .await
        .unwrap();
    let consumer = js_stream
        .get_or_create_consumer(
            "e2e-inv-hdr-worker",
            async_nats::jetstream::consumer::pull::Config {
                durable_name: Some("e2e-inv-hdr-worker".to_string()),
                ack_policy: async_nats::jetstream::consumer::AckPolicy::Explicit,
                ..Default::default()
            },
        )
        .await
        .unwrap();
    let mut messages = consumer.messages().await.unwrap();

    let state = ProxyState {
        nats: nats.clone(),
        jetstream,
        prefix: "trogon".to_string(),
        outbound_subject,
        worker_timeout: Duration::from_secs(10),
        base_url_override: Some("http://127.0.0.1:1".to_string()),
    };

    // Drive the axum router directly with tower::oneshot — no TCP listener needed.
    let request = axum::http::Request::builder()
        .method("POST")
        .uri("/anthropic/v1/messages")
        .header("authorization", "Bearer tok_anthropic_test_invhdr1")
        .body(axum::body::Body::from("{}"))
        .unwrap();

    let proxy_handle = tokio::spawn(async move { router(state).oneshot(request).await.unwrap() });

    // Pull the JetStream message and reply with a mix of valid and invalid headers.
    let msg = tokio::time::timeout(Duration::from_secs(5), messages.next())
        .await
        .expect("Timed out waiting for proxy to publish")
        .unwrap()
        .unwrap();

    let parsed: serde_json::Value = serde_json::from_slice(&msg.payload).unwrap();
    let reply_to = parsed["reply_to"].as_str().unwrap().to_string();

    let mut resp_headers = std::collections::HashMap::new();
    resp_headers.insert("content-type".to_string(), "application/json".to_string()); // valid
    resp_headers.insert("x-custom-valid".to_string(), "hello".to_string()); // valid
    resp_headers.insert("invalid header name!".to_string(), "value".to_string()); // invalid name
    resp_headers.insert("x-bad-value".to_string(), "\x00control".to_string()); // invalid value

    let reply = OutboundHttpResponse {
        status: 200,
        headers: resp_headers,
        body: b"{\"ok\":true}".to_vec(),
        error: None,
    };
    nats.publish(reply_to, serde_json::to_vec(&reply).unwrap().into())
        .await
        .unwrap();
    msg.ack().await.unwrap();

    let resp = proxy_handle.await.unwrap();

    assert_eq!(resp.status(), 200, "Proxy must return 200 despite invalid headers");
    // Valid headers must be forwarded.
    assert!(resp.headers().contains_key("content-type"));
    assert!(resp.headers().contains_key("x-custom-valid"));
    // Invalid headers must be silently dropped — not panicking, not forwarded.
    assert_eq!(resp.headers().get("x-bad-value"), None);
}
