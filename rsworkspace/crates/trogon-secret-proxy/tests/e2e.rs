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

// ── Shared helpers ────────────────────────────────────────────────────────────

async fn start_nats() -> (ContainerAsync<Nats>, u16) {
    let container: ContainerAsync<Nats> = Nats::default()
        .with_cmd(["--jetstream"])
        .start()
        .await
        .expect("Failed to start NATS container — is Docker running?");
    let port = container.get_host_port_ipv4(4222).await.unwrap();
    (container, port)
}

/// Find the first value for a header `name` in the serialised
/// `Vec<(String, String)>` JSON payload (`[["name","value"],...]`).
fn find_header<'a>(headers_json: &'a serde_json::Value, name: &str) -> Option<&'a str> {
    headers_json.as_array()?.iter().find_map(|pair| {
        let arr = pair.as_array()?;
        if arr.first()?.as_str()? == name {
            arr.get(1)?.as_str()
        } else {
            None
        }
    })
}

/// Return all values for a header `name` in the serialised headers JSON.
fn all_header_values(headers_json: &serde_json::Value, name: &str) -> Vec<String> {
    headers_json
        .as_array()
        .unwrap_or(&vec![])
        .iter()
        .filter_map(|pair| {
            let arr = pair.as_array()?;
            if arr.first()?.as_str()? == name {
                Some(arr.get(1)?.as_str()?.to_string())
            } else {
                None
            }
        })
        .collect()
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

    let resp_headers = vec![
        ("content-type".to_string(), "application/json".to_string()),  // valid
        ("x-custom-valid".to_string(), "hello".to_string()),           // valid
        ("invalid header name!".to_string(), "value".to_string()),     // invalid name
        ("x-bad-value".to_string(), "\x00control".to_string()),        // invalid value
    ];

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

/// When the worker sets `error: Some(...)` in the response AND `status: 200`,
/// the proxy must still return 502 Bad Gateway.
///
/// The `error` field takes precedence over `status` — proxy.rs checks
/// `if let Some(err) = proxy_response.error` before reading the status code.
#[tokio::test]
async fn e2e_worker_error_field_with_status_200_returns_502() {
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

    let js_stream = jetstream
        .get_stream(&stream::stream_name("trogon"))
        .await
        .unwrap();
    let consumer = js_stream
        .get_or_create_consumer(
            "e2e-err-status200-worker",
            async_nats::jetstream::consumer::pull::Config {
                durable_name: Some("e2e-err-status200-worker".to_string()),
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

    let request = axum::http::Request::builder()
        .method("POST")
        .uri("/anthropic/v1/messages")
        .header("authorization", "Bearer tok_anthropic_test_errstatus1")
        .body(axum::body::Body::from("{}"))
        .unwrap();

    let proxy_handle = tokio::spawn(async move { router(state).oneshot(request).await.unwrap() });

    let msg = tokio::time::timeout(Duration::from_secs(5), messages.next())
        .await
        .expect("Timed out waiting for proxy to publish")
        .unwrap()
        .unwrap();

    let parsed: serde_json::Value = serde_json::from_slice(&msg.payload).unwrap();
    let reply_to = parsed["reply_to"].as_str().unwrap().to_string();

    // Reply with status 200 BUT error field set — error must win.
    let reply = OutboundHttpResponse {
        status: 200,
        headers: vec![],
        body: b"this body should not reach the caller".to_vec(),
        error: Some("upstream exploded despite 200".to_string()),
    };
    nats.publish(reply_to, serde_json::to_vec(&reply).unwrap().into())
        .await
        .unwrap();
    msg.ack().await.unwrap();

    let resp = proxy_handle.await.unwrap();
    assert_eq!(
        resp.status(),
        axum::http::StatusCode::BAD_GATEWAY,
        "error field must take precedence over status 200 → 502"
    );
    let body_bytes = axum::body::to_bytes(resp.into_body(), usize::MAX).await.unwrap();
    assert!(
        std::str::from_utf8(&body_bytes).unwrap().contains("upstream exploded"),
        "Error message must be forwarded in the body"
    );
}

/// JetStream redelivery: a "worker" that NAKs a message simulates a crash
/// mid-request.  After the NAK, a real worker picks up the redelivered
/// message and completes the request successfully.
///
/// This exercises the JetStream durability guarantee that ensures a
/// message is not lost even if the first worker that pulled it fails.
#[tokio::test]
async fn e2e_nacked_message_redelivered_to_second_worker() {
    let (_nats_container, nats_port) = start_nats().await;

    let mock_server = httpmock::MockServer::start_async().await;
    let ai_mock = mock_server
        .mock_async(|when, then| {
            when.method(httpmock::Method::POST).path("/v1/messages");
            then.status(200)
                .header("content-type", "application/json")
                .body(r#"{"id":"msg_redelivered","type":"message"}"#);
        })
        .await;

    let vault = Arc::new(MemoryVault::new());
    let token = ApiKeyToken::new("tok_anthropic_test_ndlv01").unwrap();
    vault.store(&token, "sk-ant-realkey").await.unwrap();

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

    // ── Start proxy (no worker yet) ──────────────────────────────────────────
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let proxy_port = listener.local_addr().unwrap().port();

    let state = ProxyState {
        nats: nats.clone(),
        jetstream: jetstream.clone(),
        prefix: "trogon".to_string(),
        outbound_subject: outbound_subject.clone(),
        worker_timeout: Duration::from_secs(30),
        base_url_override: Some(mock_server.base_url()),
    };
    tokio::spawn(async move { axum::serve(listener, router(state)).await.unwrap() });
    tokio::time::sleep(Duration::from_millis(100)).await;

    // ── Fire the request — proxy publishes to JetStream and waits ────────────
    let request_handle = tokio::spawn(async move {
        reqwest::Client::new()
            .post(format!("http://127.0.0.1:{}/anthropic/v1/messages", proxy_port))
            .header("Authorization", "Bearer tok_anthropic_test_ndlv01")
            .header("Content-Type", "application/json")
            .body(r#"{"model":"claude-3","messages":[]}"#)
            .timeout(Duration::from_secs(35))
            .send()
            .await
            .unwrap()
    });
    tokio::time::sleep(Duration::from_millis(300)).await;

    // ── Saboteur: pull the JetStream message without ACKing ──────────────────
    // Configure a 2-second ack_wait so the un-ACKed message is redelivered
    // quickly rather than waiting for the 30-second JetStream default.
    // Dropping the saboteur handles without ACKing simulates a worker crash.
    use futures_util::StreamExt as _;
    let consumer_name = "e2e-redeliver-workers";
    {
        let js_stream = jetstream.get_stream(&stream::stream_name("trogon")).await.unwrap();
        let bad_consumer = js_stream
            .get_or_create_consumer(
                consumer_name,
                async_nats::jetstream::consumer::pull::Config {
                    durable_name: Some(consumer_name.to_string()),
                    ack_policy: async_nats::jetstream::consumer::AckPolicy::Explicit,
                    ack_wait: Duration::from_secs(2),
                    ..Default::default()
                },
            )
            .await
            .unwrap();
        let mut bad_messages = bad_consumer.messages().await.unwrap();

        // Pull the message but do NOT ack — simulates crash mid-request.
        let _bad_msg = tokio::time::timeout(Duration::from_secs(5), bad_messages.next())
            .await
            .expect("Saboteur timed out pulling message")
            .unwrap()
            .unwrap();

        // Scope ends here — bad_consumer and bad_messages are dropped without ACKing.
        // After ack_wait (2 s) the message is redelivered to the same consumer.
    }

    // Wait for ack_wait to expire so the message is available for redelivery.
    tokio::time::sleep(Duration::from_secs(3)).await;

    // ── Real worker: picks up the redelivered message ────────────────────────
    let http_client = reqwest::Client::builder()
        .timeout(Duration::from_secs(10))
        .build()
        .unwrap();
    tokio::spawn(async move {
        worker::run(
            jetstream,
            nats,
            vault,
            http_client,
            consumer_name,
            &stream::stream_name("trogon"),
        )
        .await
        .expect("Worker error");
    });

    let resp = request_handle.await.unwrap();
    assert_eq!(resp.status(), 200, "Redelivered message must be processed successfully");
    let body: serde_json::Value = resp.json().await.unwrap();
    assert_eq!(body["id"], "msg_redelivered");
    ai_mock.assert_async().await;
}

/// When the worker returns an out-of-range HTTP status code (e.g. 1000),
/// `StatusCode::from_u16` fails and the proxy falls back to 500
/// Internal Server Error.
///
/// This exercises the `unwrap_or(StatusCode::INTERNAL_SERVER_ERROR)` branch
/// in proxy.rs where the worker's status field is converted.
#[tokio::test]
async fn e2e_worker_invalid_status_code_falls_back_to_500() {
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

    // Register an inline worker that will reply with status 1000 (out of range).
    let js_stream = jetstream
        .get_stream(&stream::stream_name("trogon"))
        .await
        .unwrap();
    let consumer = js_stream
        .get_or_create_consumer(
            "e2e-inv-status-worker",
            async_nats::jetstream::consumer::pull::Config {
                durable_name: Some("e2e-inv-status-worker".to_string()),
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

    let request = axum::http::Request::builder()
        .method("POST")
        .uri("/anthropic/v1/messages")
        .header("authorization", "Bearer tok_anthropic_test_invsts1")
        .body(axum::body::Body::from("{}"))
        .unwrap();

    let proxy_handle = tokio::spawn(async move { router(state).oneshot(request).await.unwrap() });

    // Pull the JetStream message and reply with status 1000 — StatusCode::from_u16
    // rejects values > 999, triggering the unwrap_or(INTERNAL_SERVER_ERROR) fallback.
    let msg = tokio::time::timeout(Duration::from_secs(5), messages.next())
        .await
        .expect("Timed out waiting for proxy to publish")
        .unwrap()
        .unwrap();

    let parsed: serde_json::Value = serde_json::from_slice(&msg.payload).unwrap();
    let reply_to = parsed["reply_to"].as_str().unwrap().to_string();

    let reply = OutboundHttpResponse {
        status: 1000,
        headers: vec![],
        body: b"bad status".to_vec(),
        error: None,
    };
    nats.publish(reply_to, serde_json::to_vec(&reply).unwrap().into())
        .await
        .unwrap();
    msg.ack().await.unwrap();

    let resp = proxy_handle.await.unwrap();
    assert_eq!(
        resp.status(),
        axum::http::StatusCode::INTERNAL_SERVER_ERROR,
        "Out-of-range status code must fall back to 500"
    );
}

/// When every worker that receives a JetStream message crashes (drops without
/// ACKing), JetStream exhausts `max_deliver` retries and stops redelivering.
/// Since no worker ever publishes a reply, the proxy times out → 504.
///
/// Setup: consumer with `max_deliver=3` and `ack_wait=1s`.
/// Saboteur pulls each of the 3 deliveries and drops without ACKing.
/// After the 3rd expiry no more deliveries happen; proxy worker_timeout fires.
#[tokio::test]
async fn e2e_max_deliver_exhausted_proxy_returns_504() {
    use futures_util::StreamExt as _;

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
        nats,
        jetstream: jetstream.clone(),
        prefix: "trogon".to_string(),
        outbound_subject,
        // Longer than 3 * ack_wait (3 s) so all redeliveries exhaust first.
        worker_timeout: Duration::from_secs(8),
        base_url_override: Some("http://127.0.0.1:1".to_string()),
    };
    tokio::spawn(async move { axum::serve(listener, router(state)).await.ok() });
    tokio::time::sleep(Duration::from_millis(100)).await;

    // Saboteur: pulls each delivery and drops without ACKing.
    // max_deliver=3 means JetStream gives up after 3 failed attempts.
    let consumer_name = "e2e-maxdeliver-workers";
    let saboteur_js = jetstream.clone();
    tokio::spawn(async move {
        let js_stream = saboteur_js
            .get_stream(&stream::stream_name("trogon"))
            .await
            .unwrap();
        let consumer = js_stream
            .get_or_create_consumer(
                consumer_name,
                async_nats::jetstream::consumer::pull::Config {
                    durable_name: Some(consumer_name.to_string()),
                    ack_policy: async_nats::jetstream::consumer::AckPolicy::Explicit,
                    ack_wait: Duration::from_secs(1),
                    max_deliver: 3,
                    ..Default::default()
                },
            )
            .await
            .unwrap();
        let mut messages = consumer.messages().await.unwrap();
        for _ in 0..3 {
            match tokio::time::timeout(Duration::from_secs(5), messages.next()).await {
                Ok(Some(Ok(msg))) => drop(msg), // no ACK = simulates crash
                _ => break,
            }
        }
    });

    let resp = reqwest::Client::new()
        .post(format!("http://127.0.0.1:{}/anthropic/v1/messages", proxy_port))
        .header("Authorization", "Bearer tok_anthropic_test_maxdlv1")
        .body("{}")
        .timeout(Duration::from_secs(15))
        .send()
        .await
        .unwrap();

    assert_eq!(
        resp.status(),
        504,
        "All deliveries exhausted → no reply → proxy must return 504"
    );
}

/// Hop-by-hop headers sent by the client (Connection, Keep-Alive) must be
/// stripped by the proxy before publishing to JetStream — they must NOT
/// appear in `OutboundHttpRequest.headers`.
///
/// RFC 7230 §6.1: hop-by-hop headers are meaningful only for a single
/// transport hop and must be removed by any intermediary proxy.
///
/// This test uses an inline worker to inspect the raw `headers` field of
/// the JetStream message directly.
#[tokio::test]
async fn e2e_hop_by_hop_headers_stripped_from_jetstream_message() {
    use futures_util::StreamExt as _;
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

    // Inline consumer — inspects the raw JetStream message headers.
    let js_stream = jetstream.get_stream(&stream::stream_name("trogon")).await.unwrap();
    let consumer = js_stream
        .get_or_create_consumer(
            "e2e-hoptohop-worker",
            async_nats::jetstream::consumer::pull::Config {
                durable_name: Some("e2e-hoptohop-worker".to_string()),
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

    // Request carrying hop-by-hop headers alongside a custom end-to-end header.
    let request = axum::http::Request::builder()
        .method("POST")
        .uri("/anthropic/v1/messages")
        .header("authorization", "Bearer tok_anthropic_test_hophop01")
        .header("connection", "keep-alive")
        .header("keep-alive", "timeout=5, max=100")
        .header("x-end-to-end", "must-survive")
        .body(axum::body::Body::from("{}"))
        .unwrap();

    let proxy_handle = tokio::spawn(async move { router(state).oneshot(request).await.unwrap() });

    let msg = tokio::time::timeout(Duration::from_secs(5), messages.next())
        .await
        .expect("Timed out waiting for JetStream message")
        .unwrap()
        .unwrap();

    let parsed: serde_json::Value = serde_json::from_slice(&msg.payload).unwrap();
    let headers_in_message = &parsed["headers"];

    // Hop-by-hop headers must be absent from the JetStream message.
    assert!(
        find_header(headers_in_message, "connection").is_none(),
        "connection header must be stripped from the JetStream message"
    );
    assert!(
        find_header(headers_in_message, "keep-alive").is_none(),
        "keep-alive header must be stripped from the JetStream message"
    );
    // End-to-end custom headers must still be forwarded.
    assert_eq!(
        find_header(headers_in_message, "x-end-to-end"),
        Some("must-survive"),
        "end-to-end custom header must be present in the JetStream message"
    );

    // Reply so the proxy handle completes (avoids hanging the test).
    let reply_to = parsed["reply_to"].as_str().unwrap().to_string();
    let reply = trogon_secret_proxy::messages::OutboundHttpResponse {
        status: 200,
        headers: vec![],
        body: b"ok".to_vec(),
        error: None,
    };
    nats.publish(reply_to, serde_json::to_vec(&reply).unwrap().into())
        .await
        .unwrap();
    msg.ack().await.unwrap();

    let resp = proxy_handle.await.unwrap();
    assert_eq!(resp.status(), 200);
}

/// A request body sent as multiple stream chunks must arrive at the AI
/// provider as a single, correctly assembled payload.
///
/// The proxy reads the full body with `axum::body::to_bytes(req.into_body(),
/// usize::MAX)` before passing it to the worker.  This test verifies that
/// two separate chunks are concatenated correctly and the mock AI provider
/// receives the complete JSON body.
#[tokio::test]
async fn e2e_chunked_request_body_buffered_and_forwarded() {
    use tower::ServiceExt as _;

    let (_nats_container, nats_port) = start_nats().await;

    let mock_server = httpmock::MockServer::start_async().await;
    let expected_body = r#"{"model":"claude-3","messages":[]}"#;
    let ai_mock = mock_server
        .mock_async(|when, then| {
            when.method(httpmock::Method::POST)
                .path("/v1/messages")
                .body(expected_body); // exact body match
            then.status(200)
                .header("content-type", "application/json")
                .body(r#"{"id":"msg_chunked","type":"message"}"#);
        })
        .await;

    let vault = Arc::new(MemoryVault::new());
    let token = ApiKeyToken::new("tok_anthropic_test_chunk01").unwrap();
    vault.store(&token, "sk-ant-realkey").await.unwrap();

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
        nats: nats.clone(),
        jetstream: jetstream.clone(),
        prefix: "trogon".to_string(),
        outbound_subject,
        worker_timeout: Duration::from_secs(15),
        base_url_override: Some(mock_server.base_url()),
    };

    let http_client = reqwest::Client::builder()
        .timeout(Duration::from_secs(10))
        .build()
        .unwrap();
    tokio::spawn(async move {
        worker::run(
            jetstream,
            nats,
            vault,
            http_client,
            "e2e-chunk-workers",
            &stream::stream_name("trogon"),
        )
        .await
        .expect("Worker error");
    });
    tokio::time::sleep(Duration::from_millis(200)).await;

    // Body split into two chunks: the proxy must concatenate them before
    // forwarding to the worker.
    let chunk1 = bytes::Bytes::from_static(b"{\"model\":\"claude-3\",");
    let chunk2 = bytes::Bytes::from_static(b"\"messages\":[]}");
    let body_stream = futures_util::stream::iter(vec![
        Ok::<bytes::Bytes, std::io::Error>(chunk1),
        Ok::<bytes::Bytes, std::io::Error>(chunk2),
    ]);

    let request = axum::http::Request::builder()
        .method("POST")
        .uri("/anthropic/v1/messages")
        .header("authorization", "Bearer tok_anthropic_test_chunk01")
        .header("content-type", "application/json")
        .body(axum::body::Body::from_stream(body_stream))
        .unwrap();

    let response = router(state).oneshot(request).await.unwrap();

    assert_eq!(response.status(), 200, "Chunked body must be assembled and forwarded");
    let body_bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    let body: serde_json::Value = serde_json::from_slice(&body_bytes).unwrap();
    assert_eq!(body["id"], "msg_chunked");
    // Mock assertion: if this passes, the exact expected_body arrived at the provider.
    ai_mock.assert_async().await;
}

/// When the HTTP client sends the same header name twice, axum's `HeaderMap`
/// stores both values.  The proxy serialises headers into
/// `Vec<(String, String)>` which preserves all values — both `x-custom`
/// entries appear in the JetStream message in the order they were sent.
#[tokio::test]
async fn e2e_duplicate_request_headers_both_values_forwarded() {
    use futures_util::StreamExt as _;
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

    let js_stream = jetstream.get_stream(&stream::stream_name("trogon")).await.unwrap();
    let consumer = js_stream
        .get_or_create_consumer(
            "e2e-dupheader-worker",
            async_nats::jetstream::consumer::pull::Config {
                durable_name: Some("e2e-dupheader-worker".to_string()),
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

    // Build a request with the same header name twice.
    let mut req_builder = axum::http::Request::builder()
        .method("POST")
        .uri("/anthropic/v1/messages")
        .header("authorization", "Bearer tok_anthropic_test_duphdr1")
        .header("x-custom", "first")
        .header("x-custom", "second");
    req_builder = req_builder.header("content-type", "application/json");
    let request = req_builder.body(axum::body::Body::from("{}")).unwrap();

    let proxy_handle = tokio::spawn(async move { router(state).oneshot(request).await.unwrap() });

    let msg = tokio::time::timeout(Duration::from_secs(5), messages.next())
        .await
        .expect("Timed out waiting for JetStream message")
        .unwrap()
        .unwrap();

    let parsed: serde_json::Value = serde_json::from_slice(&msg.payload).unwrap();
    let x_custom_values = all_header_values(&parsed["headers"], "x-custom");

    // Vec preserves all values — both "first" and "second" must be present.
    assert!(
        x_custom_values.contains(&"first".to_string()),
        "x-custom: first must be forwarded, got: {:?}",
        x_custom_values
    );
    assert!(
        x_custom_values.contains(&"second".to_string()),
        "x-custom: second must be forwarded, got: {:?}",
        x_custom_values
    );

    let reply_to = parsed["reply_to"].as_str().unwrap().to_string();
    let reply = trogon_secret_proxy::messages::OutboundHttpResponse {
        status: 200,
        headers: vec![],
        body: b"ok".to_vec(),
        error: None,
    };
    nats.publish(reply_to, serde_json::to_vec(&reply).unwrap().into())
        .await
        .unwrap();
    msg.ack().await.unwrap();

    let resp = proxy_handle.await.unwrap();
    assert_eq!(resp.status(), 200);
}

/// A request header whose value contains non-UTF-8 bytes is silently dropped
/// by the proxy.  The proxy serialises headers with `v.to_str().ok()` — if the
/// value is not valid UTF-8 the `filter_map` returns `None` and the header
/// is omitted from the JetStream message.
#[tokio::test]
async fn e2e_non_utf8_request_header_silently_dropped() {
    use futures_util::StreamExt as _;
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

    let js_stream = jetstream.get_stream(&stream::stream_name("trogon")).await.unwrap();
    let consumer = js_stream
        .get_or_create_consumer(
            "e2e-nonutf8-worker",
            async_nats::jetstream::consumer::pull::Config {
                durable_name: Some("e2e-nonutf8-worker".to_string()),
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

    // Build a request with a header value that is valid bytes but invalid UTF-8.
    // 0xFF is never valid in UTF-8.
    let bad_value = axum::http::HeaderValue::from_bytes(&[0xFF, 0xFE]).unwrap();
    let mut headers = axum::http::HeaderMap::new();
    headers.insert(
        axum::http::HeaderName::from_static("authorization"),
        axum::http::HeaderValue::from_static("Bearer tok_anthropic_test_nonutf1"),
    );
    headers.insert(
        axum::http::HeaderName::from_static("x-binary"),
        bad_value,
    );
    headers.insert(
        axum::http::HeaderName::from_static("x-good"),
        axum::http::HeaderValue::from_static("present"),
    );

    let mut request = axum::http::Request::builder()
        .method("POST")
        .uri("/anthropic/v1/messages")
        .body(axum::body::Body::from("{}"))
        .unwrap();
    *request.headers_mut() = headers;

    let proxy_handle = tokio::spawn(async move { router(state).oneshot(request).await.unwrap() });

    let msg = tokio::time::timeout(Duration::from_secs(5), messages.next())
        .await
        .expect("Timed out waiting for JetStream message")
        .unwrap()
        .unwrap();

    let parsed: serde_json::Value = serde_json::from_slice(&msg.payload).unwrap();
    let msg_headers = &parsed["headers"];

    assert!(
        find_header(msg_headers, "x-binary").is_none(),
        "non-UTF-8 header must be silently dropped from the JetStream message"
    );
    assert_eq!(
        find_header(msg_headers, "x-good"),
        Some("present"),
        "valid UTF-8 header must still be forwarded"
    );

    let reply_to = parsed["reply_to"].as_str().unwrap().to_string();
    let reply = trogon_secret_proxy::messages::OutboundHttpResponse {
        status: 200,
        headers: vec![],
        body: b"ok".to_vec(),
        error: None,
    };
    nats.publish(reply_to, serde_json::to_vec(&reply).unwrap().into())
        .await
        .unwrap();
    msg.ack().await.unwrap();

    let resp = proxy_handle.await.unwrap();
    assert_eq!(resp.status(), 200);
}

/// A request header with a very large value (10 KB) must arrive intact in the
/// JetStream message.  The proxy does not truncate header values; it
/// serialises them as-is into the JSON payload.
#[tokio::test]
async fn e2e_large_request_header_value_forwarded() {
    use futures_util::StreamExt as _;
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

    let js_stream = jetstream.get_stream(&stream::stream_name("trogon")).await.unwrap();
    let consumer = js_stream
        .get_or_create_consumer(
            "e2e-largehdr-worker",
            async_nats::jetstream::consumer::pull::Config {
                durable_name: Some("e2e-largehdr-worker".to_string()),
                ack_policy: async_nats::jetstream::consumer::AckPolicy::Explicit,
                ..Default::default()
            },
        )
        .await
        .unwrap();
    let mut messages = consumer.messages().await.unwrap();

    // 10 KB of ASCII 'a' characters.
    let large_value: String = "a".repeat(10_240);

    let state = ProxyState {
        nats: nats.clone(),
        jetstream,
        prefix: "trogon".to_string(),
        outbound_subject,
        worker_timeout: Duration::from_secs(10),
        base_url_override: Some("http://127.0.0.1:1".to_string()),
    };

    let request = axum::http::Request::builder()
        .method("POST")
        .uri("/anthropic/v1/messages")
        .header("authorization", "Bearer tok_anthropic_test_largehd1")
        .header("x-large-header", large_value.as_str())
        .body(axum::body::Body::from("{}"))
        .unwrap();

    let proxy_handle = tokio::spawn(async move { router(state).oneshot(request).await.unwrap() });

    let msg = tokio::time::timeout(Duration::from_secs(5), messages.next())
        .await
        .expect("Timed out waiting for JetStream message")
        .unwrap()
        .unwrap();

    let parsed: serde_json::Value = serde_json::from_slice(&msg.payload).unwrap();
    let received_value = find_header(&parsed["headers"], "x-large-header").unwrap_or("");

    assert_eq!(
        received_value.len(),
        10_240,
        "Large header value must arrive with its full length intact"
    );
    assert!(
        received_value.chars().all(|c| c == 'a'),
        "Large header value content must be unchanged"
    );

    let reply_to = parsed["reply_to"].as_str().unwrap().to_string();
    let reply = trogon_secret_proxy::messages::OutboundHttpResponse {
        status: 200,
        headers: vec![],
        body: b"ok".to_vec(),
        error: None,
    };
    nats.publish(reply_to, serde_json::to_vec(&reply).unwrap().into())
        .await
        .unwrap();
    msg.ack().await.unwrap();

    let resp = proxy_handle.await.unwrap();
    assert_eq!(resp.status(), 200);
}

/// When the upstream AI provider returns a response header whose value contains
/// bytes that are not valid UTF-8 (obs-text: 0xFF 0xFE), the worker must
/// silently drop that header rather than crashing or returning an error.
///
/// The filtering happens in `worker.rs`:
///   `.filter_map(|(k, v)| v.to_str().ok().map(...))`
/// `HeaderValue::to_str()` returns `Err` for any byte outside visible ASCII,
/// so the `filter_map` discards the header without propagating an error.
///
/// A raw TCP server is used because httpmock validates header values as UTF-8.
#[tokio::test]
async fn e2e_non_utf8_upstream_response_header_silently_dropped() {
    use futures_util::StreamExt as _;
    use trogon_secret_proxy::messages::{OutboundHttpRequest, OutboundHttpResponse};
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    let (_nats_container, nats_port) = start_nats().await;

    // Raw TCP server: responds with a header value containing 0xFF 0xFE bytes.
    // These are valid HTTP/1.1 obs-text (RFC 7230 §3.2.6) so hyper accepts them,
    // but they are not valid UTF-8 so `to_str()` fails and we drop the header.
    let raw_listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let raw_port = raw_listener.local_addr().unwrap().port();

    tokio::spawn(async move {
        if let Ok((mut socket, _)) = raw_listener.accept().await {
            let mut buf = vec![0u8; 4096];
            let _ = socket.read(&mut buf).await;

            let mut response: Vec<u8> = Vec::new();
            response.extend_from_slice(b"HTTP/1.1 200 OK\r\n");
            response.extend_from_slice(b"content-type: application/json\r\n");
            // 0xFF 0xFE — valid obs-text in HTTP/1.1, but NOT valid UTF-8.
            response.extend_from_slice(b"x-bad-header: ");
            response.extend_from_slice(&[0xFF, 0xFE]);
            response.extend_from_slice(b"\r\n");
            response.extend_from_slice(b"content-length: 2\r\n");
            response.extend_from_slice(b"\r\n");
            response.extend_from_slice(b"ok");

            let _ = socket.write_all(&response).await;
        }
    });

    let vault = Arc::new(MemoryVault::new());
    let token = ApiKeyToken::new("tok_anthropic_test_nonutf01").unwrap();
    vault.store(&token, "sk-ant-realkey").await.unwrap();

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

    let vault_clone = vault.clone();
    let js_clone = jetstream.clone();
    let nats_clone = nats.clone();
    tokio::spawn(async move {
        worker::run(
            js_clone,
            nats_clone,
            vault_clone,
            reqwest::Client::new(),
            "e2e-nonutf8-worker",
            &stream::stream_name("trogon"),
        )
        .await
        .expect("Worker error");
    });
    tokio::time::sleep(Duration::from_millis(200)).await;

    let reply_subject = subjects::reply("trogon", "nonutf8-corr-001");
    let mut reply_sub = nats.subscribe(reply_subject.clone()).await.unwrap();

    // Inject the JetStream message directly — no need to go through the HTTP proxy.
    let message = OutboundHttpRequest {
        method: "GET".to_string(),
        url: format!("http://127.0.0.1:{}/v1/models", raw_port),
        headers: vec![(
            "Authorization".to_string(),
            "Bearer tok_anthropic_test_nonutf01".to_string(),
        )],
        body: vec![],
        reply_to: reply_subject.clone(),
        idempotency_key: "nonutf8-corr-001".to_string(),
    };

    jetstream
        .publish(outbound_subject, serde_json::to_vec(&message).unwrap().into())
        .await
        .unwrap();

    let reply = tokio::time::timeout(Duration::from_secs(10), reply_sub.next())
        .await
        .expect("Timed out waiting for worker reply")
        .unwrap();

    let response: OutboundHttpResponse = serde_json::from_slice(&reply.payload).unwrap();

    assert!(
        response.error.is_none(),
        "Valid HTTP response must not produce an error, got: {:?}",
        response.error
    );
    assert_eq!(response.status, 200);

    // The header with non-UTF-8 bytes must have been silently dropped.
    let has_bad_header = response.headers.iter().any(|(k, _)| k == "x-bad-header");
    assert!(
        !has_bad_header,
        "x-bad-header with non-UTF-8 value must be silently dropped, headers: {:?}",
        response.headers
    );

    // The valid content-type header must have survived.
    let has_content_type = response.headers.iter().any(|(k, _)| k == "content-type");
    assert!(
        has_content_type,
        "Valid content-type header must be preserved, headers: {:?}",
        response.headers
    );
}

/// When the worker receives a JetStream message whose `method` field is not a
/// valid HTTP method token (RFC 7230 §3.1.1 defines a method as `1*tchar`;
/// spaces are not valid tchar), `reqwest` rejects the request before it is
/// even sent.  After exhausting all retries (`HTTP_MAX_RETRIES=3`, ~700 ms
/// total) the worker must publish an `OutboundHttpResponse` with
/// `error: Some(...)` rather than panicking or hanging.
#[tokio::test]
async fn e2e_invalid_http_method_in_outbound_request_returns_worker_error() {
    use futures_util::StreamExt as _;
    use trogon_secret_proxy::messages::{OutboundHttpRequest, OutboundHttpResponse};

    let (_nats_container, nats_port) = start_nats().await;

    let vault = Arc::new(MemoryVault::new());
    let token = ApiKeyToken::new("tok_anthropic_test_invmth01").unwrap();
    vault.store(&token, "sk-ant-realkey").await.unwrap();

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

    let vault_clone = vault.clone();
    let js_clone = jetstream.clone();
    let nats_clone = nats.clone();
    tokio::spawn(async move {
        worker::run(
            js_clone,
            nats_clone,
            vault_clone,
            reqwest::Client::new(),
            "e2e-invmth-worker",
            &stream::stream_name("trogon"),
        )
        .await
        .expect("Worker error");
    });
    tokio::time::sleep(Duration::from_millis(200)).await;

    let reply_subject = subjects::reply("trogon", "invmth-corr-001");
    let mut reply_sub = nats.subscribe(reply_subject.clone()).await.unwrap();

    // "NOT VALID METHOD" contains spaces which are not valid tchar — reqwest
    // rejects this before sending any bytes to the network.
    let message = OutboundHttpRequest {
        method: "NOT VALID METHOD".to_string(),
        url: "http://127.0.0.1:1/v1/messages".to_string(), // unreachable — method fails first
        headers: vec![(
            "Authorization".to_string(),
            "Bearer tok_anthropic_test_invmth01".to_string(),
        )],
        body: vec![],
        reply_to: reply_subject.clone(),
        idempotency_key: "invmth-corr-001".to_string(),
    };

    jetstream
        .publish(outbound_subject, serde_json::to_vec(&message).unwrap().into())
        .await
        .unwrap();

    // The worker retries HTTP_MAX_RETRIES=3 times before giving up (~700 ms total).
    let reply = tokio::time::timeout(Duration::from_secs(15), reply_sub.next())
        .await
        .expect("Timed out waiting for worker error reply")
        .unwrap();

    let response: OutboundHttpResponse = serde_json::from_slice(&reply.payload).unwrap();

    assert!(
        response.error.is_some(),
        "Worker must report an error for an invalid HTTP method"
    );
    assert!(
        response
            .error
            .as_deref()
            .unwrap()
            .contains("Invalid HTTP method"),
        "Error must describe the invalid method, got: {:?}",
        response.error
    );
}

/// When an intercepting "worker" publishes raw non-JSON bytes to the Core NATS
/// reply subject, the proxy fails `serde_json::from_slice` and returns 500.
///
/// This exercises `ProxyError::Deserialize` at the library level — distinct
/// from `binary_malformed_worker_reply_returns_500` which uses the binary worker.
#[tokio::test]
async fn e2e_corrupted_worker_reply_returns_500() {
    use futures_util::StreamExt as _;
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

    // Set up a consumer to intercept the JetStream message.
    let js_stream = jetstream
        .get_stream(&stream::stream_name("trogon"))
        .await
        .unwrap();
    let consumer = js_stream
        .get_or_create_consumer(
            "e2e-corrupt-reply-consumer",
            async_nats::jetstream::consumer::pull::Config {
                durable_name: Some("e2e-corrupt-reply-consumer".to_string()),
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

    let request = axum::http::Request::builder()
        .method("POST")
        .uri("/anthropic/v1/messages")
        .header("authorization", "Bearer tok_anthropic_test_corruptreply")
        .body(axum::body::Body::from("{}"))
        .unwrap();

    let proxy_handle =
        tokio::spawn(async move { router(state).oneshot(request).await.unwrap() });

    // Intercept the JetStream message and publish garbage to the reply subject.
    let msg = tokio::time::timeout(Duration::from_secs(5), messages.next())
        .await
        .expect("Timed out waiting for JetStream message")
        .unwrap()
        .unwrap();

    let parsed: serde_json::Value = serde_json::from_slice(&msg.payload).unwrap();
    let reply_to = parsed["reply_to"].as_str().unwrap().to_string();

    // Publish bytes that cannot be deserialized as OutboundHttpResponse.
    nats.publish(reply_to, b"this is definitely not json !!!".to_vec().into())
        .await
        .unwrap();
    msg.ack().await.unwrap();

    let resp = proxy_handle.await.unwrap();
    assert_eq!(
        resp.status(),
        axum::http::StatusCode::INTERNAL_SERVER_ERROR,
        "Corrupted worker reply must produce HTTP 500"
    );
}

/// When a request arrives at `/{provider}/` (provider with trailing slash,
/// empty wildcard path), the proxy must return 400 Bad Request immediately
/// without publishing to JetStream or timing out.
///
/// If axum routes to the handler (path=""), the handler catches it and returns
/// 400 before touching NATS.  If axum itself finds no matching route it returns
/// 404.  Either way the caller must not wait 60 s for a worker that will never
/// succeed.
#[tokio::test]
async fn e2e_empty_path_after_provider_slash_handled_gracefully() {
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
        nats: nats.clone(),
        jetstream,
        prefix: "trogon".to_string(),
        outbound_subject,
        // Short timeout so the test finishes quickly when the route matches.
        worker_timeout: Duration::from_millis(500),
        base_url_override: Some("http://127.0.0.1:1".to_string()),
    };

    // /anthropic/ — trailing slash, no path after provider.
    let request = axum::http::Request::builder()
        .method("GET")
        .uri("/anthropic/")
        .header("authorization", "Bearer tok_anthropic_test_emptypath1")
        .body(axum::body::Body::empty())
        .unwrap();

    let resp = router(state).oneshot(request).await.unwrap();
    let status = resp.status().as_u16();

    // Acceptable outcomes:
    //   400 — axum routes to the handler (path=""), handler returns Bad Request.
    //   404 — axum finds no matching route for an empty catch-all segment.
    // Either is acceptable; the important thing is no 504 timeout.
    assert!(
        [400, 404].contains(&status),
        "Empty path after provider/ must yield 400 (bad request) or 404 (no route), got {}",
        status
    );
}

/// When `base_url_override` ends with a trailing slash the proxy must strip it
/// before constructing the forwarded URL so no double slash appears.
///
/// e.g. `base_url_override = "http://mock/"` + path `"v1/messages"` must
/// produce `"http://mock/v1/messages"`, not `"http://mock//v1/messages"`.
#[tokio::test]
async fn e2e_trailing_slash_in_base_url_override_is_normalized() {
    use futures_util::StreamExt as _;
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

    let js_stream = jetstream
        .get_stream(&stream::stream_name("trogon"))
        .await
        .unwrap();
    let consumer = js_stream
        .get_or_create_consumer(
            "e2e-trailing-slash-consumer",
            async_nats::jetstream::consumer::pull::Config {
                durable_name: Some("e2e-trailing-slash-consumer".to_string()),
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
        worker_timeout: Duration::from_secs(5),
        // Trailing slash → URL will contain double slash.
        base_url_override: Some("http://127.0.0.1:1/".to_string()),
    };

    let request = axum::http::Request::builder()
        .method("POST")
        .uri("/anthropic/v1/messages")
        .header("authorization", "Bearer tok_anthropic_test_trailslash1")
        .body(axum::body::Body::from("{}"))
        .unwrap();

    let proxy_handle =
        tokio::spawn(async move { router(state).oneshot(request).await.unwrap() });

    let msg = tokio::time::timeout(Duration::from_secs(5), messages.next())
        .await
        .expect("Timed out waiting for JetStream message")
        .unwrap()
        .unwrap();

    let parsed: serde_json::Value = serde_json::from_slice(&msg.payload).unwrap();
    let url = parsed["url"].as_str().unwrap();

    // Trailing slash in base_url_override must be stripped; no double slash.
    assert!(
        !url.contains("//v1/messages"),
        "Trailing slash in base_url_override must NOT produce double slash in URL, got: {}",
        url
    );
    assert!(
        url.contains("/v1/messages"),
        "URL must contain /v1/messages, got: {}",
        url
    );

    // Reply so the proxy resolves cleanly.
    let reply_to = parsed["reply_to"].as_str().unwrap().to_string();
    let reply = trogon_secret_proxy::messages::OutboundHttpResponse {
        status: 200,
        headers: vec![],
        body: vec![],
        error: None,
    };
    nats.publish(reply_to, serde_json::to_vec(&reply).unwrap().into())
        .await
        .unwrap();
    msg.ack().await.unwrap();

    proxy_handle.await.unwrap();
}

// ── Gap 1: Stream config ──────────────────────────────────────────────────────

/// `ensure_stream` must create the stream with WorkQueue retention and Memory
/// storage.  If either is wrong the proxy silently misbehaves: File storage
/// would write to disk, and the wrong retention policy would keep or lose
/// messages unexpectedly.
#[tokio::test]
async fn e2e_stream_config_has_work_queue_retention_and_memory_storage() {
    let (_nats_container, nats_port) = start_nats().await;

    let nats_config = NatsConfig {
        servers: vec![format!("localhost:{}", nats_port)],
        auth: NatsAuth::None,
    };
    let nats = connect(&nats_config, Duration::from_secs(10)).await.unwrap();
    let jetstream = Arc::new(async_nats::jetstream::new(nats));
    let outbound_subject = subjects::outbound("trogon");

    stream::ensure_stream(&jetstream, "trogon", &outbound_subject)
        .await
        .unwrap();

    let js_stream = jetstream
        .get_stream(&stream::stream_name("trogon"))
        .await
        .unwrap();

    let info = js_stream.cached_info();

    assert_eq!(
        info.config.retention,
        async_nats::jetstream::stream::RetentionPolicy::WorkQueue,
        "Stream must use WorkQueue retention so messages are deleted after ack"
    );
    assert_eq!(
        info.config.storage,
        async_nats::jetstream::stream::StorageType::Memory,
        "Stream must use Memory storage for low-latency delivery"
    );
}

// ── Gap 2: Provider name case sensitivity ────────────────────────────────────

/// Provider names are matched case-sensitively.  A request to `/ANTHROPIC/…`
/// must fail with 502 (unknown provider) rather than routing to Anthropic.
///
/// This prevents accidental routing: clients must use the canonical lowercase
/// provider name to get the correct base URL.
#[tokio::test]
async fn e2e_uppercase_provider_name_returns_502() {
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
        base_url_override: None,
    };

    let request = axum::http::Request::builder()
        .method("POST")
        .uri("/ANTHROPIC/v1/messages")
        .header("authorization", "Bearer tok_anthropic_prod_casetest1")
        .body(axum::body::Body::from("{}"))
        .unwrap();

    let resp = router(state).oneshot(request).await.unwrap();

    assert_eq!(
        resp.status(),
        axum::http::StatusCode::BAD_GATEWAY,
        "Uppercase provider name must return 502 (unknown provider)"
    );
}

// ── Gap 4: JetStream publish failure ─────────────────────────────────────────

/// When the outbound subject is not covered by any JetStream stream, the proxy
/// now awaits the `PubAckFuture` and receives a JetStream error immediately.
/// This returns 500 Internal Server Error without waiting for the worker timeout,
/// providing a fast and explicit failure instead of a silent 60-second hang.
#[tokio::test]
async fn e2e_outbound_subject_with_no_stream_returns_500_immediately() {
    use tower::ServiceExt as _;

    let (_nats_container, nats_port) = start_nats().await;

    let nats_config = NatsConfig {
        servers: vec![format!("localhost:{}", nats_port)],
        auth: NatsAuth::None,
    };
    let nats = connect(&nats_config, Duration::from_secs(10)).await.unwrap();
    let jetstream = Arc::new(async_nats::jetstream::new(nats.clone()));
    // Intentionally do NOT call ensure_stream — no stream covers this subject.

    let state = ProxyState {
        nats,
        jetstream,
        prefix: "trogon".to_string(),
        outbound_subject: "trogon.proxy.http.no-stream-here".to_string(),
        worker_timeout: Duration::from_secs(30), // long timeout to prove we don't wait
        base_url_override: Some("http://127.0.0.1:1".to_string()),
    };

    let request = axum::http::Request::builder()
        .method("POST")
        .uri("/anthropic/v1/messages")
        .header("authorization", "Bearer tok_anthropic_prod_nostreamx1")
        .body(axum::body::Body::from("{}"))
        .unwrap();

    let resp = router(state).oneshot(request).await.unwrap();

    // JetStream ACK fails immediately (no stream) → 500, not a 30s timeout.
    assert_eq!(
        resp.status(),
        axum::http::StatusCode::INTERNAL_SERVER_ERROR,
        "Missing JetStream stream must return 500 immediately, not wait for timeout"
    );
}

// ── Gap 6: Worker stream not found ───────────────────────────────────────────

/// `worker::run` must return `Err(WorkerError::JetStream)` immediately when
/// the target stream does not exist, rather than panicking or looping forever.
#[tokio::test]
async fn e2e_worker_run_returns_error_when_stream_does_not_exist() {
    let (_nats_container, nats_port) = start_nats().await;

    let nats_config = NatsConfig {
        servers: vec![format!("localhost:{}", nats_port)],
        auth: NatsAuth::None,
    };
    let nats = connect(&nats_config, Duration::from_secs(10)).await.unwrap();
    let jetstream = Arc::new(async_nats::jetstream::new(nats.clone()));
    let vault = Arc::new(MemoryVault::new());
    let http_client = reqwest::Client::new();

    // "ghost" prefix — stream was never created.
    let result = worker::run(
        jetstream,
        nats,
        vault,
        http_client,
        "test-consumer-ghost",
        &stream::stream_name("ghost"),
    )
    .await;

    assert!(
        result.is_err(),
        "worker::run must return Err when the JetStream stream does not exist"
    );
}

// ── Duplicate Authorization headers ──────────────────────────────────────────

/// When an `OutboundHttpRequest` arrives with two `Authorization` headers,
/// `resolve_token` picks the **first** one via `.find()` and ignores the rest.
///
/// - First  `Authorization`: a valid `tok_...` token stored in the vault.
/// - Second `Authorization`: a different `tok_...` token NOT in the vault.
///
/// If the second header were used instead, the vault lookup would fail and the
/// worker would return an error response.  A 200 reply confirms the first
/// header was used and the real key reached the AI provider.
///
/// This documents — and locks in — the tie-breaking rule for duplicate
/// Authorization headers forwarded through the proxy pipeline.
#[tokio::test]
async fn e2e_duplicate_authorization_headers_first_value_used() {
    use futures_util::StreamExt as _;
    use trogon_secret_proxy::messages::{OutboundHttpRequest, OutboundHttpResponse};

    let (_nats_container, nats_port) = start_nats().await;

    let vault = Arc::new(MemoryVault::new());
    // Only the FIRST token is registered; the second would fail vault lookup.
    let first_token = ApiKeyToken::new("tok_anthropic_test_dup1st1").unwrap();
    vault.store(&first_token, "sk-ant-dup-firstkey").await.unwrap();

    let mock_server = httpmock::MockServer::start_async().await;
    let ai_mock = mock_server
        .mock_async(|when, then| {
            when.method(httpmock::Method::GET)
                .path("/v1/models")
                .header("authorization", "Bearer sk-ant-dup-firstkey");
            then.status(200)
                .header("content-type", "application/json")
                .body(r#"{"models":[]}"#);
        })
        .await;

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

    let vault_clone = vault.clone();
    let js_clone = jetstream.clone();
    let nats_clone = nats.clone();
    tokio::spawn(async move {
        worker::run(
            js_clone,
            nats_clone,
            vault_clone,
            reqwest::Client::new(),
            "e2e-dupauth-worker",
            &stream::stream_name("trogon"),
        )
        .await
        .expect("Worker error");
    });
    tokio::time::sleep(Duration::from_millis(200)).await;

    let reply_subject = subjects::reply("trogon", "dupauth-corr-001");
    let mut reply_sub = nats.subscribe(reply_subject.clone()).await.unwrap();

    // Two Authorization headers: first resolves, second does NOT exist in vault.
    let message = OutboundHttpRequest {
        method: "GET".to_string(),
        url: format!("{}/v1/models", mock_server.base_url()),
        headers: vec![
            (
                "Authorization".to_string(),
                "Bearer tok_anthropic_test_dup1st1".to_string(),
            ),
            (
                "Authorization".to_string(),
                "Bearer tok_anthropic_test_dup2nd1".to_string(), // NOT in vault
            ),
        ],
        body: vec![],
        reply_to: reply_subject.clone(),
        idempotency_key: "dupauth-corr-001".to_string(),
    };

    jetstream
        .publish(outbound_subject, serde_json::to_vec(&message).unwrap().into())
        .await
        .unwrap();

    let reply = tokio::time::timeout(Duration::from_secs(10), reply_sub.next())
        .await
        .expect("Timed out waiting for worker reply")
        .unwrap();

    let response: OutboundHttpResponse = serde_json::from_slice(&reply.payload).unwrap();

    assert!(
        response.error.is_none(),
        "First Authorization header must resolve successfully; got error: {:?}",
        response.error
    );
    assert_eq!(
        response.status, 200,
        "First Authorization header's real key must be accepted by the AI provider"
    );
    ai_mock.assert_async().await;
}

// ── Gap 13: transfer-encoding is hop-by-hop ───────────────────────────────────

/// `Transfer-Encoding: chunked` must be stripped from the outbound JetStream
/// message by the proxy.  It is a hop-by-hop header (RFC 7230 §6.1) and must
/// not be forwarded to the AI provider.
///
/// This exercises the `"transfer-encoding"` entry in the `HOP_BY_HOP` list
/// in `proxy.rs` specifically (other hop-by-hop headers are tested in
/// `e2e_hop_by_hop_headers_stripped_from_jetstream_message`).
#[tokio::test]
async fn e2e_transfer_encoding_header_stripped_from_outbound_request() {
    use futures_util::StreamExt as _;
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

    // Intercept consumer — reads the raw JetStream message and checks headers.
    let js_stream = jetstream.get_stream(&stream::stream_name("trogon")).await.unwrap();
    let consumer = js_stream
        .get_or_create_consumer(
            "e2e-transfer-enc-consumer",
            async_nats::jetstream::consumer::pull::Config {
                durable_name: Some("e2e-transfer-enc-consumer".to_string()),
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

    let request = axum::http::Request::builder()
        .method("POST")
        .uri("/anthropic/v1/messages")
        .header("authorization", "Bearer tok_anthropic_test_transenc1")
        .header("transfer-encoding", "chunked")
        .header("content-type", "application/json")
        .body(axum::body::Body::from("{}"))
        .unwrap();

    let proxy_handle = tokio::spawn(async move { router(state).oneshot(request).await.unwrap() });

    let msg = tokio::time::timeout(Duration::from_secs(5), messages.next())
        .await
        .expect("Timed out waiting for JetStream message")
        .unwrap()
        .unwrap();

    let parsed: serde_json::Value = serde_json::from_slice(&msg.payload).unwrap();
    let headers_in_message = &parsed["headers"];

    // transfer-encoding is hop-by-hop — must NOT appear in the JetStream message.
    assert!(
        find_header(headers_in_message, "transfer-encoding").is_none(),
        "transfer-encoding must be stripped from the JetStream message (it is hop-by-hop)"
    );
    // End-to-end headers must still be forwarded.
    assert_eq!(
        find_header(headers_in_message, "content-type"),
        Some("application/json"),
        "content-type (end-to-end) must be forwarded to the AI provider"
    );

    // Reply so the proxy handle can complete cleanly.
    let reply_to = parsed["reply_to"].as_str().unwrap().to_string();
    let reply = trogon_secret_proxy::messages::OutboundHttpResponse {
        status: 200,
        headers: vec![],
        body: b"ok".to_vec(),
        error: None,
    };
    nats.publish(reply_to, serde_json::to_vec(&reply).unwrap().into())
        .await
        .unwrap();
    msg.ack().await.unwrap();

    let resp = proxy_handle.await.unwrap();
    assert_eq!(resp.status(), 200);
}

// ── Gap 11: worker publishes reply to wrong subject ───────────────────────────

/// When a worker publishes its reply to a DIFFERENT Core NATS subject than
/// the one the proxy subscribed to, the proxy never receives the reply and
/// times out after `worker_timeout` with HTTP 504 Gateway Timeout.
///
/// In production this would happen if a buggy worker corrupted the `reply_to`
/// field.  The proxy must not hang indefinitely.
#[tokio::test]
async fn e2e_worker_reply_to_wrong_subject_causes_proxy_timeout() {
    use futures_util::StreamExt as _;
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

    // Intercept consumer plays the role of a misbehaving worker.
    let js_stream = jetstream.get_stream(&stream::stream_name("trogon")).await.unwrap();
    let consumer = js_stream
        .get_or_create_consumer(
            "e2e-wrongreply-consumer",
            async_nats::jetstream::consumer::pull::Config {
                durable_name: Some("e2e-wrongreply-consumer".to_string()),
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
        // Short timeout so the test finishes quickly.
        worker_timeout: Duration::from_millis(500),
        base_url_override: Some("http://127.0.0.1:1".to_string()),
    };

    let request = axum::http::Request::builder()
        .method("POST")
        .uri("/anthropic/v1/messages")
        .header("authorization", "Bearer tok_anthropic_test_wrongrpl1")
        .body(axum::body::Body::from("{}"))
        .unwrap();

    let proxy_handle = tokio::spawn(async move { router(state).oneshot(request).await.unwrap() });

    let msg = tokio::time::timeout(Duration::from_secs(5), messages.next())
        .await
        .expect("Timed out waiting for JetStream message")
        .unwrap()
        .unwrap();

    let _parsed: serde_json::Value = serde_json::from_slice(&msg.payload).unwrap();

    // Deliberately publish to a DIFFERENT subject than the proxy's reply_to.
    let wrong_subject = "trogon.proxy.reply.this-is-the-wrong-subject-xyz";
    let reply = trogon_secret_proxy::messages::OutboundHttpResponse {
        status: 200,
        headers: vec![],
        body: b"ok".to_vec(),
        error: None,
    };
    nats.publish(wrong_subject, serde_json::to_vec(&reply).unwrap().into())
        .await
        .unwrap();
    msg.ack().await.unwrap();

    // Proxy must time out (500 ms) and return 504 Gateway Timeout.
    let resp = proxy_handle.await.unwrap();
    assert_eq!(
        resp.status(),
        axum::http::StatusCode::GATEWAY_TIMEOUT,
        "A misdirected reply must cause the proxy to time out with 504"
    );
}

// ── Gap 18: empty reply_to field ─────────────────────────────────────────────

/// When an `OutboundHttpRequest` has `reply_to = ""`, the worker must:
///   1. Still resolve the token and forward the request to the AI provider.
///   2. Attempt `nats.publish("", payload)` — which fails (empty subject is
///      invalid in NATS) — log the error silently, and fall through to ack.
///   3. Acknowledge the JetStream message so it is not redelivered.
///   4. Continue processing subsequent valid messages (worker must not crash).
///
/// This verifies that the worker's error path is resilient: a corrupt
/// `reply_to` does not cause the worker to panic or enter an error loop.
#[tokio::test]
async fn e2e_empty_reply_to_field_worker_handles_gracefully() {
    use futures_util::StreamExt as _;
    use trogon_secret_proxy::messages::{OutboundHttpRequest, OutboundHttpResponse};

    let (_nats_container, nats_port) = start_nats().await;

    let mock_server = httpmock::MockServer::start_async().await;
    let ai_mock = mock_server
        .mock_async(|when, then| {
            when.any_request();
            then.status(200)
                .header("content-type", "application/json")
                .body(r#"{"id":"msg_emptyrplyto"}"#);
        })
        .await;

    let vault = Arc::new(MemoryVault::new());
    let token = ApiKeyToken::new("tok_anthropic_test_emprep001").unwrap();
    vault.store(&token, "sk-ant-emprep-key").await.unwrap();

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

    let vault_clone = vault.clone();
    let js_clone = jetstream.clone();
    let nats_clone = nats.clone();
    tokio::spawn(async move {
        worker::run(
            js_clone,
            nats_clone,
            vault_clone,
            reqwest::Client::new(),
            "e2e-emprep-worker",
            &stream::stream_name("trogon"),
        )
        .await
        .expect("Worker error");
    });
    tokio::time::sleep(Duration::from_millis(200)).await;

    // Message 1: empty reply_to — worker resolves token, calls AI, but cannot
    // publish the reply because "" is an invalid NATS subject.
    let msg1 = OutboundHttpRequest {
        method: "POST".to_string(),
        url: format!("{}/v1/messages", mock_server.base_url()),
        headers: vec![(
            "Authorization".to_string(),
            "Bearer tok_anthropic_test_emprep001".to_string(),
        )],
        body: b"{}".to_vec(),
        reply_to: "".to_string(), // EMPTY — nats.publish will fail
        idempotency_key: "emprep-corr-001".to_string(),
    };
    jetstream
        .publish(outbound_subject.clone(), serde_json::to_vec(&msg1).unwrap().into())
        .await
        .unwrap();

    // Allow the worker to process message 1 (token resolution + AI call + failed publish + ack).
    tokio::time::sleep(Duration::from_secs(1)).await;

    // Message 2: valid reply_to — proves the worker is still alive.
    let reply_subject = subjects::reply("trogon", "emprep-corr-002");
    let mut reply_sub = nats.subscribe(reply_subject.clone()).await.unwrap();

    let msg2 = OutboundHttpRequest {
        method: "POST".to_string(),
        url: format!("{}/v1/messages", mock_server.base_url()),
        headers: vec![(
            "Authorization".to_string(),
            "Bearer tok_anthropic_test_emprep001".to_string(),
        )],
        body: b"{}".to_vec(),
        reply_to: reply_subject.clone(),
        idempotency_key: "emprep-corr-002".to_string(),
    };
    jetstream
        .publish(outbound_subject, serde_json::to_vec(&msg2).unwrap().into())
        .await
        .unwrap();

    let reply = tokio::time::timeout(Duration::from_secs(10), reply_sub.next())
        .await
        .expect(
            "Worker must still be alive and reply to the second message \
             after the failed first publish",
        )
        .unwrap();

    let response: OutboundHttpResponse = serde_json::from_slice(&reply.payload).unwrap();
    assert!(
        response.error.is_none(),
        "Worker must successfully process the valid second message"
    );

    // AI provider must have been called for both messages.
    assert_eq!(
        ai_mock.hits(),
        2,
        "AI provider must be called for both messages — worker acks after a failed publish"
    );
}

// ── Gap 19: wrong retention policy — messages survive ack ────────────────────

/// When the JetStream stream is pre-created with `RetentionPolicy::Limits`
/// instead of `WorkQueue`, `ensure_stream` returns the existing stream without
/// modifying its retention policy (NATS `get_or_create_stream` is a no-op when
/// the stream already exists).
///
/// Consequence (documented production risk): messages survive acknowledgement
/// and are visible to new consumers with `DeliverPolicy::All`.  A correctly
/// configured WorkQueue stream would delete the message on ack, so a second
/// consumer would receive nothing.
#[tokio::test]
async fn e2e_wrong_retention_policy_messages_survive_ack() {
    use futures_util::StreamExt as _;

    let (_nats_container, nats_port) = start_nats().await;

    let nats_config = NatsConfig {
        servers: vec![format!("localhost:{}", nats_port)],
        auth: NatsAuth::None,
    };
    let nats = connect(&nats_config, Duration::from_secs(10)).await.unwrap();
    let jetstream = Arc::new(async_nats::jetstream::new(nats));

    // Use an isolated prefix so this test does not interfere with the shared
    // "trogon" stream used by other tests.
    let prefix = "wrongret";
    let outbound_subject = subjects::outbound(prefix);
    let sname = stream::stream_name(prefix);

    // Step 1: Pre-create the stream with the WRONG retention policy.
    jetstream
        .get_or_create_stream(async_nats::jetstream::stream::Config {
            name: sname.clone(),
            subjects: vec![outbound_subject.clone()],
            retention: async_nats::jetstream::stream::RetentionPolicy::Limits,
            storage: async_nats::jetstream::stream::StorageType::Memory,
            ..Default::default()
        })
        .await
        .unwrap();

    // Step 2: ensure_stream is a no-op — returns the existing Limits stream.
    stream::ensure_stream(&jetstream, prefix, &outbound_subject)
        .await
        .unwrap();

    // Verify the bug: stream still has Limits retention, NOT WorkQueue.
    let info = jetstream
        .get_stream(&sname)
        .await
        .unwrap()
        .cached_info()
        .clone();
    assert_eq!(
        info.config.retention,
        async_nats::jetstream::stream::RetentionPolicy::Limits,
        "ensure_stream must not modify an existing stream's retention policy"
    );

    // Step 3: Publish a test message.
    jetstream
        .publish(outbound_subject.clone(), b"test-payload".to_vec().into())
        .await
        .unwrap()
        .await
        .unwrap();

    // Consumer A: pull the message and ack it.
    let stream_handle = jetstream.get_stream(&sname).await.unwrap();
    let consumer_a = stream_handle
        .get_or_create_consumer(
            "wrongret-consumer-a",
            async_nats::jetstream::consumer::pull::Config {
                durable_name: Some("wrongret-consumer-a".to_string()),
                ack_policy: async_nats::jetstream::consumer::AckPolicy::Explicit,
                deliver_policy: async_nats::jetstream::consumer::DeliverPolicy::All,
                ..Default::default()
            },
        )
        .await
        .unwrap();
    let mut msgs_a = consumer_a.messages().await.unwrap();
    let msg_a = tokio::time::timeout(Duration::from_secs(5), msgs_a.next())
        .await
        .expect("Consumer A must receive the message")
        .unwrap()
        .unwrap();
    msg_a.ack().await.unwrap();

    // Give NATS time to process the ack.
    tokio::time::sleep(Duration::from_millis(300)).await;

    // Consumer B: brand-new durable with DeliverPolicy::All.
    // With WorkQueue retention the message would be GONE after Consumer A's ack.
    // With Limits retention the message survives — this is the production risk.
    let stream_handle2 = jetstream.get_stream(&sname).await.unwrap();
    let consumer_b = stream_handle2
        .get_or_create_consumer(
            "wrongret-consumer-b",
            async_nats::jetstream::consumer::pull::Config {
                durable_name: Some("wrongret-consumer-b".to_string()),
                ack_policy: async_nats::jetstream::consumer::AckPolicy::Explicit,
                deliver_policy: async_nats::jetstream::consumer::DeliverPolicy::All,
                ..Default::default()
            },
        )
        .await
        .unwrap();
    let mut msgs_b = consumer_b.messages().await.unwrap();
    let msg_b = tokio::time::timeout(Duration::from_secs(3), msgs_b.next())
        .await
        .expect(
            "Consumer B must still see the message — Limits retention does not delete on ack \
             (demonstrates production risk of wrong retention policy)",
        )
        .unwrap()
        .unwrap();

    assert_eq!(
        &*msg_b.payload,
        b"test-payload",
        "Message must survive ack under Limits retention"
    );
    msg_b.ack().await.unwrap();
}

// ── Gap 17: invalid URL in OutboundHttpRequest ────────────────────────────────

/// When an `OutboundHttpRequest` contains an invalid URL, `reqwest` fails
/// immediately (URL parse / builder error), the worker retries
/// `HTTP_MAX_RETRIES` (3) times, and then publishes an error response.
///
/// The caller of the reply subject receives `response.error.is_some()`.
///
/// This exercises the same `Err` branch of `forward_request_with_retry` as a
/// DNS / transport failure, but at the URL-construction layer.
#[tokio::test]
async fn e2e_invalid_url_in_outbound_request_returns_worker_error() {
    use futures_util::StreamExt as _;
    use trogon_secret_proxy::messages::{OutboundHttpRequest, OutboundHttpResponse};

    let (_nats_container, nats_port) = start_nats().await;

    let vault = Arc::new(MemoryVault::new());
    let token = ApiKeyToken::new("tok_anthropic_test_invurl001").unwrap();
    vault.store(&token, "sk-ant-invurl-key").await.unwrap();

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

    let vault_clone = vault.clone();
    let js_clone = jetstream.clone();
    let nats_clone = nats.clone();
    tokio::spawn(async move {
        worker::run(
            js_clone,
            nats_clone,
            vault_clone,
            reqwest::Client::new(),
            "e2e-invurl-worker",
            &stream::stream_name("trogon"),
        )
        .await
        .expect("Worker error");
    });
    tokio::time::sleep(Duration::from_millis(200)).await;

    let reply_subject = subjects::reply("trogon", "invurl-corr-001");
    let mut reply_sub = nats.subscribe(reply_subject.clone()).await.unwrap();

    // URL is syntactically invalid — spaces and special characters make it
    // unparseable by any URL parser.
    let message = OutboundHttpRequest {
        method: "POST".to_string(),
        url: "not a valid url ://##@@!!".to_string(),
        headers: vec![(
            "Authorization".to_string(),
            "Bearer tok_anthropic_test_invurl001".to_string(),
        )],
        body: b"{}".to_vec(),
        reply_to: reply_subject.clone(),
        idempotency_key: "invurl-corr-001".to_string(),
    };

    jetstream
        .publish(outbound_subject, serde_json::to_vec(&message).unwrap().into())
        .await
        .unwrap();

    // Worker retries HTTP_MAX_RETRIES=3 times before giving up (~700 ms total).
    let reply = tokio::time::timeout(Duration::from_secs(15), reply_sub.next())
        .await
        .expect("Timed out waiting for worker error reply")
        .unwrap();

    let response: OutboundHttpResponse = serde_json::from_slice(&reply.payload).unwrap();
    assert!(
        response.error.is_some(),
        "Worker must report an error for an invalid URL"
    );
}

// ── NATS-level Reply-To header injected by proxy ──────────────────────────────

/// The proxy injects `Reply-To` as a **NATS-level header** on the JetStream
/// message (`proxy.rs` line 138: `nats_headers.insert("Reply-To", ...)`).
///
/// This allows consumers outside the standard worker (monitoring, replay
/// tools) to locate the reply channel without parsing the JSON payload.
/// The value must match the `reply_to` field in the payload exactly.
#[tokio::test]
async fn e2e_nats_reply_to_header_injected_in_jetstream_message() {
    use futures_util::StreamExt as _;
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

    // Intercept consumer — reads the raw JetStream message and checks NATS headers.
    let js_stream = jetstream.get_stream(&stream::stream_name("trogon")).await.unwrap();
    let consumer = js_stream
        .get_or_create_consumer(
            "e2e-nats-hdr-consumer",
            async_nats::jetstream::consumer::pull::Config {
                durable_name: Some("e2e-nats-hdr-consumer".to_string()),
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

    let request = axum::http::Request::builder()
        .method("POST")
        .uri("/anthropic/v1/messages")
        .header("authorization", "Bearer tok_anthropic_test_natshdr01")
        .body(axum::body::Body::from("{}"))
        .unwrap();

    let proxy_handle = tokio::spawn(async move { router(state).oneshot(request).await.unwrap() });

    let msg = tokio::time::timeout(Duration::from_secs(5), messages.next())
        .await
        .expect("Timed out waiting for JetStream message")
        .unwrap()
        .unwrap();

    // ── Assert NATS-level headers (not the JSON payload headers) ──────────────
    let nats_headers = msg
        .headers
        .as_ref()
        .expect("JetStream message must carry NATS-level headers");

    let reply_to_nats = nats_headers
        .get("Reply-To")
        .expect("Reply-To NATS header must be injected by the proxy (proxy.rs:138)");

    // Must match the reply_to field inside the JSON payload.
    let parsed: serde_json::Value = serde_json::from_slice(&msg.payload).unwrap();
    let reply_to_payload = parsed["reply_to"].as_str().unwrap();

    assert_eq!(
        reply_to_nats.as_str(),
        reply_to_payload,
        "Reply-To NATS header must equal the reply_to field in the JSON payload"
    );
    assert!(
        reply_to_payload.starts_with("trogon.proxy.reply."),
        "reply_to must be a trogon reply subject, got: {}",
        reply_to_payload
    );

    // Reply so the proxy handle completes cleanly.
    let reply = trogon_secret_proxy::messages::OutboundHttpResponse {
        status: 200,
        headers: vec![],
        body: b"ok".to_vec(),
        error: None,
    };
    nats.publish(reply_to_payload.to_string(), serde_json::to_vec(&reply).unwrap().into())
        .await
        .unwrap();
    msg.ack().await.unwrap();

    let resp = proxy_handle.await.unwrap();
    assert_eq!(resp.status(), 200);
}

// ── Gap 1: 4xx error status preserved ─────────────────────────────────────

/// When the worker sets the `error` field AND the status code is already a
/// 4xx client error, the proxy must return the original 4xx status code —
/// not convert it to 502 Bad Gateway.
///
/// proxy.rs logic:
/// ```text
/// let error_status = if status.is_client_error() || status.is_server_error() {
///     status          // ← 4xx/5xx preserved
/// } else {
///     StatusCode::BAD_GATEWAY   // ← 2xx/3xx with error field → 502
/// };
/// ```
///
/// The complementary test `e2e_worker_error_field_with_status_200_returns_502`
/// covers the 200→502 path; this test covers the 403→403 path.
#[tokio::test]
async fn e2e_worker_error_with_4xx_status_preserves_status_code() {
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

    let js_stream = jetstream
        .get_stream(&stream::stream_name("trogon"))
        .await
        .unwrap();
    let consumer = js_stream
        .get_or_create_consumer(
            "e2e-err-4xx-worker",
            async_nats::jetstream::consumer::pull::Config {
                durable_name: Some("e2e-err-4xx-worker".to_string()),
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

    let request = axum::http::Request::builder()
        .method("POST")
        .uri("/anthropic/v1/messages")
        .header("authorization", "Bearer tok_anthropic_test_err4xx1")
        .body(axum::body::Body::from("{}"))
        .unwrap();

    let proxy_handle = tokio::spawn(async move { router(state).oneshot(request).await.unwrap() });

    let msg = tokio::time::timeout(Duration::from_secs(5), messages.next())
        .await
        .expect("Timed out waiting for proxy to publish")
        .unwrap()
        .unwrap();

    let parsed: serde_json::Value = serde_json::from_slice(&msg.payload).unwrap();
    let reply_to = parsed["reply_to"].as_str().unwrap().to_string();

    // Reply with 403 AND error field set.
    // proxy.rs must preserve 403 (not convert to 502) because 4xx is already
    // an error status code.
    let reply = OutboundHttpResponse {
        status: 403,
        headers: vec![],
        body: b"access forbidden".to_vec(),
        error: Some("Access denied: insufficient permissions".to_string()),
    };
    nats.publish(reply_to, serde_json::to_vec(&reply).unwrap().into())
        .await
        .unwrap();
    msg.ack().await.unwrap();

    let resp = proxy_handle.await.unwrap();
    assert_eq!(
        resp.status(),
        axum::http::StatusCode::FORBIDDEN,
        "error field with 4xx status must preserve 403 — must NOT convert to 502"
    );
    let body_bytes = axum::body::to_bytes(resp.into_body(), usize::MAX)
        .await
        .unwrap();
    assert!(
        std::str::from_utf8(&body_bytes)
            .unwrap()
            .contains("Access denied"),
        "Error message must be forwarded in the response body"
    );
}

// ── Gap 15: hop-by-hop headers stripped ───────────────────────────────────

/// Hop-by-hop headers (`proxy-authenticate`, `te`, `trailers`) must be
/// stripped by the proxy before publishing the `OutboundHttpRequest` to
/// JetStream.  Non-hop-by-hop headers must be preserved.
///
/// The test directly inspects the JSON payload of the JetStream message,
/// which is more reliable than checking the provider's received headers
/// through a binary test (where the test HTTP client might strip some
/// hop-by-hop headers itself before they even reach the proxy).
#[tokio::test]
async fn e2e_hop_by_hop_headers_stripped_in_outbound_request() {
    use futures_util::StreamExt as _;
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

    let js_stream = jetstream
        .get_stream(&stream::stream_name("trogon"))
        .await
        .unwrap();
    let consumer = js_stream
        .get_or_create_consumer(
            "e2e-hbh-worker",
            async_nats::jetstream::consumer::pull::Config {
                durable_name: Some("e2e-hbh-worker".to_string()),
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
        worker_timeout: Duration::from_secs(5),
        base_url_override: Some("http://127.0.0.1:1".to_string()),
    };

    // Include hop-by-hop headers alongside a custom non-hop-by-hop header.
    let request = axum::http::Request::builder()
        .method("POST")
        .uri("/anthropic/v1/messages")
        .header("authorization", "Bearer tok_anthropic_test_hbh01")
        .header("proxy-authenticate", "Basic realm=\"Proxy\"")
        .header("te", "trailers")
        .header("trailers", "X-Checksum")
        .header("x-custom-business-header", "must-be-forwarded")
        .body(axum::body::Body::from("{}"))
        .unwrap();

    let proxy_handle = tokio::spawn(async move { router(state).oneshot(request).await });

    let msg = tokio::time::timeout(Duration::from_secs(5), messages.next())
        .await
        .expect("Timed out waiting for proxy to publish")
        .unwrap()
        .unwrap();

    let parsed: serde_json::Value = serde_json::from_slice(&msg.payload).unwrap();
    let headers_json = &parsed["headers"];

    // Hop-by-hop headers must NOT appear in the OutboundHttpRequest payload.
    let hop_by_hop = ["proxy-authenticate", "te", "trailers"];
    for name in &hop_by_hop {
        assert!(
            find_header(headers_json, name).is_none(),
            "Hop-by-hop header '{}' must be stripped before publishing to JetStream",
            name
        );
    }

    // Non-hop-by-hop headers must be preserved.
    assert!(
        find_header(headers_json, "x-custom-business-header").is_some(),
        "Non-hop-by-hop header 'x-custom-business-header' must be forwarded unchanged"
    );

    // Send a reply to unblock the proxy task and verify it completes cleanly.
    let reply_to = parsed["reply_to"].as_str().unwrap().to_string();
    let reply = trogon_secret_proxy::messages::OutboundHttpResponse {
        status: 200,
        headers: vec![],
        body: b"ok".to_vec(),
        error: None,
    };
    nats.publish(reply_to, serde_json::to_vec(&reply).unwrap().into())
        .await
        .unwrap();
    msg.ack().await.unwrap();

    let resp = proxy_handle.await.unwrap().unwrap();
    assert_eq!(resp.status(), 200, "Proxy must complete the request successfully");
}

/// Multiple `transfer-encoding` headers (sent by e.g. different middleware
/// layers) must ALL be stripped by the proxy — none may appear in the
/// `OutboundHttpRequest` published to JetStream.
///
/// This test directly inspects the JetStream payload rather than relying on a
/// binary integration test, because the test HTTP client may strip these
/// headers itself before they reach the proxy.
#[tokio::test]
async fn e2e_multiple_transfer_encoding_headers_all_stripped() {
    use futures_util::StreamExt as _;
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

    let js_stream = jetstream
        .get_stream(&stream::stream_name("trogon"))
        .await
        .unwrap();
    let consumer = js_stream
        .get_or_create_consumer(
            "e2e-multi-te-worker",
            async_nats::jetstream::consumer::pull::Config {
                durable_name: Some("e2e-multi-te-worker".to_string()),
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
        worker_timeout: Duration::from_secs(5),
        base_url_override: Some("http://127.0.0.1:1".to_string()),
    };

    // Include two transfer-encoding headers alongside a safe custom header.
    let request = axum::http::Request::builder()
        .method("POST")
        .uri("/anthropic/v1/messages")
        .header("authorization", "Bearer tok_anthropic_test_multite01")
        .header("transfer-encoding", "chunked")
        .header("transfer-encoding", "gzip")
        .header("x-safe-header", "must-survive")
        .body(axum::body::Body::from("{}"))
        .unwrap();

    let proxy_handle = tokio::spawn(async move { router(state).oneshot(request).await });

    let msg = tokio::time::timeout(Duration::from_secs(5), messages.next())
        .await
        .expect("Timed out waiting for proxy to publish")
        .unwrap()
        .unwrap();

    let parsed: serde_json::Value = serde_json::from_slice(&msg.payload).unwrap();
    let headers_json = &parsed["headers"];

    // Neither transfer-encoding value must appear in the outbound request.
    let empty = vec![];
    let te_headers: Vec<&str> = headers_json
        .as_array()
        .unwrap_or(&empty)
        .iter()
        .filter_map(|pair| {
            let arr = pair.as_array()?;
            if arr.first()?.as_str()? == "transfer-encoding" {
                arr.get(1)?.as_str()
            } else {
                None
            }
        })
        .collect();

    assert!(
        te_headers.is_empty(),
        "All transfer-encoding headers must be stripped, found: {:?}",
        te_headers
    );

    // Non-hop-by-hop header must survive.
    assert!(
        find_header(headers_json, "x-safe-header").is_some(),
        "Non-hop-by-hop header 'x-safe-header' must be preserved"
    );

    // Unblock the proxy.
    let reply_to = parsed["reply_to"].as_str().unwrap().to_string();
    let reply = trogon_secret_proxy::messages::OutboundHttpResponse {
        status: 200,
        headers: vec![],
        body: b"ok".to_vec(),
        error: None,
    };
    nats.publish(reply_to, serde_json::to_vec(&reply).unwrap().into())
        .await
        .unwrap();
    msg.ack().await.unwrap();

    let resp = proxy_handle.await.unwrap().unwrap();
    assert_eq!(resp.status(), 200, "Proxy must complete the request successfully");
}

/// When the incoming URI has a trailing `?` with no query parameters (e.g.
/// `/anthropic/v1/messages?`), `uri.query()` returns `Some("")`, which the
/// proxy formats as `"?"` and appends to the upstream URL.
///
/// This test documents that concrete behavior: the URL stored in the
/// `OutboundHttpRequest` payload ends with `?`.  A bare `?` is valid HTTP
/// and AI providers handle it correctly.
#[tokio::test]
async fn e2e_empty_query_string_appended_as_bare_question_mark() {
    use futures_util::StreamExt as _;
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

    let js_stream = jetstream
        .get_stream(&stream::stream_name("trogon"))
        .await
        .unwrap();
    let consumer = js_stream
        .get_or_create_consumer(
            "e2e-empty-qs-worker",
            async_nats::jetstream::consumer::pull::Config {
                durable_name: Some("e2e-empty-qs-worker".to_string()),
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
        worker_timeout: Duration::from_secs(5),
        base_url_override: Some("http://mock-provider".to_string()),
    };

    // URI with a trailing `?` — empty query string, not None.
    let request = axum::http::Request::builder()
        .method("POST")
        .uri("/anthropic/v1/messages?")
        .header("authorization", "Bearer tok_anthropic_test_emptyqs01")
        .body(axum::body::Body::from("{}"))
        .unwrap();

    let proxy_handle = tokio::spawn(async move { router(state).oneshot(request).await });

    let msg = tokio::time::timeout(Duration::from_secs(5), messages.next())
        .await
        .expect("Timed out waiting for proxy to publish")
        .unwrap()
        .unwrap();

    let parsed: serde_json::Value = serde_json::from_slice(&msg.payload).unwrap();
    let url = parsed["url"].as_str().expect("url field must be present");

    // uri.query() = Some("") → format!("?{}", "") = "?" → appended to URL.
    assert!(
        url.ends_with('?'),
        "URL with empty query string must end with '?', got: {}",
        url
    );
    assert!(
        url.contains("/v1/messages?"),
        "Path must be preserved before the bare '?', got: {}",
        url
    );

    // Unblock the proxy.
    let reply_to = parsed["reply_to"].as_str().unwrap().to_string();
    let reply = trogon_secret_proxy::messages::OutboundHttpResponse {
        status: 200,
        headers: vec![],
        body: b"ok".to_vec(),
        error: None,
    };
    nats.publish(reply_to, serde_json::to_vec(&reply).unwrap().into())
        .await
        .unwrap();
    msg.ack().await.unwrap();

    let resp = proxy_handle.await.unwrap().unwrap();
    assert_eq!(resp.status(), 200);
}

/// A request header whose value bytes are valid Latin-1 but NOT valid UTF-8
/// (e.g. `0xFF`) must be silently dropped by the proxy before the
/// `OutboundHttpRequest` is published to JetStream.
///
/// `proxy.rs` line 101: `v.to_str().ok().map(...)` — returns `None` for
/// non-UTF-8 bytes, so the header is filtered out.  A sibling header with a
/// valid ASCII value must be preserved.
///
/// Uses `Request::from_parts` to inject a raw `HeaderValue::from_bytes`
/// containing `0xFF`, bypassing the `&str` builder API.
#[tokio::test]
async fn e2e_request_header_with_invalid_utf8_silently_dropped() {
    use futures_util::StreamExt as _;
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

    let js_stream = jetstream
        .get_stream(&stream::stream_name("trogon"))
        .await
        .unwrap();
    let consumer = js_stream
        .get_or_create_consumer(
            "e2e-inv-utf8-req-worker",
            async_nats::jetstream::consumer::pull::Config {
                durable_name: Some("e2e-inv-utf8-req-worker".to_string()),
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
        worker_timeout: Duration::from_secs(5),
        base_url_override: Some("http://127.0.0.1:1".to_string()),
    };

    // Build a base request, then inject a non-UTF-8 header via from_parts.
    let base = axum::http::Request::builder()
        .method("POST")
        .uri("/anthropic/v1/messages")
        .header("authorization", "Bearer tok_anthropic_test_invutf801")
        .header("x-ascii-header", "must-survive")
        .body(axum::body::Body::from("{}"))
        .unwrap();

    let (mut parts, body) = base.into_parts();
    // 0xFF is a valid header value byte (Latin-1) but not valid UTF-8.
    let bad_value = axum::http::HeaderValue::from_bytes(b"\xff\xfe").unwrap();
    parts.headers.insert(
        axum::http::HeaderName::from_static("x-binary-header"),
        bad_value,
    );
    let request = axum::http::Request::from_parts(parts, body);

    let proxy_handle = tokio::spawn(async move { router(state).oneshot(request).await });

    let msg = tokio::time::timeout(Duration::from_secs(5), messages.next())
        .await
        .expect("Timed out waiting for proxy to publish")
        .unwrap()
        .unwrap();

    let parsed: serde_json::Value = serde_json::from_slice(&msg.payload).unwrap();
    let headers_json = &parsed["headers"];

    // The invalid-UTF-8 header must be absent.
    assert!(
        find_header(headers_json, "x-binary-header").is_none(),
        "Header with invalid UTF-8 value must be silently dropped"
    );

    // The valid ASCII header must be preserved.
    assert!(
        find_header(headers_json, "x-ascii-header").is_some(),
        "Header with valid ASCII value must be forwarded"
    );

    // Unblock the proxy.
    let reply_to = parsed["reply_to"].as_str().unwrap().to_string();
    let reply = trogon_secret_proxy::messages::OutboundHttpResponse {
        status: 200,
        headers: vec![],
        body: b"ok".to_vec(),
        error: None,
    };
    nats.publish(reply_to, serde_json::to_vec(&reply).unwrap().into())
        .await
        .unwrap();
    msg.ack().await.unwrap();

    let resp = proxy_handle.await.unwrap().unwrap();
    assert_eq!(resp.status(), 200);
}

// ── Gap A ─────────────────────────────────────────────────────────────────────

/// Percent-encoded characters in the query string (e.g. `%20` for a space,
/// `%2B` for `+`) must be forwarded verbatim to the upstream provider.
///
/// `proxy.rs` line 67–72: `uri().query()` returns the raw query string
/// without decoding, and `format!("?{}", q)` appends it unchanged to the
/// URL stored in the `OutboundHttpRequest`.
#[tokio::test]
async fn e2e_query_string_encoded_chars_preserved() {
    use futures_util::StreamExt as _;
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

    let js_stream = jetstream
        .get_stream(&stream::stream_name("trogon"))
        .await
        .unwrap();
    let consumer = js_stream
        .get_or_create_consumer(
            "e2e-encoded-qs-worker",
            async_nats::jetstream::consumer::pull::Config {
                durable_name: Some("e2e-encoded-qs-worker".to_string()),
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
        worker_timeout: Duration::from_secs(5),
        base_url_override: Some("http://mock-provider".to_string()),
    };

    let request = axum::http::Request::builder()
        .method("GET")
        .uri("/anthropic/v1/messages?foo=bar%20baz&x=y%2Bz")
        .header("authorization", "Bearer tok_anthropic_test_encqs001")
        .body(axum::body::Body::empty())
        .unwrap();

    let proxy_handle = tokio::spawn(async move { router(state).oneshot(request).await });

    let msg = tokio::time::timeout(Duration::from_secs(5), messages.next())
        .await
        .expect("Timed out waiting for proxy to publish")
        .unwrap()
        .unwrap();

    let parsed: serde_json::Value = serde_json::from_slice(&msg.payload).unwrap();
    let url = parsed["url"].as_str().expect("url field must be present");

    // Percent-encoded sequences must be preserved verbatim — not decoded.
    assert!(
        url.contains("foo=bar%20baz"),
        "URL must preserve %20 encoding, got: {}",
        url
    );
    assert!(
        url.contains("x=y%2Bz"),
        "URL must preserve %2B encoding, got: {}",
        url
    );
    assert!(
        !url.contains("bar baz"),
        "URL must NOT decode %20 to a space, got: {}",
        url
    );

    // Unblock the proxy.
    let reply_to = parsed["reply_to"].as_str().unwrap().to_string();
    let reply = trogon_secret_proxy::messages::OutboundHttpResponse {
        status: 200,
        headers: vec![],
        body: b"ok".to_vec(),
        error: None,
    };
    nats.publish(reply_to, serde_json::to_vec(&reply).unwrap().into())
        .await
        .unwrap();
    msg.ack().await.unwrap();

    let resp = proxy_handle.await.unwrap().unwrap();
    assert_eq!(resp.status(), 200);
}

// ── Gap D ─────────────────────────────────────────────────────────────────────

/// When the upstream provider returns multiple `Set-Cookie` headers, the
/// proxy must forward all of them to the caller without collapsing them.
///
/// `proxy.rs` line 194–195: `response_headers.append(name, value)` (not
/// `insert`) preserves duplicate header names.
#[tokio::test]
async fn e2e_duplicate_set_cookie_headers_both_forwarded() {
    use futures_util::StreamExt as _;
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

    let js_stream = jetstream
        .get_stream(&stream::stream_name("trogon"))
        .await
        .unwrap();
    let consumer = js_stream
        .get_or_create_consumer(
            "e2e-dup-set-cookie-worker",
            async_nats::jetstream::consumer::pull::Config {
                durable_name: Some("e2e-dup-set-cookie-worker".to_string()),
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
        worker_timeout: Duration::from_secs(5),
        base_url_override: Some("http://mock-provider".to_string()),
    };

    let request = axum::http::Request::builder()
        .method("POST")
        .uri("/anthropic/v1/messages")
        .header("authorization", "Bearer tok_anthropic_test_dupcook01")
        .body(axum::body::Body::from("{}"))
        .unwrap();

    let proxy_handle = tokio::spawn(async move { router(state).oneshot(request).await });

    let msg = tokio::time::timeout(Duration::from_secs(5), messages.next())
        .await
        .expect("Timed out waiting for proxy to publish")
        .unwrap()
        .unwrap();

    let parsed: serde_json::Value = serde_json::from_slice(&msg.payload).unwrap();
    let reply_to = parsed["reply_to"].as_str().unwrap().to_string();

    // Simulate a provider response with two distinct Set-Cookie headers.
    let reply = trogon_secret_proxy::messages::OutboundHttpResponse {
        status: 200,
        headers: vec![
            ("set-cookie".to_string(), "session=abc; Path=/".to_string()),
            ("set-cookie".to_string(), "theme=dark; Path=/".to_string()),
        ],
        body: b"ok".to_vec(),
        error: None,
    };
    nats.publish(reply_to, serde_json::to_vec(&reply).unwrap().into())
        .await
        .unwrap();
    msg.ack().await.unwrap();

    let resp = proxy_handle.await.unwrap().unwrap();
    assert_eq!(resp.status(), 200);

    // Both Set-Cookie headers must be present in the response.
    let cookie_headers: Vec<_> = resp.headers().get_all("set-cookie").iter().collect();
    assert_eq!(
        cookie_headers.len(),
        2,
        "Both Set-Cookie headers must be forwarded, got {} header(s)",
        cookie_headers.len()
    );
    let values: Vec<&str> = cookie_headers.iter().map(|v| v.to_str().unwrap()).collect();
    assert!(values.contains(&"session=abc; Path=/"), "session cookie must be present");
    assert!(values.contains(&"theme=dark; Path=/"), "theme cookie must be present");
}

// ── Gap 1 ─────────────────────────────────────────────────────────────────────

/// When the worker sends an `OutboundHttpResponse` whose `status` field is not
/// a valid HTTP status code (e.g. `9999`), `StatusCode::from_u16(9999)` returns
/// `Err`, and the proxy falls back to `500 Internal Server Error`.
///
/// `proxy.rs` line 164–165:
/// ```rust
/// let status = StatusCode::from_u16(proxy_response.status)
///     .unwrap_or(StatusCode::INTERNAL_SERVER_ERROR);
/// ```
#[tokio::test]
async fn e2e_invalid_status_code_in_worker_response_falls_back_to_500() {
    use futures_util::StreamExt as _;
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

    let js_stream = jetstream
        .get_stream(&stream::stream_name("trogon"))
        .await
        .unwrap();
    let consumer = js_stream
        .get_or_create_consumer(
            "e2e-invalid-status-worker",
            async_nats::jetstream::consumer::pull::Config {
                durable_name: Some("e2e-invalid-status-worker".to_string()),
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
        worker_timeout: Duration::from_secs(5),
        base_url_override: Some("http://mock-provider".to_string()),
    };

    let request = axum::http::Request::builder()
        .method("GET")
        .uri("/anthropic/v1/messages")
        .header("authorization", "Bearer tok_anthropic_test_invstat01")
        .body(axum::body::Body::empty())
        .unwrap();

    let proxy_handle = tokio::spawn(async move { router(state).oneshot(request).await });

    let msg = tokio::time::timeout(Duration::from_secs(5), messages.next())
        .await
        .expect("Timed out waiting for proxy to publish")
        .unwrap()
        .unwrap();

    let parsed: serde_json::Value = serde_json::from_slice(&msg.payload).unwrap();
    let reply_to = parsed["reply_to"].as_str().unwrap().to_string();

    // Return a response with an out-of-range HTTP status code.
    let reply = trogon_secret_proxy::messages::OutboundHttpResponse {
        status: 9999,
        headers: vec![],
        body: b"body".to_vec(),
        error: None,
    };
    nats.publish(reply_to, serde_json::to_vec(&reply).unwrap().into())
        .await
        .unwrap();
    msg.ack().await.unwrap();

    let resp = proxy_handle.await.unwrap().unwrap();
    assert_eq!(
        resp.status(),
        500,
        "Invalid status code must fall back to 500 Internal Server Error"
    );
}

// ── Gap 2 ─────────────────────────────────────────────────────────────────────

/// When the worker publishes bytes to the Core NATS reply subject that are not
/// valid JSON, `serde_json::from_slice` fails with `ProxyError::Deserialize`,
/// which maps to `500 Internal Server Error` via `IntoResponse`.
///
/// `proxy.rs` line 161–162:
/// ```rust
/// let proxy_response: OutboundHttpResponse = serde_json::from_slice(&reply_msg.payload)
///     .map_err(|e| ProxyError::Deserialize(e.to_string()))?;
/// ```
#[tokio::test]
async fn e2e_corrupted_reply_json_returns_500() {
    use futures_util::StreamExt as _;
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

    let js_stream = jetstream
        .get_stream(&stream::stream_name("trogon"))
        .await
        .unwrap();
    let consumer = js_stream
        .get_or_create_consumer(
            "e2e-corrupted-reply-worker",
            async_nats::jetstream::consumer::pull::Config {
                durable_name: Some("e2e-corrupted-reply-worker".to_string()),
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
        worker_timeout: Duration::from_secs(5),
        base_url_override: Some("http://mock-provider".to_string()),
    };

    let request = axum::http::Request::builder()
        .method("POST")
        .uri("/anthropic/v1/messages")
        .header("authorization", "Bearer tok_anthropic_test_corrpjson1")
        .body(axum::body::Body::from("{}"))
        .unwrap();

    let proxy_handle = tokio::spawn(async move { router(state).oneshot(request).await });

    let msg = tokio::time::timeout(Duration::from_secs(5), messages.next())
        .await
        .expect("Timed out waiting for proxy to publish")
        .unwrap()
        .unwrap();

    // Extract the reply subject from the well-formed JetStream message, then
    // publish garbage bytes to it — simulating a misbehaving worker.
    let parsed: serde_json::Value = serde_json::from_slice(&msg.payload).unwrap();
    let reply_to = parsed["reply_to"].as_str().unwrap().to_string();

    nats.publish(reply_to, b"not valid json {{{".as_ref().into())
        .await
        .unwrap();
    msg.ack().await.unwrap();

    let resp = proxy_handle.await.unwrap().unwrap();
    assert_eq!(
        resp.status(),
        500,
        "Corrupted reply payload must produce 500 Internal Server Error"
    );
}
