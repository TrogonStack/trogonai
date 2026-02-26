//! Binary end-to-end tests.
//!
//! Starts the real compiled `proxy` and `worker` binaries as OS processes,
//! spins up a Docker NATS server and an httpmock AI provider, then exercises
//! the full pipeline exactly as it runs in production.
//!
//! Run with:
//!   cargo test -p trogon-secret-proxy --test binary_e2e
//!
//! Requires Docker.

use std::sync::atomic::{AtomicU16, Ordering};
use std::time::Duration;

use futures_util::future::join_all;
use futures_util::StreamExt as _;
use testcontainers_modules::nats::Nats;
use testcontainers_modules::testcontainers::{runners::AsyncRunner, ContainerAsync, ImageExt};
use tokio::process::{Child, Command};

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

static PORT_COUNTER: AtomicU16 = AtomicU16::new(30000);

/// Return a unique port number for each caller.
/// Using an atomic counter avoids the bind-and-release race condition where
/// two parallel tests could receive the same OS-assigned port and collide.
fn free_port() -> u16 {
    PORT_COUNTER.fetch_add(1, Ordering::SeqCst)
}

/// Poll until a TCP connection to `port` succeeds or `timeout` elapses.
/// Much more reliable than a fixed sleep for waiting on binary startup.
async fn wait_for_port(port: u16, timeout: Duration) {
    let deadline = tokio::time::Instant::now() + timeout;
    loop {
        match tokio::net::TcpStream::connect(format!("127.0.0.1:{}", port)).await {
            Ok(_) => return,
            Err(_) => {
                if tokio::time::Instant::now() >= deadline {
                    panic!("Port {} not ready within {:?}", port, timeout);
                }
                tokio::time::sleep(Duration::from_millis(100)).await;
            }
        }
    }
}

fn spawn_proxy(nats_port: u16, proxy_port: u16, mock_base_url: &str) -> Child {
    spawn_proxy_with_timeout(nats_port, proxy_port, mock_base_url, 15)
}

fn spawn_proxy_with_timeout(nats_port: u16, proxy_port: u16, mock_base_url: &str, timeout_secs: u64) -> Child {
    Command::new(env!("CARGO_BIN_EXE_proxy"))
        .env("NATS_URL", format!("localhost:{}", nats_port))
        .env("PROXY_PORT", proxy_port.to_string())
        .env("PROXY_WORKER_TIMEOUT_SECS", timeout_secs.to_string())
        .env("PROXY_BASE_URL_OVERRIDE", mock_base_url)
        .env("RUST_LOG", "warn")
        .kill_on_drop(true)
        .spawn()
        .expect("Failed to spawn proxy binary — run `cargo build` first")
}

fn spawn_worker(nats_port: u16, consumer_name: &str, token: &str, real_key: &str) -> Child {
    Command::new(env!("CARGO_BIN_EXE_worker"))
        .env("NATS_URL", format!("localhost:{}", nats_port))
        .env("WORKER_CONSUMER_NAME", consumer_name)
        .env(format!("VAULT_TOKEN_{}", token), real_key)
        .env("RUST_LOG", "warn")
        .kill_on_drop(true)
        .spawn()
        .expect("Failed to spawn worker binary — run `cargo build` first")
}

// ── Tests ─────────────────────────────────────────────────────────────────────

/// Full pipeline with real binaries: proxy binary + worker binary process a
/// request end-to-end.  The mock AI provider verifies the real key arrives,
/// not the proxy token.
#[tokio::test]
async fn binary_full_pipeline_happy_path() {
    let (_nats_container, nats_port) = start_nats().await;

    let mock_server = httpmock::MockServer::start_async().await;
    let ai_mock = mock_server
        .mock_async(|when, then| {
            when.method(httpmock::Method::POST)
                .path("/v1/messages")
                .header("authorization", "Bearer sk-ant-bin-realkey");
            then.status(200)
                .header("content-type", "application/json")
                .body(r#"{"id":"msg_binary_01","type":"message","content":[]}"#);
        })
        .await;

    let proxy_port = free_port();
    let token = "tok_anthropic_prod_bin001";

    let _proxy = spawn_proxy(nats_port, proxy_port, &mock_server.base_url());
    wait_for_port(proxy_port, Duration::from_secs(15)).await;

    let _worker = spawn_worker(nats_port, "binary-workers-1", token, "sk-ant-bin-realkey");
    // Give the worker time to connect to NATS and register its consumer.
    tokio::time::sleep(Duration::from_millis(500)).await;

    let resp = reqwest::Client::new()
        .post(format!(
            "http://127.0.0.1:{}/anthropic/v1/messages",
            proxy_port
        ))
        .header("Authorization", format!("Bearer {}", token))
        .header("Content-Type", "application/json")
        .body(r#"{"model":"claude-3-5-sonnet-20241022","max_tokens":10,"messages":[{"role":"user","content":"Hi"}]}"#)
        .timeout(Duration::from_secs(20))
        .send()
        .await
        .expect("Request to proxy binary failed");

    assert_eq!(resp.status(), 200, "Expected 200, got {}", resp.status());
    let body: serde_json::Value = resp.json().await.unwrap();
    assert_eq!(body["id"], "msg_binary_01", "Response body mismatch");

    // Critical: real key reached the mock, not the proxy token.
    ai_mock.assert_async().await;
}

/// JetStream durability with real binaries: proxy publishes the message to
/// JetStream before the worker process starts.  Worker joins later and must
/// pick up the persisted message.
#[tokio::test]
async fn binary_worker_starts_late_message_is_delivered() {
    let (_nats_container, nats_port) = start_nats().await;

    let mock_server = httpmock::MockServer::start_async().await;
    let ai_mock = mock_server
        .mock_async(|when, then| {
            when.method(httpmock::Method::POST).path("/v1/messages");
            then.status(200)
                .header("content-type", "application/json")
                .body(r#"{"id":"msg_binary_durable","type":"message","content":[]}"#);
        })
        .await;

    let proxy_port = free_port();
    let token = "tok_anthropic_prod_bin002";

    // Start proxy only — no worker yet.
    let _proxy = spawn_proxy(nats_port, proxy_port, &mock_server.base_url());
    wait_for_port(proxy_port, Duration::from_secs(15)).await;

    // Send the request — proxy publishes to JetStream and waits.
    let request_handle = tokio::spawn(async move {
        reqwest::Client::new()
            .post(format!(
                "http://127.0.0.1:{}/anthropic/v1/messages",
                proxy_port
            ))
            .header("Authorization", format!("Bearer {}", token))
            .header("Content-Type", "application/json")
            .body(r#"{"model":"claude-3","messages":[]}"#)
            .timeout(Duration::from_secs(20))
            .send()
            .await
            .unwrap()
    });

    // Give proxy time to publish the message before worker starts.
    tokio::time::sleep(Duration::from_millis(600)).await;

    // Start worker now — it must find and process the persisted message.
    let _worker = spawn_worker(nats_port, "binary-workers-2", token, "sk-ant-bin-realkey");

    let resp = request_handle.await.unwrap();
    assert_eq!(resp.status(), 200, "Expected 200, got {}", resp.status());
    let body: serde_json::Value = resp.json().await.unwrap();
    assert_eq!(body["id"], "msg_binary_durable");
    ai_mock.assert_async().await;
}

/// Horizontal scaling with real binaries: two worker processes share the load
/// via the durable queue group.  All 5 concurrent requests must succeed.
#[tokio::test]
async fn binary_two_workers_handle_concurrent_requests() {
    let (_nats_container, nats_port) = start_nats().await;

    let mock_server = httpmock::MockServer::start_async().await;
    let _mock = mock_server
        .mock_async(|when, then| {
            when.method(httpmock::Method::POST).path("/v1/messages");
            then.status(200)
                .header("content-type", "application/json")
                .body(r#"{"id":"msg_binary_scaled","type":"message","content":[]}"#);
        })
        .await;

    let proxy_port = free_port();
    let token = "tok_anthropic_prod_bin003";

    let _proxy = spawn_proxy(nats_port, proxy_port, &mock_server.base_url());
    wait_for_port(proxy_port, Duration::from_secs(15)).await;

    // Two worker instances share the same consumer name → JetStream queue group.
    let _worker_a = spawn_worker(nats_port, "binary-workers-scaled", token, "sk-ant-bin-realkey");
    let _worker_b = spawn_worker(nats_port, "binary-workers-scaled", token, "sk-ant-bin-realkey");
    tokio::time::sleep(Duration::from_millis(500)).await;

    // Fire 5 requests simultaneously.
    let client = reqwest::Client::new();
    let handles: Vec<_> = (0..5)
        .map(|_| {
            let c = client.clone();
            let url = format!("http://127.0.0.1:{}/anthropic/v1/messages", proxy_port);
            tokio::spawn(async move {
                c.post(url)
                    .header("Authorization", format!("Bearer {}", token))
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
        assert_eq!(body["id"], "msg_binary_scaled");
    }
}

/// Token not registered in the worker vault → proxy must return an error
/// status (the worker replies with a 401-equivalent wrapped as 502).
#[tokio::test]
async fn binary_unknown_token_returns_error() {
    let (_nats_container, nats_port) = start_nats().await;

    let mock_server = httpmock::MockServer::start_async().await;
    // This mock must NOT be called — the worker should reject the token
    // before reaching the AI provider.
    let _should_not_be_called = mock_server
        .mock_async(|when, then| {
            when.any_request();
            then.status(200).body("should not reach here");
        })
        .await;

    let proxy_port = free_port();

    // Worker starts with an EMPTY vault — no tokens registered.
    let _proxy = spawn_proxy(nats_port, proxy_port, &mock_server.base_url());
    wait_for_port(proxy_port, Duration::from_secs(15)).await;

    // Spawn worker with no VAULT_TOKEN_* vars → vault is empty.
    let _worker = Command::new(env!("CARGO_BIN_EXE_worker"))
        .env("NATS_URL", format!("localhost:{}", nats_port))
        .env("WORKER_CONSUMER_NAME", "binary-workers-unknown")
        .env("RUST_LOG", "warn")
        .kill_on_drop(true)
        .spawn()
        .unwrap();
    tokio::time::sleep(Duration::from_millis(500)).await;

    let resp = reqwest::Client::new()
        .post(format!(
            "http://127.0.0.1:{}/anthropic/v1/messages",
            proxy_port
        ))
        .header("Authorization", "Bearer tok_anthropic_prod_notinvault")
        .header("Content-Type", "application/json")
        .body("{}")
        .timeout(Duration::from_secs(15))
        .send()
        .await
        .unwrap();

    assert!(
        resp.status().is_client_error() || resp.status().is_server_error(),
        "Expected error status for unknown token, got {}",
        resp.status()
    );
    // Mock was never reached.
    assert_eq!(_should_not_be_called.hits(), 0, "AI provider should not have been called");
}

/// Two different tokens each map to a different real key.
/// The mock AI verifies each request arrived with the correct key.
#[tokio::test]
async fn binary_multiple_tokens_each_resolve_to_correct_key() {
    let (_nats_container, nats_port) = start_nats().await;

    let mock_server = httpmock::MockServer::start_async().await;

    // Token A → sk-ant-key-a → response id "msg_key_a"
    let mock_a = mock_server
        .mock_async(|when, then| {
            when.method(httpmock::Method::POST)
                .path("/v1/messages")
                .header("authorization", "Bearer sk-ant-key-a");
            then.status(200)
                .header("content-type", "application/json")
                .body(r#"{"id":"msg_key_a","type":"message"}"#);
        })
        .await;

    // Token B → sk-ant-key-b → response id "msg_key_b"
    let mock_b = mock_server
        .mock_async(|when, then| {
            when.method(httpmock::Method::POST)
                .path("/v1/messages")
                .header("authorization", "Bearer sk-ant-key-b");
            then.status(200)
                .header("content-type", "application/json")
                .body(r#"{"id":"msg_key_b","type":"message"}"#);
        })
        .await;

    let proxy_port = free_port();
    let token_a = "tok_anthropic_prod_mta001";
    let token_b = "tok_anthropic_prod_mtb002";

    let _proxy = spawn_proxy(nats_port, proxy_port, &mock_server.base_url());
    wait_for_port(proxy_port, Duration::from_secs(15)).await;

    // Worker seeded with both tokens.
    let _worker = Command::new(env!("CARGO_BIN_EXE_worker"))
        .env("NATS_URL", format!("localhost:{}", nats_port))
        .env("WORKER_CONSUMER_NAME", "binary-workers-multi")
        .env(format!("VAULT_TOKEN_{}", token_a), "sk-ant-key-a")
        .env(format!("VAULT_TOKEN_{}", token_b), "sk-ant-key-b")
        .env("RUST_LOG", "warn")
        .kill_on_drop(true)
        .spawn()
        .unwrap();
    tokio::time::sleep(Duration::from_millis(500)).await;

    let client = reqwest::Client::new();

    // Request with token A.
    let resp_a = client
        .post(format!("http://127.0.0.1:{}/anthropic/v1/messages", proxy_port))
        .header("Authorization", format!("Bearer {}", token_a))
        .header("Content-Type", "application/json")
        .body("{}")
        .timeout(Duration::from_secs(15))
        .send()
        .await
        .unwrap();

    assert_eq!(resp_a.status(), 200);
    let body_a: serde_json::Value = resp_a.json().await.unwrap();
    assert_eq!(body_a["id"], "msg_key_a", "Token A should resolve to key-a");

    // Request with token B.
    let resp_b = client
        .post(format!("http://127.0.0.1:{}/anthropic/v1/messages", proxy_port))
        .header("Authorization", format!("Bearer {}", token_b))
        .header("Content-Type", "application/json")
        .body("{}")
        .timeout(Duration::from_secs(15))
        .send()
        .await
        .unwrap();

    assert_eq!(resp_b.status(), 200);
    let body_b: serde_json::Value = resp_b.json().await.unwrap();
    assert_eq!(body_b["id"], "msg_key_b", "Token B should resolve to key-b");

    mock_a.assert_async().await;
    mock_b.assert_async().await;
}

/// Worker restart: kill the first worker process, start a second one.
/// The second worker must process new requests correctly.
#[tokio::test]
async fn binary_worker_restart_new_worker_handles_requests() {
    let (_nats_container, nats_port) = start_nats().await;

    let mock_server = httpmock::MockServer::start_async().await;
    let _mock = mock_server
        .mock_async(|when, then| {
            when.method(httpmock::Method::POST).path("/v1/messages");
            then.status(200)
                .header("content-type", "application/json")
                .body(r#"{"id":"msg_after_restart","type":"message"}"#);
        })
        .await;

    let proxy_port = free_port();
    let token = "tok_anthropic_prod_rst001";

    let _proxy = spawn_proxy(nats_port, proxy_port, &mock_server.base_url());
    wait_for_port(proxy_port, Duration::from_secs(15)).await;

    // Start first worker, send a request, verify it works.
    let mut worker_1 = spawn_worker(nats_port, "binary-workers-restart", token, "sk-ant-restart-key");
    tokio::time::sleep(Duration::from_millis(500)).await;

    let resp = reqwest::Client::new()
        .post(format!("http://127.0.0.1:{}/anthropic/v1/messages", proxy_port))
        .header("Authorization", format!("Bearer {}", token))
        .header("Content-Type", "application/json")
        .body("{}")
        .timeout(Duration::from_secs(15))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200, "First request failed before restart");

    // Kill worker 1.
    worker_1.kill().await.unwrap();
    tokio::time::sleep(Duration::from_millis(200)).await;

    // Start worker 2 — same consumer name picks up the durable subscription.
    let _worker_2 = spawn_worker(nats_port, "binary-workers-restart", token, "sk-ant-restart-key");
    tokio::time::sleep(Duration::from_millis(500)).await;

    // New request after restart must succeed.
    let resp2 = reqwest::Client::new()
        .post(format!("http://127.0.0.1:{}/anthropic/v1/messages", proxy_port))
        .header("Authorization", format!("Bearer {}", token))
        .header("Content-Type", "application/json")
        .body("{}")
        .timeout(Duration::from_secs(15))
        .send()
        .await
        .unwrap();

    assert_eq!(resp2.status(), 200, "Second request failed after restart");
    let body: serde_json::Value = resp2.json().await.unwrap();
    assert_eq!(body["id"], "msg_after_restart");
}

/// No worker running → proxy must return 504 Gateway Timeout after its
/// configured timeout expires.
#[tokio::test]
async fn binary_no_worker_returns_504_timeout() {
    let (_nats_container, nats_port) = start_nats().await;

    let mock_server = httpmock::MockServer::start_async().await;
    let proxy_port = free_port();

    // 2-second timeout so the test doesn't wait 60s.
    let _proxy = spawn_proxy_with_timeout(nats_port, proxy_port, &mock_server.base_url(), 2);
    wait_for_port(proxy_port, Duration::from_secs(15)).await;

    // No worker spawned — proxy will time out waiting for a reply.
    let resp = reqwest::Client::new()
        .post(format!(
            "http://127.0.0.1:{}/anthropic/v1/messages",
            proxy_port
        ))
        .header("Authorization", "Bearer tok_anthropic_prod_tout01")
        .header("Content-Type", "application/json")
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

/// Response headers returned by the AI provider must be forwarded back to
/// the caller through the proxy.
#[tokio::test]
async fn binary_response_headers_are_forwarded_to_caller() {
    let (_nats_container, nats_port) = start_nats().await;

    let mock_server = httpmock::MockServer::start_async().await;
    let _mock = mock_server
        .mock_async(|when, then| {
            when.method(httpmock::Method::POST).path("/v1/messages");
            then.status(200)
                .header("content-type", "application/json")
                .header("x-request-id", "upstream-req-id-123")
                .header("x-ratelimit-remaining-requests", "999")
                .body(r#"{"id":"msg_headers"}"#);
        })
        .await;

    let proxy_port = free_port();
    let token = "tok_anthropic_prod_hdr001";

    let _proxy = spawn_proxy(nats_port, proxy_port, &mock_server.base_url());
    wait_for_port(proxy_port, Duration::from_secs(15)).await;

    let _worker = spawn_worker(nats_port, "binary-workers-headers", token, "sk-ant-key-hdr");
    tokio::time::sleep(Duration::from_millis(500)).await;

    let resp = reqwest::Client::new()
        .post(format!(
            "http://127.0.0.1:{}/anthropic/v1/messages",
            proxy_port
        ))
        .header("Authorization", format!("Bearer {}", token))
        .header("Content-Type", "application/json")
        .body("{}")
        .timeout(Duration::from_secs(15))
        .send()
        .await
        .unwrap();

    assert_eq!(resp.status(), 200);
    assert_eq!(
        resp.headers().get("x-request-id").and_then(|v| v.to_str().ok()),
        Some("upstream-req-id-123"),
        "x-request-id header should be forwarded"
    );
    assert_eq!(
        resp.headers()
            .get("x-ratelimit-remaining-requests")
            .and_then(|v| v.to_str().ok()),
        Some("999"),
        "x-ratelimit-remaining-requests header should be forwarded"
    );
}

/// Unknown provider in the path → proxy returns 502 immediately without
/// publishing to NATS or waiting for a worker.
///
/// NOTE: spawned without PROXY_BASE_URL_OVERRIDE so the provider check in
/// proxy.rs is NOT bypassed.  Unknown providers are rejected before any
/// NATS publish, so no worker is needed.
#[tokio::test]
async fn binary_unknown_provider_returns_502_immediately() {
    let (_nats_container, nats_port) = start_nats().await;
    let proxy_port = free_port();

    // Spawn proxy WITHOUT PROXY_BASE_URL_OVERRIDE so the provider guard runs.
    let _proxy = Command::new(env!("CARGO_BIN_EXE_proxy"))
        .env("NATS_URL", format!("localhost:{}", nats_port))
        .env("PROXY_PORT", proxy_port.to_string())
        .env("PROXY_WORKER_TIMEOUT_SECS", "5")
        .env("RUST_LOG", "warn")
        .kill_on_drop(true)
        .spawn()
        .expect("Failed to spawn proxy binary");
    wait_for_port(proxy_port, Duration::from_secs(15)).await;

    // No worker spawned — the proxy must reject before touching NATS.
    let resp = reqwest::Client::new()
        .post(format!(
            "http://127.0.0.1:{}/fakeai/v1/completions",
            proxy_port
        ))
        .header("Authorization", "Bearer tok_anthropic_prod_abc123")
        .header("Content-Type", "application/json")
        .body("{}")
        .timeout(Duration::from_secs(5))
        .send()
        .await
        .unwrap();

    assert_eq!(
        resp.status(),
        502,
        "Unknown provider should return 502 immediately, got {}",
        resp.status()
    );
}

/// GET request (e.g. listing models) passes through the proxy and reaches
/// the AI provider with the original method intact.
#[tokio::test]
async fn binary_get_request_passes_through() {
    let (_nats_container, nats_port) = start_nats().await;

    let mock_server = httpmock::MockServer::start_async().await;
    let _mock = mock_server
        .mock_async(|when, then| {
            when.method(httpmock::Method::GET).path("/v1/models");
            then.status(200)
                .header("content-type", "application/json")
                .body(r#"{"object":"list","data":[{"id":"claude-3-5-sonnet-20241022"}]}"#);
        })
        .await;

    let proxy_port = free_port();
    let token = "tok_anthropic_prod_get001";

    let _proxy = spawn_proxy(nats_port, proxy_port, &mock_server.base_url());
    wait_for_port(proxy_port, Duration::from_secs(15)).await;

    let _worker = spawn_worker(nats_port, "binary-workers-get", token, "sk-ant-key-get");
    tokio::time::sleep(Duration::from_millis(500)).await;

    let resp = reqwest::Client::new()
        .get(format!(
            "http://127.0.0.1:{}/anthropic/v1/models",
            proxy_port
        ))
        .header("Authorization", format!("Bearer {}", token))
        .timeout(Duration::from_secs(15))
        .send()
        .await
        .unwrap();

    assert_eq!(resp.status(), 200, "Expected 200, got {}", resp.status());
    let body: serde_json::Value = resp.json().await.unwrap();
    assert_eq!(body["object"], "list", "Response body mismatch");
    _mock.assert_async().await;
}

/// AI provider always responds with 503 Service Unavailable.
/// The worker exhausts its 4 attempts (1 + 3 retries) then publishes the
/// last provider status back to the proxy, which forwards it as-is.
#[tokio::test]
async fn binary_provider_5xx_exhausts_retries_and_status_is_forwarded() {
    let (_nats_container, nats_port) = start_nats().await;

    let mock_server = httpmock::MockServer::start_async().await;
    // Always returns 503 — worker will retry up to 4 attempts then give up.
    let _mock = mock_server
        .mock_async(|when, then| {
            when.method(httpmock::Method::POST).path("/v1/messages");
            then.status(503)
                .header("content-type", "application/json")
                .body(r#"{"error":{"type":"overloaded_error","message":"Overloaded"}}"#);
        })
        .await;

    let proxy_port = free_port();
    let token = "tok_anthropic_prod_5xx001";

    let _proxy = spawn_proxy(nats_port, proxy_port, &mock_server.base_url());
    wait_for_port(proxy_port, Duration::from_secs(15)).await;

    let _worker = spawn_worker(nats_port, "binary-workers-5xx", token, "sk-ant-key-5xx");
    tokio::time::sleep(Duration::from_millis(500)).await;

    let resp = reqwest::Client::new()
        .post(format!(
            "http://127.0.0.1:{}/anthropic/v1/messages",
            proxy_port
        ))
        .header("Authorization", format!("Bearer {}", token))
        .header("Content-Type", "application/json")
        .body("{}")
        // Retries add up to ~700ms of backoff; give generous timeout.
        .timeout(Duration::from_secs(20))
        .send()
        .await
        .unwrap();

    // Worker forwards the provider's last status code directly — 503 passes through.
    assert_eq!(
        resp.status(),
        503,
        "Provider 503 should be forwarded to the caller, got {}",
        resp.status()
    );

    // Worker made 4 attempts (1 original + 3 retries).
    assert_eq!(
        _mock.hits(),
        4,
        "Worker should have made exactly 4 attempts before giving up"
    );
}

/// Large request body (~50 KB) must pass through the proxy and reach the
/// AI provider intact.
#[tokio::test]
async fn binary_large_request_body_passes_through() {
    let (_nats_container, nats_port) = start_nats().await;

    // Build a ~50 KB JSON body.
    let large_content = "x".repeat(50_000);
    let large_body = format!(
        r#"{{"model":"claude-3","messages":[{{"role":"user","content":"{}"}}]}}"#,
        large_content
    );

    let mock_server = httpmock::MockServer::start_async().await;
    let _mock = mock_server
        .mock_async(|when, then| {
            when.method(httpmock::Method::POST).path("/v1/messages");
            then.status(200)
                .header("content-type", "application/json")
                .body(r#"{"id":"msg_large_body"}"#);
        })
        .await;

    let proxy_port = free_port();
    let token = "tok_anthropic_prod_lgb001";

    let _proxy = spawn_proxy(nats_port, proxy_port, &mock_server.base_url());
    wait_for_port(proxy_port, Duration::from_secs(15)).await;

    let _worker = spawn_worker(nats_port, "binary-workers-large", token, "sk-ant-key-large");
    tokio::time::sleep(Duration::from_millis(500)).await;

    let resp = reqwest::Client::new()
        .post(format!(
            "http://127.0.0.1:{}/anthropic/v1/messages",
            proxy_port
        ))
        .header("Authorization", format!("Bearer {}", token))
        .header("Content-Type", "application/json")
        .body(large_body)
        .timeout(Duration::from_secs(20))
        .send()
        .await
        .unwrap();

    assert_eq!(resp.status(), 200, "Expected 200, got {}", resp.status());
    let body: serde_json::Value = resp.json().await.unwrap();
    assert_eq!(body["id"], "msg_large_body");
}

/// Security: if a caller sends an actual API key (e.g. `sk-ant-...`) instead
/// of an opaque token, the worker must reject it.  The real key must never
/// reach the AI provider.
#[tokio::test]
async fn binary_real_api_key_in_auth_is_rejected() {
    let (_nats_container, nats_port) = start_nats().await;

    let mock_server = httpmock::MockServer::start_async().await;
    // This mock must NOT be reached — the worker rejects before forwarding.
    let _should_not_be_called = mock_server
        .mock_async(|when, then| {
            when.any_request();
            then.status(200).body("should not reach here");
        })
        .await;

    let proxy_port = free_port();

    let _proxy = spawn_proxy(nats_port, proxy_port, &mock_server.base_url());
    wait_for_port(proxy_port, Duration::from_secs(15)).await;

    // Worker with empty vault (no tokens registered).
    let _worker = Command::new(env!("CARGO_BIN_EXE_worker"))
        .env("NATS_URL", format!("localhost:{}", nats_port))
        .env("WORKER_CONSUMER_NAME", "binary-workers-realkey")
        .env("RUST_LOG", "warn")
        .kill_on_drop(true)
        .spawn()
        .unwrap();
    tokio::time::sleep(Duration::from_millis(500)).await;

    // Send a request with a REAL key in Authorization, not a tok_ token.
    let resp = reqwest::Client::new()
        .post(format!(
            "http://127.0.0.1:{}/anthropic/v1/messages",
            proxy_port
        ))
        .header("Authorization", "Bearer sk-ant-real-secret-key-12345")
        .header("Content-Type", "application/json")
        .body("{}")
        .timeout(Duration::from_secs(15))
        .send()
        .await
        .unwrap();

    // Worker rejects non-tok_ credentials → proxy returns 502.
    assert!(
        resp.status().is_server_error(),
        "Real API key should be rejected with a server error, got {}",
        resp.status()
    );
    // Critical: the AI provider was never contacted.
    assert_eq!(
        _should_not_be_called.hits(),
        0,
        "AI provider must not be called when a real key is passed"
    );
}

/// OpenAI provider: requests to /openai/* are routed correctly and the
/// worker resolves an OpenAI token to its real key.
#[tokio::test]
async fn binary_openai_provider_routes_correctly() {
    let (_nats_container, nats_port) = start_nats().await;

    let mock_server = httpmock::MockServer::start_async().await;
    let _mock = mock_server
        .mock_async(|when, then| {
            when.method(httpmock::Method::POST)
                .path("/v1/chat/completions")
                .header("authorization", "Bearer sk-openai-real-key");
            then.status(200)
                .header("content-type", "application/json")
                .body(r#"{"id":"chatcmpl-openai-01","object":"chat.completion"}"#);
        })
        .await;

    let proxy_port = free_port();
    let token = "tok_openai_prod_oai001";

    let _proxy = spawn_proxy(nats_port, proxy_port, &mock_server.base_url());
    wait_for_port(proxy_port, Duration::from_secs(15)).await;

    let _worker = spawn_worker(nats_port, "binary-workers-openai", token, "sk-openai-real-key");
    tokio::time::sleep(Duration::from_millis(500)).await;

    let resp = reqwest::Client::new()
        .post(format!(
            "http://127.0.0.1:{}/openai/v1/chat/completions",
            proxy_port
        ))
        .header("Authorization", format!("Bearer {}", token))
        .header("Content-Type", "application/json")
        .body(r#"{"model":"gpt-4o","messages":[{"role":"user","content":"Hi"}]}"#)
        .timeout(Duration::from_secs(15))
        .send()
        .await
        .unwrap();

    assert_eq!(resp.status(), 200, "Expected 200, got {}", resp.status());
    let body: serde_json::Value = resp.json().await.unwrap();
    assert_eq!(body["object"], "chat.completion");
    _mock.assert_async().await;
}

/// 4xx responses from the AI provider are NOT retried — the worker forwards
/// the error immediately (retrying a 4xx is pointless).
#[tokio::test]
async fn binary_4xx_from_provider_is_not_retried() {
    let (_nats_container, nats_port) = start_nats().await;

    let mock_server = httpmock::MockServer::start_async().await;
    // Returns 422 Unprocessable Entity — a client error that must not be retried.
    let mock = mock_server
        .mock_async(|when, then| {
            when.method(httpmock::Method::POST).path("/v1/messages");
            then.status(422)
                .header("content-type", "application/json")
                .body(r#"{"error":{"type":"invalid_request_error","message":"max_tokens required"}}"#);
        })
        .await;

    let proxy_port = free_port();
    let token = "tok_anthropic_prod_4xx001";

    let _proxy = spawn_proxy(nats_port, proxy_port, &mock_server.base_url());
    wait_for_port(proxy_port, Duration::from_secs(15)).await;

    let _worker = spawn_worker(nats_port, "binary-workers-4xx", token, "sk-ant-key-4xx");
    tokio::time::sleep(Duration::from_millis(500)).await;

    let resp = reqwest::Client::new()
        .post(format!(
            "http://127.0.0.1:{}/anthropic/v1/messages",
            proxy_port
        ))
        .header("Authorization", format!("Bearer {}", token))
        .header("Content-Type", "application/json")
        .body("{}")
        .timeout(Duration::from_secs(15))
        .send()
        .await
        .unwrap();

    // 422 forwarded as-is.
    assert_eq!(
        resp.status(),
        422,
        "4xx should be forwarded without retry, got {}",
        resp.status()
    );
    // Provider was called exactly once — no retries.
    assert_eq!(mock.hits(), 1, "4xx must not trigger retries");
}

/// No Authorization header at all → worker cannot extract a token and must
/// return an error; proxy returns 502.
#[tokio::test]
async fn binary_missing_auth_header_returns_error() {
    let (_nats_container, nats_port) = start_nats().await;

    let mock_server = httpmock::MockServer::start_async().await;
    let _should_not_be_called = mock_server
        .mock_async(|when, then| {
            when.any_request();
            then.status(200).body("should not reach here");
        })
        .await;

    let proxy_port = free_port();
    let token = "tok_anthropic_prod_noauth1";

    let _proxy = spawn_proxy(nats_port, proxy_port, &mock_server.base_url());
    wait_for_port(proxy_port, Duration::from_secs(15)).await;

    let _worker = spawn_worker(nats_port, "binary-workers-noauth", token, "sk-ant-key-noauth");
    tokio::time::sleep(Duration::from_millis(500)).await;

    // No Authorization header in the request.
    let resp = reqwest::Client::new()
        .post(format!(
            "http://127.0.0.1:{}/anthropic/v1/messages",
            proxy_port
        ))
        .header("Content-Type", "application/json")
        .body("{}")
        .timeout(Duration::from_secs(15))
        .send()
        .await
        .unwrap();

    assert!(
        resp.status().is_server_error(),
        "Missing auth header should return 5xx, got {}",
        resp.status()
    );
    assert_eq!(
        _should_not_be_called.hits(),
        0,
        "AI provider must not be called when Authorization header is missing"
    );
}

/// Request headers sent by the caller (e.g. anthropic-version, x-api-key)
/// must be forwarded to the AI provider unchanged.
#[tokio::test]
async fn binary_request_headers_are_forwarded_to_provider() {
    let (_nats_container, nats_port) = start_nats().await;

    let mock_server = httpmock::MockServer::start_async().await;
    let _mock = mock_server
        .mock_async(|when, then| {
            when.method(httpmock::Method::POST)
                .path("/v1/messages")
                // Verify custom request headers arrive at the provider.
                .header("anthropic-version", "2023-06-01")
                .header("x-custom-caller-header", "my-service-v2");
            then.status(200)
                .header("content-type", "application/json")
                .body(r#"{"id":"msg_reqhdrs"}"#);
        })
        .await;

    let proxy_port = free_port();
    let token = "tok_anthropic_prod_hdrs001";

    let _proxy = spawn_proxy(nats_port, proxy_port, &mock_server.base_url());
    wait_for_port(proxy_port, Duration::from_secs(15)).await;

    let _worker = spawn_worker(nats_port, "binary-workers-reqhdrs", token, "sk-ant-key-hdrs");
    tokio::time::sleep(Duration::from_millis(500)).await;

    let resp = reqwest::Client::new()
        .post(format!(
            "http://127.0.0.1:{}/anthropic/v1/messages",
            proxy_port
        ))
        .header("Authorization", format!("Bearer {}", token))
        .header("Content-Type", "application/json")
        .header("anthropic-version", "2023-06-01")
        .header("x-custom-caller-header", "my-service-v2")
        .body("{}")
        .timeout(Duration::from_secs(15))
        .send()
        .await
        .unwrap();

    assert_eq!(resp.status(), 200, "Expected 200, got {}", resp.status());
    _mock.assert_async().await;
}

/// Query string parameters appended to the path must be forwarded to the
/// AI provider intact.
#[tokio::test]
async fn binary_query_string_is_forwarded() {
    let (_nats_container, nats_port) = start_nats().await;

    let mock_server = httpmock::MockServer::start_async().await;
    let _mock = mock_server
        .mock_async(|when, then| {
            when.method(httpmock::Method::GET)
                .path("/v1/models")
                .query_param("limit", "5")
                .query_param("order", "desc");
            then.status(200)
                .header("content-type", "application/json")
                .body(r#"{"object":"list","data":[]}"#);
        })
        .await;

    let proxy_port = free_port();
    let token = "tok_anthropic_prod_qs001";

    let _proxy = spawn_proxy(nats_port, proxy_port, &mock_server.base_url());
    wait_for_port(proxy_port, Duration::from_secs(15)).await;

    let _worker = spawn_worker(nats_port, "binary-workers-qs", token, "sk-ant-key-qs");
    tokio::time::sleep(Duration::from_millis(500)).await;

    let resp = reqwest::Client::new()
        .get(format!(
            "http://127.0.0.1:{}/anthropic/v1/models?limit=5&order=desc",
            proxy_port
        ))
        .header("Authorization", format!("Bearer {}", token))
        .timeout(Duration::from_secs(15))
        .send()
        .await
        .unwrap();

    assert_eq!(resp.status(), 200, "Expected 200, got {}", resp.status());
    _mock.assert_async().await;
}

/// DELETE and PUT methods pass through the proxy with the correct method.
#[tokio::test]
async fn binary_delete_and_put_methods_pass_through() {
    let (_nats_container, nats_port) = start_nats().await;

    let mock_server = httpmock::MockServer::start_async().await;
    let mock_delete = mock_server
        .mock_async(|when, then| {
            when.method(httpmock::Method::DELETE).path("/v1/files/file-abc");
            then.status(200)
                .header("content-type", "application/json")
                .body(r#"{"id":"file-abc","deleted":true}"#);
        })
        .await;
    let mock_put = mock_server
        .mock_async(|when, then| {
            when.method(httpmock::Method::PUT).path("/v1/assistants/asst-xyz");
            then.status(200)
                .header("content-type", "application/json")
                .body(r#"{"id":"asst-xyz","updated":true}"#);
        })
        .await;

    let proxy_port = free_port();
    let token = "tok_openai_prod_meth001";

    let _proxy = spawn_proxy(nats_port, proxy_port, &mock_server.base_url());
    wait_for_port(proxy_port, Duration::from_secs(15)).await;

    let _worker = spawn_worker(nats_port, "binary-workers-methods", token, "sk-openai-key-meth");
    tokio::time::sleep(Duration::from_millis(500)).await;

    let client = reqwest::Client::new();

    let resp_delete = client
        .delete(format!(
            "http://127.0.0.1:{}/openai/v1/files/file-abc",
            proxy_port
        ))
        .header("Authorization", format!("Bearer {}", token))
        .timeout(Duration::from_secs(15))
        .send()
        .await
        .unwrap();
    assert_eq!(resp_delete.status(), 200, "DELETE failed: {}", resp_delete.status());
    let body_d: serde_json::Value = resp_delete.json().await.unwrap();
    assert_eq!(body_d["deleted"], true);

    let resp_put = client
        .put(format!(
            "http://127.0.0.1:{}/openai/v1/assistants/asst-xyz",
            proxy_port
        ))
        .header("Authorization", format!("Bearer {}", token))
        .header("Content-Type", "application/json")
        .body(r#"{"name":"updated"}"#)
        .timeout(Duration::from_secs(15))
        .send()
        .await
        .unwrap();
    assert_eq!(resp_put.status(), 200, "PUT failed: {}", resp_put.status());
    let body_p: serde_json::Value = resp_put.json().await.unwrap();
    assert_eq!(body_p["updated"], true);

    mock_delete.assert_async().await;
    mock_put.assert_async().await;
}

/// Proxy times out (no worker responds in time), but a late worker comes up
/// and processes the orphaned JetStream message.  The worker must survive
/// publishing to a reply subject with no subscriber and keep handling new
/// requests normally afterwards.
#[tokio::test]
async fn binary_worker_survives_orphaned_reply_subject() {
    let (_nats_container, nats_port) = start_nats().await;

    let mock_server = httpmock::MockServer::start_async().await;
    let _mock = mock_server
        .mock_async(|when, then| {
            when.method(httpmock::Method::POST).path("/v1/messages");
            then.status(200)
                .header("content-type", "application/json")
                .body(r#"{"id":"msg_orphan_ok"}"#);
        })
        .await;

    let proxy_port = free_port();
    let token = "tok_anthropic_prod_orp001";

    // 2-second proxy timeout so the test doesn't wait long.
    let _proxy = spawn_proxy_with_timeout(nats_port, proxy_port, &mock_server.base_url(), 2);
    wait_for_port(proxy_port, Duration::from_secs(15)).await;

    // Send request BEFORE worker starts → proxy will time out.
    let timeout_resp = reqwest::Client::new()
        .post(format!("http://127.0.0.1:{}/anthropic/v1/messages", proxy_port))
        .header("Authorization", format!("Bearer {}", token))
        .header("Content-Type", "application/json")
        .body("{}")
        .timeout(Duration::from_secs(10))
        .send()
        .await
        .unwrap();
    assert_eq!(timeout_resp.status(), 504, "Should have timed out with 504");

    // Now start the worker — it picks up the orphaned message from JetStream,
    // calls the mock, tries to publish to the dead reply subject (ignored),
    // then acks the message and stays healthy.
    let _worker = spawn_worker(nats_port, "binary-workers-orphan", token, "sk-ant-key-orphan");
    tokio::time::sleep(Duration::from_millis(800)).await;

    // A fresh request must succeed — worker is still alive.
    let ok_resp = reqwest::Client::new()
        .post(format!("http://127.0.0.1:{}/anthropic/v1/messages", proxy_port))
        .header("Authorization", format!("Bearer {}", token))
        .header("Content-Type", "application/json")
        .body("{}")
        .timeout(Duration::from_secs(15))
        .send()
        .await
        .unwrap();
    assert_eq!(ok_resp.status(), 200, "Worker should still be alive after orphaned reply");

    // Mock was called twice: once for the orphaned message, once for the fresh request.
    assert_eq!(_mock.hits(), 2, "Mock should have been hit exactly twice");
}

/// Gemini, Cohere, and Mistral providers are routed correctly through the
/// full pipeline.
#[tokio::test]
async fn binary_gemini_cohere_mistral_providers_route_correctly() {
    let (_nats_container, nats_port) = start_nats().await;

    let mock_server = httpmock::MockServer::start_async().await;

    let mock_gemini = mock_server
        .mock_async(|when, then| {
            when.method(httpmock::Method::POST)
                .path("/v1beta/models/gemini-pro:generateContent");
            then.status(200)
                .header("content-type", "application/json")
                .body(r#"{"candidates":[{"content":{"parts":[{"text":"hi"}]}}]}"#);
        })
        .await;
    let mock_cohere = mock_server
        .mock_async(|when, then| {
            when.method(httpmock::Method::POST).path("/v1/chat");
            then.status(200)
                .header("content-type", "application/json")
                .body(r#"{"text":"hi from cohere"}"#);
        })
        .await;
    let mock_mistral = mock_server
        .mock_async(|when, then| {
            when.method(httpmock::Method::POST).path("/v1/chat/completions");
            then.status(200)
                .header("content-type", "application/json")
                .body(r#"{"id":"mistral-01","object":"chat.completion"}"#);
        })
        .await;

    let proxy_port = free_port();
    let tok_gemini  = "tok_gemini_prod_gem001";
    let tok_cohere  = "tok_cohere_prod_coh001";
    let tok_mistral = "tok_mistral_prod_mis001";

    let _proxy = spawn_proxy(nats_port, proxy_port, &mock_server.base_url());
    wait_for_port(proxy_port, Duration::from_secs(15)).await;

    let _worker = Command::new(env!("CARGO_BIN_EXE_worker"))
        .env("NATS_URL", format!("localhost:{}", nats_port))
        .env("WORKER_CONSUMER_NAME", "binary-workers-multi-prov")
        .env(format!("VAULT_TOKEN_{}", tok_gemini),  "sk-gemini-real")
        .env(format!("VAULT_TOKEN_{}", tok_cohere),  "sk-cohere-real")
        .env(format!("VAULT_TOKEN_{}", tok_mistral), "sk-mistral-real")
        .env("RUST_LOG", "warn")
        .kill_on_drop(true)
        .spawn()
        .unwrap();
    tokio::time::sleep(Duration::from_millis(500)).await;

    let client = reqwest::Client::new();

    let resp_gemini = client
        .post(format!(
            "http://127.0.0.1:{}/gemini/v1beta/models/gemini-pro:generateContent",
            proxy_port
        ))
        .header("Authorization", format!("Bearer {}", tok_gemini))
        .header("Content-Type", "application/json")
        .body("{}")
        .timeout(Duration::from_secs(15))
        .send()
        .await
        .unwrap();
    assert_eq!(resp_gemini.status(), 200, "Gemini failed: {}", resp_gemini.status());

    let resp_cohere = client
        .post(format!(
            "http://127.0.0.1:{}/cohere/v1/chat",
            proxy_port
        ))
        .header("Authorization", format!("Bearer {}", tok_cohere))
        .header("Content-Type", "application/json")
        .body("{}")
        .timeout(Duration::from_secs(15))
        .send()
        .await
        .unwrap();
    assert_eq!(resp_cohere.status(), 200, "Cohere failed: {}", resp_cohere.status());

    let resp_mistral = client
        .post(format!(
            "http://127.0.0.1:{}/mistral/v1/chat/completions",
            proxy_port
        ))
        .header("Authorization", format!("Bearer {}", tok_mistral))
        .header("Content-Type", "application/json")
        .body("{}")
        .timeout(Duration::from_secs(15))
        .send()
        .await
        .unwrap();
    assert_eq!(resp_mistral.status(), 200, "Mistral failed: {}", resp_mistral.status());

    mock_gemini.assert_async().await;
    mock_cohere.assert_async().await;
    mock_mistral.assert_async().await;
}

/// PATCH method passes through the proxy with the correct method verb.
#[tokio::test]
async fn binary_patch_method_passes_through() {
    let (_nats_container, nats_port) = start_nats().await;

    let mock_server = httpmock::MockServer::start_async().await;
    let _mock = mock_server
        .mock_async(|when, then| {
            when.method(httpmock::Method::PATCH)
                .path("/v1/messages/msg-abc");
            then.status(200)
                .header("content-type", "application/json")
                .body(r#"{"id":"msg-abc","patched":true}"#);
        })
        .await;

    let proxy_port = free_port();
    let token = "tok_anthropic_prod_pat001";

    let _proxy = spawn_proxy(nats_port, proxy_port, &mock_server.base_url());
    wait_for_port(proxy_port, Duration::from_secs(15)).await;

    let _worker = spawn_worker(nats_port, "binary-workers-patch", token, "sk-ant-key-patch");
    tokio::time::sleep(Duration::from_millis(500)).await;

    let resp = reqwest::Client::new()
        .patch(format!(
            "http://127.0.0.1:{}/anthropic/v1/messages/msg-abc",
            proxy_port
        ))
        .header("Authorization", format!("Bearer {}", token))
        .header("Content-Type", "application/json")
        .body(r#"{"metadata":{"tag":"updated"}}"#)
        .timeout(Duration::from_secs(15))
        .send()
        .await
        .unwrap();

    assert_eq!(resp.status(), 200, "Expected 200, got {}", resp.status());
    let body: serde_json::Value = resp.json().await.unwrap();
    assert_eq!(body["patched"], true);
    _mock.assert_async().await;
}

/// Non-200 success status codes (201, 202, 204) are forwarded to the caller
/// unchanged.
#[tokio::test]
async fn binary_non_200_success_status_forwarded() {
    let (_nats_container, nats_port) = start_nats().await;

    let mock_server = httpmock::MockServer::start_async().await;

    // 201 Created — e.g. uploading a file.
    let mock_201 = mock_server
        .mock_async(|when, then| {
            when.method(httpmock::Method::POST).path("/v1/files");
            then.status(201)
                .header("content-type", "application/json")
                .body(r#"{"id":"file-new","object":"file"}"#);
        })
        .await;

    // 202 Accepted — e.g. async batch job.
    let mock_202 = mock_server
        .mock_async(|when, then| {
            when.method(httpmock::Method::POST).path("/v1/batches");
            then.status(202)
                .header("content-type", "application/json")
                .body(r#"{"id":"batch-new","status":"processing"}"#);
        })
        .await;

    // 204 No Content — e.g. deleting with no body in response.
    let mock_204 = mock_server
        .mock_async(|when, then| {
            when.method(httpmock::Method::DELETE).path("/v1/sessions/sess-abc");
            then.status(204);
        })
        .await;

    let proxy_port = free_port();
    let token = "tok_openai_prod_2xx001";

    let _proxy = spawn_proxy(nats_port, proxy_port, &mock_server.base_url());
    wait_for_port(proxy_port, Duration::from_secs(15)).await;

    let _worker = spawn_worker(nats_port, "binary-workers-2xx", token, "sk-openai-key-2xx");
    tokio::time::sleep(Duration::from_millis(500)).await;

    let client = reqwest::Client::new();
    let base = format!("http://127.0.0.1:{}/openai", proxy_port);

    let r201 = client.post(format!("{}/v1/files", base))
        .header("Authorization", format!("Bearer {}", token))
        .header("Content-Type", "application/json")
        .body("{}")
        .timeout(Duration::from_secs(15))
        .send().await.unwrap();
    assert_eq!(r201.status(), 201, "Expected 201, got {}", r201.status());

    let r202 = client.post(format!("{}/v1/batches", base))
        .header("Authorization", format!("Bearer {}", token))
        .header("Content-Type", "application/json")
        .body("{}")
        .timeout(Duration::from_secs(15))
        .send().await.unwrap();
    assert_eq!(r202.status(), 202, "Expected 202, got {}", r202.status());

    let r204 = client.delete(format!("{}/v1/sessions/sess-abc", base))
        .header("Authorization", format!("Bearer {}", token))
        .timeout(Duration::from_secs(15))
        .send().await.unwrap();
    assert_eq!(r204.status(), 204, "Expected 204, got {}", r204.status());

    mock_201.assert_async().await;
    mock_202.assert_async().await;
    mock_204.assert_async().await;
}

/// Large response body (~100 KB) from the AI provider passes through the
/// worker and proxy to the caller intact.
#[tokio::test]
async fn binary_large_response_body_passes_through() {
    let (_nats_container, nats_port) = start_nats().await;

    // Build a ~100 KB JSON response body.
    let large_text = "y".repeat(100_000);
    let large_response = format!(
        r#"{{"id":"msg_large_resp","content":[{{"type":"text","text":"{}"}}]}}"#,
        large_text
    );
    let expected_len = large_response.len();

    let mock_server = httpmock::MockServer::start_async().await;
    let _mock = mock_server
        .mock_async(|when, then| {
            when.method(httpmock::Method::POST).path("/v1/messages");
            then.status(200)
                .header("content-type", "application/json")
                .body(large_response.clone());
        })
        .await;

    let proxy_port = free_port();
    let token = "tok_anthropic_prod_lrb001";

    let _proxy = spawn_proxy(nats_port, proxy_port, &mock_server.base_url());
    wait_for_port(proxy_port, Duration::from_secs(15)).await;

    let _worker = spawn_worker(nats_port, "binary-workers-lrb", token, "sk-ant-key-lrb");
    tokio::time::sleep(Duration::from_millis(500)).await;

    let resp = reqwest::Client::new()
        .post(format!(
            "http://127.0.0.1:{}/anthropic/v1/messages",
            proxy_port
        ))
        .header("Authorization", format!("Bearer {}", token))
        .header("Content-Type", "application/json")
        .body("{}")
        .timeout(Duration::from_secs(20))
        .send()
        .await
        .unwrap();

    assert_eq!(resp.status(), 200, "Expected 200, got {}", resp.status());
    let body_bytes = resp.bytes().await.unwrap();
    assert_eq!(
        body_bytes.len(),
        expected_len,
        "Response body length mismatch: got {} bytes, expected {}",
        body_bytes.len(),
        expected_len
    );
    let body: serde_json::Value = serde_json::from_slice(&body_bytes).unwrap();
    assert_eq!(body["id"], "msg_large_resp");
}

/// The worker adds an `X-Request-Id` header (idempotency key) to every
/// request forwarded to the AI provider.  The value must be a valid UUID and
/// must differ between requests.
///
/// httpmock doesn't expose captured header values, so this test spins up a
/// minimal axum server that records the X-Request-Id from each incoming call.
#[tokio::test]
async fn binary_idempotency_key_sent_to_provider() {
    use axum::{extract::State, http::Request, body::Body as AxumBody};
    use std::sync::{Arc, Mutex};

    let (_nats_container, nats_port) = start_nats().await;

    // Shared store for captured X-Request-Id values.
    let captured: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
    let captured_for_handler = captured.clone();

    // Minimal axum "AI provider" server that records the idempotency key.
    let capture_app = axum::Router::new()
        .route(
            "/v1/messages",
            axum::routing::post(
                move |State(ids): State<Arc<Mutex<Vec<String>>>>, req: Request<AxumBody>| {
                    let ids = ids.clone();
                    async move {
                        if let Some(v) = req.headers().get("x-request-id") {
                            ids.lock().unwrap().push(v.to_str().unwrap().to_string());
                        }
                        axum::response::Response::builder()
                            .status(200)
                            .header("content-type", "application/json")
                            .body(AxumBody::from(r#"{"id":"msg_idem"}"#))
                            .unwrap()
                    }
                },
            ),
        )
        .with_state(captured_for_handler);

    let provider_port = free_port();
    let provider_listener =
        tokio::net::TcpListener::bind(format!("127.0.0.1:{}", provider_port))
            .await
            .unwrap();
    tokio::spawn(async move {
        axum::serve(provider_listener, capture_app).await.unwrap();
    });
    wait_for_port(provider_port, Duration::from_secs(5)).await;

    let provider_base = format!("http://127.0.0.1:{}", provider_port);
    let proxy_port = free_port();
    let token = "tok_anthropic_prod_idem01";

    let _proxy = spawn_proxy(nats_port, proxy_port, &provider_base);
    wait_for_port(proxy_port, Duration::from_secs(15)).await;

    let _worker = spawn_worker(nats_port, "binary-workers-idem", token, "sk-ant-key-idem");
    tokio::time::sleep(Duration::from_millis(500)).await;

    // Send the same logical request twice.
    let client = reqwest::Client::new();
    for _ in 0..2 {
        let resp = client
            .post(format!("http://127.0.0.1:{}/anthropic/v1/messages", proxy_port))
            .header("Authorization", format!("Bearer {}", token))
            .header("Content-Type", "application/json")
            .body("{}")
            .timeout(Duration::from_secs(15))
            .send()
            .await
            .unwrap();
        assert_eq!(resp.status(), 200);
    }

    let ids = captured.lock().unwrap().clone();
    assert_eq!(ids.len(), 2, "Provider should have been called twice");

    // Each ID must be a valid UUID.
    uuid::Uuid::parse_str(&ids[0])
        .expect("X-Request-Id in first request is not a valid UUID");
    uuid::Uuid::parse_str(&ids[1])
        .expect("X-Request-Id in second request is not a valid UUID");

    // The two requests must carry different idempotency keys.
    assert_ne!(ids[0], ids[1], "Each request must have a unique X-Request-Id");
}

/// 429 Too Many Requests is treated as a 4xx client error and forwarded
/// immediately without any retries.
#[tokio::test]
async fn binary_429_rate_limit_is_not_retried() {
    let (_nats_container, nats_port) = start_nats().await;

    let mock_server = httpmock::MockServer::start_async().await;
    let mock = mock_server
        .mock_async(|when, then| {
            when.method(httpmock::Method::POST).path("/v1/messages");
            then.status(429)
                .header("content-type", "application/json")
                .header("retry-after", "60")
                .body(r#"{"error":{"type":"rate_limit_error","message":"Rate limit exceeded"}}"#);
        })
        .await;

    let proxy_port = free_port();
    let token = "tok_anthropic_prod_rl001";

    let _proxy = spawn_proxy(nats_port, proxy_port, &mock_server.base_url());
    wait_for_port(proxy_port, Duration::from_secs(15)).await;

    let _worker = spawn_worker(nats_port, "binary-workers-ratelimit", token, "sk-ant-key-rl");
    tokio::time::sleep(Duration::from_millis(500)).await;

    let resp = reqwest::Client::new()
        .post(format!(
            "http://127.0.0.1:{}/anthropic/v1/messages",
            proxy_port
        ))
        .header("Authorization", format!("Bearer {}", token))
        .header("Content-Type", "application/json")
        .body("{}")
        .timeout(Duration::from_secs(15))
        .send()
        .await
        .unwrap();

    // 429 forwarded as-is.
    assert_eq!(
        resp.status(),
        429,
        "Expected 429 to be forwarded, got {}",
        resp.status()
    );
    // retry-after header forwarded to caller.
    assert_eq!(
        resp.headers().get("retry-after").and_then(|v| v.to_str().ok()),
        Some("60"),
        "retry-after header should be forwarded"
    );
    // Provider called exactly once — no retry on 429.
    assert_eq!(mock.hits(), 1, "429 must not trigger retries");
}

/// HEAD method passes through the proxy.  The provider returns 200 with no
/// body; the proxy forwards the status and headers correctly.
#[tokio::test]
async fn binary_head_method_passes_through() {
    let (_nats_container, nats_port) = start_nats().await;

    let mock_server = httpmock::MockServer::start_async().await;
    let _mock = mock_server
        .mock_async(|when, then| {
            when.method(httpmock::Method::HEAD).path("/v1/models");
            then.status(200)
                .header("x-model-count", "42");
        })
        .await;

    let proxy_port = free_port();
    let token = "tok_anthropic_prod_head01";

    let _proxy = spawn_proxy(nats_port, proxy_port, &mock_server.base_url());
    wait_for_port(proxy_port, Duration::from_secs(15)).await;

    let _worker = spawn_worker(nats_port, "binary-workers-head", token, "sk-ant-key-head");
    tokio::time::sleep(Duration::from_millis(500)).await;

    let resp = reqwest::Client::new()
        .head(format!(
            "http://127.0.0.1:{}/anthropic/v1/models",
            proxy_port
        ))
        .header("Authorization", format!("Bearer {}", token))
        .timeout(Duration::from_secs(15))
        .send()
        .await
        .unwrap();

    assert_eq!(resp.status(), 200, "HEAD should return 200, got {}", resp.status());
    assert_eq!(
        resp.headers().get("x-model-count").and_then(|v| v.to_str().ok()),
        Some("42"),
        "x-model-count header should be forwarded"
    );
    _mock.assert_async().await;
}

/// Two proxy instances connected to the same NATS cluster both serve requests
/// correctly.  A single worker processes messages from either proxy — the
/// correlation IDs ensure replies are routed back to the originating proxy.
#[tokio::test]
async fn binary_two_proxy_instances_both_serve_requests() {
    let (_nats_container, nats_port) = start_nats().await;

    let mock_server = httpmock::MockServer::start_async().await;
    let _mock = mock_server
        .mock_async(|when, then| {
            when.method(httpmock::Method::POST).path("/v1/messages");
            then.status(200)
                .header("content-type", "application/json")
                .body(r#"{"id":"msg_multi_proxy"}"#);
        })
        .await;

    let proxy_port_a = free_port();
    let proxy_port_b = free_port();
    let token = "tok_anthropic_prod_mp001";

    let _proxy_a = spawn_proxy(nats_port, proxy_port_a, &mock_server.base_url());
    let _proxy_b = spawn_proxy(nats_port, proxy_port_b, &mock_server.base_url());
    wait_for_port(proxy_port_a, Duration::from_secs(15)).await;
    wait_for_port(proxy_port_b, Duration::from_secs(15)).await;

    let _worker = spawn_worker(nats_port, "binary-workers-multiproxy", token, "sk-ant-key-mp");
    tokio::time::sleep(Duration::from_millis(500)).await;

    let client = reqwest::Client::new();

    // Send requests to each proxy in parallel.
    let handles: Vec<_> = [proxy_port_a, proxy_port_b, proxy_port_a, proxy_port_b]
        .iter()
        .map(|&port| {
            let c = client.clone();
            let tok = token.to_string();
            tokio::spawn(async move {
                c.post(format!("http://127.0.0.1:{}/anthropic/v1/messages", port))
                    .header("Authorization", format!("Bearer {}", tok))
                    .header("Content-Type", "application/json")
                    .body("{}")
                    .timeout(Duration::from_secs(20))
                    .send()
                    .await
                    .unwrap()
            })
        })
        .collect();

    let results = join_all(handles).await;
    for result in results {
        let resp = result.unwrap();
        assert_eq!(resp.status(), 200, "Expected 200, got {}", resp.status());
        let body: serde_json::Value = resp.json().await.unwrap();
        assert_eq!(body["id"], "msg_multi_proxy");
    }

    // All 4 requests reached the provider.
    assert_eq!(_mock.hits(), 4, "All 4 requests should have reached the provider");
}

/// NATS brief disconnect: pause the NATS container for ~1 second, unpause it,
/// then verify the proxy and worker reconnect and continue serving requests.
#[tokio::test]
async fn binary_nats_reconnection_after_brief_disconnect() {
    let (nats_container, nats_port) = start_nats().await;

    let mock_server = httpmock::MockServer::start_async().await;
    let _mock = mock_server
        .mock_async(|when, then| {
            when.method(httpmock::Method::POST).path("/v1/messages");
            then.status(200)
                .header("content-type", "application/json")
                .body(r#"{"id":"msg_reconnect"}"#);
        })
        .await;

    let proxy_port = free_port();
    let token = "tok_anthropic_prod_rec001";

    let _proxy = spawn_proxy(nats_port, proxy_port, &mock_server.base_url());
    wait_for_port(proxy_port, Duration::from_secs(15)).await;

    let _worker = spawn_worker(nats_port, "binary-workers-reconnect", token, "sk-ant-key-rec");
    tokio::time::sleep(Duration::from_millis(500)).await;

    // Verify pipeline works before disconnect.
    let before = reqwest::Client::new()
        .post(format!("http://127.0.0.1:{}/anthropic/v1/messages", proxy_port))
        .header("Authorization", format!("Bearer {}", token))
        .header("Content-Type", "application/json")
        .body("{}")
        .timeout(Duration::from_secs(15))
        .send()
        .await
        .unwrap();
    assert_eq!(before.status(), 200, "Pre-disconnect request failed");

    // Pause NATS container (~1 second).
    let container_id = nats_container.id();
    std::process::Command::new("docker")
        .args(["pause", container_id])
        .status()
        .expect("docker pause failed — is Docker available?");

    tokio::time::sleep(Duration::from_millis(1_000)).await;

    std::process::Command::new("docker")
        .args(["unpause", container_id])
        .status()
        .expect("docker unpause failed");

    // Allow clients to reconnect.
    tokio::time::sleep(Duration::from_millis(2_000)).await;

    // Pipeline must work again after reconnection.
    let after = reqwest::Client::new()
        .post(format!("http://127.0.0.1:{}/anthropic/v1/messages", proxy_port))
        .header("Authorization", format!("Bearer {}", token))
        .header("Content-Type", "application/json")
        .body("{}")
        .timeout(Duration::from_secs(20))
        .send()
        .await
        .unwrap();
    assert_eq!(after.status(), 200, "Post-reconnect request failed");

    assert_eq!(_mock.hits(), 2, "Both pre- and post-reconnect requests should reach the provider");
}

/// Binary response body (e.g. audio or image generation) passes through the
/// proxy and worker as raw bytes without corruption.
#[tokio::test]
async fn binary_binary_response_body_passes_through() {
    let (_nats_container, nats_port) = start_nats().await;

    // Fake PNG: valid PNG magic bytes + arbitrary payload.
    let png_magic: &[u8] = &[0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A];
    let payload: Vec<u8> = png_magic
        .iter()
        .copied()
        .chain((0u8..=255).cycle().take(4096))
        .collect();
    let expected = payload.clone();

    let mock_server = httpmock::MockServer::start_async().await;
    let _mock = mock_server
        .mock_async(move |when, then| {
            when.method(httpmock::Method::POST).path("/v1/audio/speech");
            then.status(200)
                .header("content-type", "image/png")
                .body(payload.clone());
        })
        .await;

    let proxy_port = free_port();
    let token = "tok_openai_prod_bin001";

    let _proxy = spawn_proxy(nats_port, proxy_port, &mock_server.base_url());
    wait_for_port(proxy_port, Duration::from_secs(15)).await;

    let _worker = spawn_worker(nats_port, "binary-workers-binresp", token, "sk-openai-key-bin");
    tokio::time::sleep(Duration::from_millis(500)).await;

    let resp = reqwest::Client::new()
        .post(format!(
            "http://127.0.0.1:{}/openai/v1/audio/speech",
            proxy_port
        ))
        .header("Authorization", format!("Bearer {}", token))
        .header("Content-Type", "application/json")
        .body(r#"{"model":"tts-1","input":"hello","voice":"alloy"}"#)
        .timeout(Duration::from_secs(15))
        .send()
        .await
        .unwrap();

    assert_eq!(resp.status(), 200, "Expected 200, got {}", resp.status());
    assert_eq!(
        resp.headers()
            .get("content-type")
            .and_then(|v| v.to_str().ok()),
        Some("image/png"),
        "content-type should be forwarded"
    );
    let body_bytes = resp.bytes().await.unwrap();
    assert_eq!(
        body_bytes.as_ref(),
        expected.as_slice(),
        "Binary response body corrupted in transit"
    );
}

/// The worker must resolve the token regardless of whether the caller sends
/// `Authorization` or `authorization` (HTTP header names are case-insensitive).
#[tokio::test]
async fn binary_authorization_header_case_insensitive() {
    let (_nats_container, nats_port) = start_nats().await;

    let mock_server = httpmock::MockServer::start_async().await;
    let _mock = mock_server
        .mock_async(|when, then| {
            when.method(httpmock::Method::POST).path("/v1/messages");
            then.status(200)
                .header("content-type", "application/json")
                .body(r#"{"id":"msg_case_ok"}"#);
        })
        .await;

    let proxy_port = free_port();
    let token = "tok_anthropic_prod_ci001";

    let _proxy = spawn_proxy(nats_port, proxy_port, &mock_server.base_url());
    wait_for_port(proxy_port, Duration::from_secs(15)).await;

    let _worker = spawn_worker(nats_port, "binary-workers-case", token, "sk-ant-key-case");
    tokio::time::sleep(Duration::from_millis(500)).await;

    let client = reqwest::Client::new();

    // Lowercase "authorization".
    let resp_lower = client
        .post(format!("http://127.0.0.1:{}/anthropic/v1/messages", proxy_port))
        .header("authorization", format!("Bearer {}", token))
        .header("content-type", "application/json")
        .body("{}")
        .timeout(Duration::from_secs(15))
        .send()
        .await
        .unwrap();
    assert_eq!(resp_lower.status(), 200, "lowercase authorization failed: {}", resp_lower.status());

    // All-caps "AUTHORIZATION".
    let resp_upper = client
        .post(format!("http://127.0.0.1:{}/anthropic/v1/messages", proxy_port))
        .header("AUTHORIZATION", format!("Bearer {}", token))
        .header("content-type", "application/json")
        .body("{}")
        .timeout(Duration::from_secs(15))
        .send()
        .await
        .unwrap();
    assert_eq!(resp_upper.status(), 200, "AUTHORIZATION failed: {}", resp_upper.status());

    assert_eq!(_mock.hits(), 2, "Both requests should have reached the provider");
}

/// Two successive NATS outages: after each pause/unpause cycle the proxy and
/// worker reconnect and continue serving requests normally.
#[tokio::test]
async fn binary_multiple_nats_reconnections() {
    let (nats_container, nats_port) = start_nats().await;

    let mock_server = httpmock::MockServer::start_async().await;
    let _mock = mock_server
        .mock_async(|when, then| {
            when.method(httpmock::Method::POST).path("/v1/messages");
            then.status(200)
                .header("content-type", "application/json")
                .body(r#"{"id":"msg_multi_rec"}"#);
        })
        .await;

    let proxy_port = free_port();
    let token = "tok_anthropic_prod_mr001";

    let _proxy = spawn_proxy(nats_port, proxy_port, &mock_server.base_url());
    wait_for_port(proxy_port, Duration::from_secs(15)).await;

    let _worker = spawn_worker(nats_port, "binary-workers-multirec", token, "sk-ant-key-mr");
    tokio::time::sleep(Duration::from_millis(500)).await;

    let client = reqwest::Client::new();
    let container_id = nats_container.id().to_string();

    // Baseline request.
    let r0 = client
        .post(format!("http://127.0.0.1:{}/anthropic/v1/messages", proxy_port))
        .header("Authorization", format!("Bearer {}", token))
        .header("Content-Type", "application/json")
        .body("{}").timeout(Duration::from_secs(15)).send().await.unwrap();
    assert_eq!(r0.status(), 200, "Baseline request failed");

    // First disconnect.
    std::process::Command::new("docker").args(["pause", &container_id]).status().unwrap();
    tokio::time::sleep(Duration::from_millis(800)).await;
    std::process::Command::new("docker").args(["unpause", &container_id]).status().unwrap();
    tokio::time::sleep(Duration::from_millis(2_000)).await;

    let r1 = client
        .post(format!("http://127.0.0.1:{}/anthropic/v1/messages", proxy_port))
        .header("Authorization", format!("Bearer {}", token))
        .header("Content-Type", "application/json")
        .body("{}").timeout(Duration::from_secs(20)).send().await.unwrap();
    assert_eq!(r1.status(), 200, "Post-first-reconnect request failed");

    // Second disconnect.
    std::process::Command::new("docker").args(["pause", &container_id]).status().unwrap();
    tokio::time::sleep(Duration::from_millis(800)).await;
    std::process::Command::new("docker").args(["unpause", &container_id]).status().unwrap();
    tokio::time::sleep(Duration::from_millis(2_000)).await;

    let r2 = client
        .post(format!("http://127.0.0.1:{}/anthropic/v1/messages", proxy_port))
        .header("Authorization", format!("Bearer {}", token))
        .header("Content-Type", "application/json")
        .body("{}").timeout(Duration::from_secs(20)).send().await.unwrap();
    assert_eq!(r2.status(), 200, "Post-second-reconnect request failed");

    assert_eq!(_mock.hits(), 3, "All 3 requests should have reached the provider");
}

/// OPTIONS request (CORS preflight) passes through the full pipeline when
/// the AI provider handles it.
#[tokio::test]
async fn binary_options_request_passes_through() {
    let (_nats_container, nats_port) = start_nats().await;

    let mock_server = httpmock::MockServer::start_async().await;
    let _mock = mock_server
        .mock_async(|when, then| {
            when.method(httpmock::Method::OPTIONS).path("/v1/messages");
            then.status(204)
                .header("access-control-allow-origin", "*")
                .header("access-control-allow-methods", "GET, POST, OPTIONS")
                .header("access-control-allow-headers", "Authorization, Content-Type");
        })
        .await;

    let proxy_port = free_port();
    let token = "tok_anthropic_prod_opt001";

    let _proxy = spawn_proxy(nats_port, proxy_port, &mock_server.base_url());
    wait_for_port(proxy_port, Duration::from_secs(15)).await;

    let _worker = spawn_worker(nats_port, "binary-workers-options", token, "sk-ant-key-opt");
    tokio::time::sleep(Duration::from_millis(500)).await;

    let resp = reqwest::Client::new()
        .request(
            reqwest::Method::OPTIONS,
            format!("http://127.0.0.1:{}/anthropic/v1/messages", proxy_port),
        )
        .header("Authorization", format!("Bearer {}", token))
        .header("Origin", "https://my-app.example.com")
        .header("Access-Control-Request-Method", "POST")
        .timeout(Duration::from_secs(15))
        .send()
        .await
        .unwrap();

    assert_eq!(resp.status(), 204, "OPTIONS should return 204, got {}", resp.status());
    assert_eq!(
        resp.headers()
            .get("access-control-allow-origin")
            .and_then(|v| v.to_str().ok()),
        Some("*"),
        "CORS header should be forwarded"
    );
    _mock.assert_async().await;
}

// ── Custom-prefix helpers ──────────────────────────────────────────────────

fn spawn_proxy_with_prefix(
    nats_port: u16,
    proxy_port: u16,
    mock_base_url: &str,
    prefix: &str,
) -> Child {
    Command::new(env!("CARGO_BIN_EXE_proxy"))
        .env("NATS_URL", format!("localhost:{}", nats_port))
        .env("PROXY_PORT", proxy_port.to_string())
        .env("PROXY_WORKER_TIMEOUT_SECS", "15")
        .env("PROXY_BASE_URL_OVERRIDE", mock_base_url)
        .env("PROXY_PREFIX", prefix)
        .env("RUST_LOG", "warn")
        .kill_on_drop(true)
        .spawn()
        .expect("Failed to spawn proxy binary")
}

fn spawn_worker_with_prefix(
    nats_port: u16,
    consumer_name: &str,
    token: &str,
    real_key: &str,
    prefix: &str,
) -> Child {
    Command::new(env!("CARGO_BIN_EXE_worker"))
        .env("NATS_URL", format!("localhost:{}", nats_port))
        .env("WORKER_CONSUMER_NAME", consumer_name)
        .env("PROXY_PREFIX", prefix)
        .env(format!("VAULT_TOKEN_{}", token), real_key)
        .env("RUST_LOG", "warn")
        .kill_on_drop(true)
        .spawn()
        .expect("Failed to spawn worker binary")
}

// ── Tests ──────────────────────────────────────────────────────────────────

/// Custom PROXY_PREFIX: both proxy and worker must use the same prefix for
/// their NATS subjects to align.  A mismatch would cause the proxy to time
/// out because the worker listens on a different subject.
#[tokio::test]
async fn binary_custom_prefix_routes_correctly() {
    let (_nats_container, nats_port) = start_nats().await;

    let mock_server = httpmock::MockServer::start_async().await;
    let _mock = mock_server
        .mock_async(|when, then| {
            when.method(httpmock::Method::POST).path("/v1/messages");
            then.status(200)
                .header("content-type", "application/json")
                .body(r#"{"id":"msg_custom_prefix"}"#);
        })
        .await;

    let proxy_port = free_port();
    let token = "tok_anthropic_prod_pfx001";
    let prefix = "mycompany";

    let _proxy = spawn_proxy_with_prefix(nats_port, proxy_port, &mock_server.base_url(), prefix);
    wait_for_port(proxy_port, Duration::from_secs(15)).await;

    let _worker =
        spawn_worker_with_prefix(nats_port, "pfx-workers", token, "sk-ant-key-pfx", prefix);
    tokio::time::sleep(Duration::from_millis(500)).await;

    let resp = reqwest::Client::new()
        .post(format!(
            "http://127.0.0.1:{}/anthropic/v1/messages",
            proxy_port
        ))
        .header("Authorization", format!("Bearer {}", token))
        .header("Content-Type", "application/json")
        .body("{}")
        .timeout(Duration::from_secs(15))
        .send()
        .await
        .unwrap();

    assert_eq!(resp.status(), 200, "Custom prefix request failed: {}", resp.status());
    let body: serde_json::Value = resp.json().await.unwrap();
    assert_eq!(body["id"], "msg_custom_prefix");
    _mock.assert_async().await;
}

/// Prefix mismatch: proxy uses prefix "alpha", worker uses prefix "beta".
/// Each prefix now gets its own stream (PROXY_REQUESTS_ALPHA vs
/// PROXY_REQUESTS_BETA), so the worker never sees the proxy's message and
/// the proxy returns 504 Timeout.
#[tokio::test]
async fn binary_prefix_mismatch_causes_timeout() {
    let (_nats_container, nats_port) = start_nats().await;

    let mock_server = httpmock::MockServer::start_async().await;
    let proxy_port = free_port();
    let token = "tok_anthropic_prod_pfx002";

    // Proxy on prefix "alpha" with 2s timeout.
    let _proxy = Command::new(env!("CARGO_BIN_EXE_proxy"))
        .env("NATS_URL", format!("localhost:{}", nats_port))
        .env("PROXY_PORT", proxy_port.to_string())
        .env("PROXY_WORKER_TIMEOUT_SECS", "2")
        .env("PROXY_BASE_URL_OVERRIDE", &mock_server.base_url())
        .env("PROXY_PREFIX", "alpha")
        .env("RUST_LOG", "warn")
        .kill_on_drop(true)
        .spawn()
        .unwrap();
    wait_for_port(proxy_port, Duration::from_secs(15)).await;

    // Worker on prefix "beta" → listens on PROXY_REQUESTS_BETA, never sees
    // messages published to PROXY_REQUESTS_ALPHA.
    let _worker =
        spawn_worker_with_prefix(nats_port, "pfx-mismatch-workers", token, "sk-ant-key", "beta");
    tokio::time::sleep(Duration::from_millis(500)).await;

    let resp = reqwest::Client::new()
        .post(format!(
            "http://127.0.0.1:{}/anthropic/v1/messages",
            proxy_port
        ))
        .header("Authorization", format!("Bearer {}", token))
        .header("Content-Type", "application/json")
        .body("{}")
        .timeout(Duration::from_secs(10))
        .send()
        .await
        .unwrap();

    assert_eq!(
        resp.status(),
        504,
        "Prefix mismatch should time out with 504, got {}",
        resp.status()
    );
    // AI provider was never contacted (no mock registered → any hit would panic).
}

/// Corrupted JetStream payload: the test publishes invalid JSON directly to
/// the stream.  The worker must NAK it (or skip it) and continue processing
/// the next valid request without crashing.
#[tokio::test]
async fn binary_invalid_jetstream_payload_nacked_worker_continues() {
    let (_nats_container, nats_port) = start_nats().await;

    let mock_server = httpmock::MockServer::start_async().await;
    let _mock = mock_server
        .mock_async(|when, then| {
            when.method(httpmock::Method::POST).path("/v1/messages");
            then.status(200)
                .header("content-type", "application/json")
                .body(r#"{"id":"msg_after_nak"}"#);
        })
        .await;

    let proxy_port = free_port();
    let token = "tok_anthropic_prod_nak001";

    let _proxy = spawn_proxy(nats_port, proxy_port, &mock_server.base_url());
    wait_for_port(proxy_port, Duration::from_secs(15)).await;

    let _worker =
        spawn_worker(nats_port, "binary-workers-nak", token, "sk-ant-key-nak");
    tokio::time::sleep(Duration::from_millis(500)).await;

    // Connect to NATS from the test and publish garbage directly to the stream.
    let nats = async_nats::connect(format!("localhost:{}", nats_port))
        .await
        .unwrap();
    let js = async_nats::jetstream::new(nats);
    js.publish(
        "trogon.proxy.http.outbound",
        bytes::Bytes::from(b"NOT VALID JSON {{{".as_ref()),
    )
    .await
    .unwrap()
    .await
    .unwrap(); // wait for ack from JetStream server

    // Give the worker time to receive and NAK the bad message.
    tokio::time::sleep(Duration::from_millis(800)).await;

    // Worker must still be alive and process a valid request.
    let resp = reqwest::Client::new()
        .post(format!(
            "http://127.0.0.1:{}/anthropic/v1/messages",
            proxy_port
        ))
        .header("Authorization", format!("Bearer {}", token))
        .header("Content-Type", "application/json")
        .body("{}")
        .timeout(Duration::from_secs(15))
        .send()
        .await
        .unwrap();

    assert_eq!(
        resp.status(),
        200,
        "Worker should continue serving requests after NAKing a bad payload"
    );
    _mock.assert_async().await;
}

/// Effective token revocation: a request succeeds when the token is in the
/// vault; after the worker restarts WITHOUT the token, the same request fails.
/// (Runtime revocation is not supported by the binary — the vault is seeded
/// once at startup from env vars.  Restarting without the token is the
/// operational equivalent of revocation.)
#[tokio::test]
async fn binary_token_revocation_via_worker_restart() {
    let (_nats_container, nats_port) = start_nats().await;

    let mock_server = httpmock::MockServer::start_async().await;
    let _should_not_be_called_after = mock_server
        .mock_async(|when, then| {
            when.any_request();
            then.status(200)
                .header("content-type", "application/json")
                .body(r#"{"id":"msg_pre_revoke"}"#);
        })
        .await;

    let proxy_port = free_port();
    let token = "tok_anthropic_prod_rev001";

    let _proxy = spawn_proxy(nats_port, proxy_port, &mock_server.base_url());
    wait_for_port(proxy_port, Duration::from_secs(15)).await;

    // Worker 1: token is present.
    let mut worker_1 = spawn_worker(nats_port, "binary-workers-rev", token, "sk-ant-key-rev");
    tokio::time::sleep(Duration::from_millis(500)).await;

    let resp_before = reqwest::Client::new()
        .post(format!(
            "http://127.0.0.1:{}/anthropic/v1/messages",
            proxy_port
        ))
        .header("Authorization", format!("Bearer {}", token))
        .header("Content-Type", "application/json")
        .body("{}")
        .timeout(Duration::from_secs(15))
        .send()
        .await
        .unwrap();
    assert_eq!(resp_before.status(), 200, "Pre-revocation request should succeed");

    // Revoke: kill worker 1, restart WITHOUT the token.
    worker_1.kill().await.unwrap();
    tokio::time::sleep(Duration::from_millis(200)).await;

    // Worker 2: empty vault (token not seeded).
    let _worker_2 = Command::new(env!("CARGO_BIN_EXE_worker"))
        .env("NATS_URL", format!("localhost:{}", nats_port))
        .env("WORKER_CONSUMER_NAME", "binary-workers-rev")
        .env("RUST_LOG", "warn")
        .kill_on_drop(true)
        .spawn()
        .unwrap();
    tokio::time::sleep(Duration::from_millis(500)).await;

    let resp_after = reqwest::Client::new()
        .post(format!(
            "http://127.0.0.1:{}/anthropic/v1/messages",
            proxy_port
        ))
        .header("Authorization", format!("Bearer {}", token))
        .header("Content-Type", "application/json")
        .body("{}")
        .timeout(Duration::from_secs(15))
        .send()
        .await
        .unwrap();

    assert!(
        resp_after.status().is_server_error(),
        "Post-revocation request should fail with 5xx, got {}",
        resp_after.status()
    );
    // AI provider was called once (pre-revocation) and not after.
    assert_eq!(
        _should_not_be_called_after.hits(),
        1,
        "AI provider should only have been called once (before revocation)"
    );
}

/// Gap 1 — ProxyError::Deserialize → 500.
///
/// A rogue NATS actor publishes malformed JSON to the Core NATS reply subject
/// that the proxy is subscribed to.  The proxy fails to deserialize the reply
/// and must return HTTP 500.
///
/// This is the only binary-mode test for the `ProxyError::Deserialize` path in
/// proxy.rs (previously only reachable via unit tests).
#[tokio::test]
async fn binary_malformed_worker_reply_returns_500() {
    let (_nats_container, nats_port) = start_nats().await;

    // No real mock server needed: the bad worker publishes before any HTTP call
    // to the AI provider is made.  Point the override at an unused address.
    let proxy_port = free_port();
    let _proxy = spawn_proxy(nats_port, proxy_port, "http://127.0.0.1:1");
    wait_for_port(proxy_port, Duration::from_secs(15)).await;

    // Connect to NATS directly and act as a "bad worker".
    let nats = async_nats::connect(format!("localhost:{}", nats_port))
        .await
        .unwrap();
    let jetstream = async_nats::jetstream::new(nats.clone());

    // The proxy creates PROXY_REQUESTS_TROGON during startup (before TCP listen).
    let stream = jetstream.get_stream("PROXY_REQUESTS_TROGON").await.unwrap();
    let consumer = stream
        .get_or_create_consumer(
            "bad-worker-test",
            async_nats::jetstream::consumer::pull::Config {
                durable_name: Some("bad-worker-test".to_string()),
                ack_policy: async_nats::jetstream::consumer::AckPolicy::Explicit,
                ..Default::default()
            },
        )
        .await
        .unwrap();
    let mut messages = consumer.messages().await.unwrap();

    // Send a request to the proxy; it blocks waiting for a Core NATS reply.
    let request_handle = tokio::spawn(async move {
        reqwest::Client::new()
            .post(format!(
                "http://127.0.0.1:{}/anthropic/v1/messages",
                proxy_port
            ))
            .header("Authorization", "Bearer tok_anthropic_prod_bad001")
            .header("Content-Type", "application/json")
            .body("{}")
            .timeout(Duration::from_secs(20))
            .send()
            .await
            .unwrap()
    });

    // Pull the JetStream message the proxy published.
    let msg = tokio::time::timeout(Duration::from_secs(10), messages.next())
        .await
        .expect("Timed out waiting for JetStream message from proxy")
        .unwrap()
        .unwrap();

    // Extract the reply subject embedded in the payload.
    let parsed: serde_json::Value = serde_json::from_slice(&msg.payload).unwrap();
    let reply_to = parsed["reply_to"].as_str().unwrap().to_string();

    // Publish intentionally malformed JSON to the reply subject.
    nats.publish(reply_to, b"NOT VALID JSON {{{".to_vec().into())
        .await
        .unwrap();
    msg.ack().await.unwrap();

    let resp = request_handle.await.unwrap();
    assert_eq!(
        resp.status(),
        500,
        "Proxy must return 500 when it cannot deserialize the worker reply"
    );
}

/// Gap 2 — non-Bearer Authorization scheme rejected in binary mode.
///
/// A client sends `Authorization: Basic ...` instead of `Authorization: Bearer tok_...`.
/// The worker rejects it ("not a Bearer token") and the proxy surfaces that as 502.
///
/// Previously this code path was only exercised by unit tests in worker.rs.
/// This test also verifies Gap 3 for this case: the 502 body contains the
/// human-readable rejection reason.
#[tokio::test]
async fn binary_non_bearer_auth_scheme_returns_502_with_error_body() {
    let (_nats_container, nats_port) = start_nats().await;

    let mock_server = httpmock::MockServer::start_async().await;
    let proxy_port = free_port();

    let _proxy = spawn_proxy(nats_port, proxy_port, &mock_server.base_url());
    wait_for_port(proxy_port, Duration::from_secs(15)).await;

    // Worker is running; the Basic-auth request will be rejected before vault lookup.
    let _worker = spawn_worker(
        nats_port,
        "binary-workers-basic",
        "tok_anthropic_prod_basic001",
        "sk-ant-realkey",
    );
    tokio::time::sleep(Duration::from_millis(500)).await;

    let resp = reqwest::Client::new()
        .post(format!(
            "http://127.0.0.1:{}/anthropic/v1/messages",
            proxy_port
        ))
        .header("Authorization", "Basic dXNlcjpwYXNz") // "user:pass" base64
        .header("Content-Type", "application/json")
        .body("{}")
        .timeout(Duration::from_secs(15))
        .send()
        .await
        .unwrap();

    assert_eq!(
        resp.status(),
        502,
        "Non-Bearer Authorization scheme must be rejected with 502"
    );
    let body = resp.text().await.unwrap();
    assert!(
        body.contains("not a Bearer token"),
        "502 body must explain the rejection reason, got: {:?}",
        body
    );
}

/// Gap 3 — error response body text verification.
///
/// Binary tests previously only asserted HTTP status codes for error cases.
/// This test explicitly verifies that the 502 body forwarded to the caller
/// contains a human-readable description of the failure for each error type:
///
/// - Missing `Authorization` header
/// - Token not present in vault
/// - Real API key supplied instead of a `tok_...` token
#[tokio::test]
async fn binary_error_response_body_describes_the_error() {
    let (_nats_container, nats_port) = start_nats().await;

    let mock_server = httpmock::MockServer::start_async().await;
    let proxy_port = free_port();

    let _proxy = spawn_proxy(nats_port, proxy_port, &mock_server.base_url());
    wait_for_port(proxy_port, Duration::from_secs(15)).await;

    let _worker = spawn_worker(
        nats_port,
        "binary-workers-body",
        "tok_anthropic_prod_body001",
        "sk-ant-realkey",
    );
    tokio::time::sleep(Duration::from_millis(500)).await;

    let client = reqwest::Client::new();
    let base = format!("http://127.0.0.1:{}/anthropic/v1/messages", proxy_port);

    // ── Case 1: missing Authorization header ─────────────────────────────────
    let resp = client
        .post(&base)
        .header("Content-Type", "application/json")
        .body("{}")
        .timeout(Duration::from_secs(15))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 502, "missing auth header must be 502");
    let body = resp.text().await.unwrap();
    assert!(
        body.contains("Missing Authorization"),
        "missing auth body must mention 'Missing Authorization', got: {:?}",
        body
    );

    // ── Case 2: token not registered in vault ────────────────────────────────
    let resp = client
        .post(&base)
        .header("Authorization", "Bearer tok_anthropic_prod_notinvault99")
        .header("Content-Type", "application/json")
        .body("{}")
        .timeout(Duration::from_secs(15))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 502, "unknown token must be 502");
    let body = resp.text().await.unwrap();
    assert!(
        body.contains("not found"),
        "unknown token body must mention 'not found', got: {:?}",
        body
    );

    // ── Case 3: real API key supplied directly (not a tok_ token) ────────────
    let resp = client
        .post(&base)
        .header("Authorization", "Bearer sk-ant-realkey-direct")
        .header("Content-Type", "application/json")
        .body("{}")
        .timeout(Duration::from_secs(15))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 502, "real API key must be 502");
    let body = resp.text().await.unwrap();
    assert!(
        body.contains("does not look like a proxy token"),
        "real key body must explain the rejection, got: {:?}",
        body
    );
}

/// Gap 1 — TRACE HTTP method passes through end-to-end.
///
/// TRACE is the only standard HTTP method not covered by previous tests.
/// The proxy must forward it to the AI provider and return the response.
#[tokio::test]
async fn binary_trace_method_passes_through() {
    let (_nats_container, nats_port) = start_nats().await;

    let mock_server = httpmock::MockServer::start_async().await;
    let ai_mock = mock_server
        .mock_async(|when, then| {
            when.method(httpmock::Method::TRACE).path("/v1/messages");
            then.status(200).body("traced");
        })
        .await;

    let proxy_port = free_port();
    let token = "tok_anthropic_prod_trace01";

    let _proxy = spawn_proxy(nats_port, proxy_port, &mock_server.base_url());
    wait_for_port(proxy_port, Duration::from_secs(15)).await;

    let _worker = spawn_worker(nats_port, "binary-workers-trace", token, "sk-ant-trace-key");
    tokio::time::sleep(Duration::from_millis(500)).await;

    let resp = reqwest::Client::new()
        .request(
            reqwest::Method::TRACE,
            format!("http://127.0.0.1:{}/anthropic/v1/messages", proxy_port),
        )
        .header("Authorization", format!("Bearer {}", token))
        .timeout(Duration::from_secs(15))
        .send()
        .await
        .expect("TRACE request to proxy failed");

    assert_eq!(resp.status(), 200, "TRACE must be forwarded and return 200");
    assert_eq!(resp.text().await.unwrap(), "traced");
    ai_mock.assert_async().await;
}

/// Gap 2 — malformed `tok_` token (invalid id characters) is rejected with 502.
///
/// A token that starts with `tok_` but contains an illegal character in the id
/// segment (e.g. a hyphen) fails `ApiKeyToken::new()` validation in the worker
/// with `TokenError::InvalidIdCharacter`.  The proxy surfaces the "Invalid proxy
/// token" error as a 502.
///
/// Previously this `ApiKeyToken::new` error path was only covered by unit tests.
#[tokio::test]
async fn binary_malformed_tok_token_format_returns_502_with_error_body() {
    let (_nats_container, nats_port) = start_nats().await;

    let mock_server = httpmock::MockServer::start_async().await;
    let proxy_port = free_port();

    let _proxy = spawn_proxy(nats_port, proxy_port, &mock_server.base_url());
    wait_for_port(proxy_port, Duration::from_secs(15)).await;

    let _worker = spawn_worker(
        nats_port,
        "binary-workers-badfmt",
        "tok_anthropic_prod_valid01",
        "sk-ant-realkey",
    );
    tokio::time::sleep(Duration::from_millis(500)).await;

    // Hyphen in the id segment violates [a-zA-Z0-9]+ — ApiKeyToken::new returns Err.
    let resp = reqwest::Client::new()
        .post(format!(
            "http://127.0.0.1:{}/anthropic/v1/messages",
            proxy_port
        ))
        .header("Authorization", "Bearer tok_anthropic_prod_abc-def")
        .header("Content-Type", "application/json")
        .body("{}")
        .timeout(Duration::from_secs(15))
        .send()
        .await
        .unwrap();

    assert_eq!(
        resp.status(),
        502,
        "Malformed tok_ token must be rejected with 502"
    );
    let body = resp.text().await.unwrap();
    assert!(
        body.contains("Invalid proxy token"),
        "502 body must mention 'Invalid proxy token', got: {:?}",
        body
    );
}

/// Gap 3 — invalid `VAULT_TOKEN_*` env var is skipped; worker still handles
/// requests for valid tokens.
///
/// `bin/worker.rs` logs a warning and skips any `VAULT_TOKEN_*` whose key fails
/// `ApiKeyToken::new()` validation, then continues seeding the rest.
/// This test verifies that a worker started with one invalid and one valid
/// `VAULT_TOKEN_*` env var correctly resolves the valid token and rejects the
/// malformed one.
#[tokio::test]
async fn binary_invalid_vault_env_var_skipped_valid_token_still_works() {
    let (_nats_container, nats_port) = start_nats().await;

    let mock_server = httpmock::MockServer::start_async().await;
    let ai_mock = mock_server
        .mock_async(|when, then| {
            when.method(httpmock::Method::POST)
                .path("/v1/messages")
                .header("authorization", "Bearer sk-ant-good-key");
            then.status(200)
                .header("content-type", "application/json")
                .body(r#"{"id":"msg_goodkey"}"#);
        })
        .await;

    let proxy_port = free_port();
    let valid_token = "tok_anthropic_prod_good001";

    let _proxy = spawn_proxy(nats_port, proxy_port, &mock_server.base_url());
    wait_for_port(proxy_port, Duration::from_secs(15)).await;

    // Start worker with:
    //   - one INVALID VAULT_TOKEN_* env var (hyphen in key name → bad ApiKeyToken)
    //   - one VALID VAULT_TOKEN_* env var
    // The worker must skip the invalid one and seed the valid one.
    let _worker = Command::new(env!("CARGO_BIN_EXE_worker"))
        .env("NATS_URL", format!("localhost:{}", nats_port))
        .env("WORKER_CONSUMER_NAME", "binary-workers-invenv")
        // Invalid: hyphen in the token segment — ApiKeyToken::new returns Err.
        .env("VAULT_TOKEN_tok_anthropic_prod_bad-key", "sk-ant-bad")
        // Valid: correct format.
        .env(
            format!("VAULT_TOKEN_{}", valid_token),
            "sk-ant-good-key",
        )
        .env("RUST_LOG", "warn")
        .kill_on_drop(true)
        .spawn()
        .unwrap();
    tokio::time::sleep(Duration::from_millis(500)).await;

    // The valid token must resolve and reach the AI provider.
    let resp = reqwest::Client::new()
        .post(format!(
            "http://127.0.0.1:{}/anthropic/v1/messages",
            proxy_port
        ))
        .header("Authorization", format!("Bearer {}", valid_token))
        .header("Content-Type", "application/json")
        .body("{}")
        .timeout(Duration::from_secs(15))
        .send()
        .await
        .unwrap();

    assert_eq!(
        resp.status(),
        200,
        "Valid token must still resolve after invalid VAULT_TOKEN_* env var is skipped"
    );
    ai_mock.assert_async().await;
}

/// `Authorization: Bearer  tok_...` (two spaces after Bearer) does not match
/// `BEARER_PREFIX = "Bearer "` (one space), so the extracted token starts with
/// a space and fails the `tok_` prefix check → worker returns an error →
/// proxy returns 502.
#[tokio::test]
async fn binary_bearer_extra_whitespace_returns_502() {
    let (_nats_container, nats_port) = start_nats().await;

    let mock_server = httpmock::MockServer::start_async().await;
    let _should_not_be_called = mock_server
        .mock_async(|when, then| {
            when.any_request();
            then.status(200).body("should not reach here");
        })
        .await;

    let proxy_port = free_port();
    let token = "tok_anthropic_prod_wsptest1";

    let _proxy = spawn_proxy(nats_port, proxy_port, &mock_server.base_url());
    wait_for_port(proxy_port, Duration::from_secs(15)).await;

    let _worker = spawn_worker(nats_port, "binary-workers-ws", token, "sk-ant-realkey");
    tokio::time::sleep(Duration::from_millis(500)).await;

    // Two spaces between "Bearer" and the token.
    let resp = reqwest::Client::new()
        .post(format!("http://127.0.0.1:{}/anthropic/v1/messages", proxy_port))
        .header("Authorization", format!("Bearer  {}", token))  // double space
        .header("Content-Type", "application/json")
        .body("{}")
        .timeout(Duration::from_secs(15))
        .send()
        .await
        .unwrap();

    assert_eq!(
        resp.status(),
        502,
        "Double-space Bearer must return 502, got {}",
        resp.status()
    );
    assert_eq!(_should_not_be_called.hits(), 0, "AI provider must not be called");
}

/// Tokens from different environments (prod vs. staging) stored in the same
/// vault resolve to different real keys without cross-contamination.
#[tokio::test]
async fn binary_staging_and_prod_tokens_resolve_independently() {
    let (_nats_container, nats_port) = start_nats().await;

    let mock_server = httpmock::MockServer::start_async().await;

    let mock_prod = mock_server
        .mock_async(|when, then| {
            when.method(httpmock::Method::POST)
                .path("/v1/messages")
                .header("authorization", "Bearer sk-prod-key");
            then.status(200)
                .header("content-type", "application/json")
                .body(r#"{"id":"msg_prod","env":"prod"}"#);
        })
        .await;

    let mock_staging = mock_server
        .mock_async(|when, then| {
            when.method(httpmock::Method::POST)
                .path("/v1/messages")
                .header("authorization", "Bearer sk-staging-key");
            then.status(200)
                .header("content-type", "application/json")
                .body(r#"{"id":"msg_staging","env":"staging"}"#);
        })
        .await;

    let proxy_port = free_port();
    let prod_token = "tok_anthropic_prod_envtest1";
    let staging_token = "tok_anthropic_staging_envtest2";

    let _proxy = spawn_proxy(nats_port, proxy_port, &mock_server.base_url());
    wait_for_port(proxy_port, Duration::from_secs(15)).await;

    // Single worker seeded with both tokens from different environments.
    let _worker = Command::new(env!("CARGO_BIN_EXE_worker"))
        .env("NATS_URL", format!("localhost:{}", nats_port))
        .env("WORKER_CONSUMER_NAME", "binary-workers-envtest")
        .env(format!("VAULT_TOKEN_{}", prod_token), "sk-prod-key")
        .env(format!("VAULT_TOKEN_{}", staging_token), "sk-staging-key")
        .env("RUST_LOG", "warn")
        .kill_on_drop(true)
        .spawn()
        .unwrap();
    tokio::time::sleep(Duration::from_millis(500)).await;

    let client = reqwest::Client::new();

    // Request with prod token → prod key must be used.
    let resp_prod = client
        .post(format!("http://127.0.0.1:{}/anthropic/v1/messages", proxy_port))
        .header("Authorization", format!("Bearer {}", prod_token))
        .header("Content-Type", "application/json")
        .body("{}")
        .timeout(Duration::from_secs(15))
        .send()
        .await
        .unwrap();
    assert_eq!(resp_prod.status(), 200);
    let body_prod: serde_json::Value = resp_prod.json().await.unwrap();
    assert_eq!(body_prod["id"], "msg_prod", "Prod token must use prod key");

    // Request with staging token → staging key must be used.
    let resp_staging = client
        .post(format!("http://127.0.0.1:{}/anthropic/v1/messages", proxy_port))
        .header("Authorization", format!("Bearer {}", staging_token))
        .header("Content-Type", "application/json")
        .body("{}")
        .timeout(Duration::from_secs(15))
        .send()
        .await
        .unwrap();
    assert_eq!(resp_staging.status(), 200);
    let body_staging: serde_json::Value = resp_staging.json().await.unwrap();
    assert_eq!(body_staging["id"], "msg_staging", "Staging token must use staging key");

    mock_prod.assert_async().await;
    mock_staging.assert_async().await;
}

/// When the AI provider returns a Server-Sent Events (SSE) / streaming
/// response, the proxy buffers the entire body before returning it to
/// the caller.
///
/// This documents the current buffering behavior: the caller does NOT
/// receive tokens incrementally — it receives the complete response
/// once the provider finishes.  The body and Content-Type are preserved.
#[tokio::test]
async fn binary_sse_response_buffered_and_returned_completely() {
    let (_nats_container, nats_port) = start_nats().await;

    // Simulate an SSE stream as returned by OpenAI / Anthropic with stream:true.
    let sse_body = concat!(
        "data: {\"type\":\"content_block_delta\",\"delta\":{\"text\":\"Hello\"}}\n\n",
        "data: {\"type\":\"content_block_delta\",\"delta\":{\"text\":\" world\"}}\n\n",
        "data: [DONE]\n\n",
    );

    let mock_server = httpmock::MockServer::start_async().await;
    let ai_mock = mock_server
        .mock_async(|when, then| {
            when.method(httpmock::Method::POST).path("/v1/messages");
            then.status(200)
                .header("content-type", "text/event-stream")
                .header("cache-control", "no-cache")
                .body(sse_body);
        })
        .await;

    let proxy_port = free_port();
    let token = "tok_anthropic_prod_ssetest1";

    let _proxy = spawn_proxy(nats_port, proxy_port, &mock_server.base_url());
    wait_for_port(proxy_port, Duration::from_secs(15)).await;

    let _worker = spawn_worker(nats_port, "binary-workers-sse", token, "sk-ant-realkey");
    tokio::time::sleep(Duration::from_millis(500)).await;

    let resp = reqwest::Client::new()
        .post(format!("http://127.0.0.1:{}/anthropic/v1/messages", proxy_port))
        .header("Authorization", format!("Bearer {}", token))
        .header("Content-Type", "application/json")
        .body(r#"{"model":"claude-3","stream":true,"messages":[]}"#)
        .timeout(Duration::from_secs(20))
        .send()
        .await
        .unwrap();

    assert_eq!(resp.status(), 200, "SSE response must return 200, got {}", resp.status());

    // Proxy buffers the full SSE stream and returns it as a single response body.
    let body = resp.text().await.unwrap();
    assert!(body.contains("Hello"), "First SSE token must be present in buffered body");
    assert!(body.contains("world"), "Second SSE token must be present in buffered body");
    assert!(body.contains("[DONE]"), "SSE done sentinel must be present in buffered body");

    ai_mock.assert_async().await;
}

/// Worker-first startup ordering: the worker binary starts and creates the
/// JetStream stream before the proxy binary is launched.
///
/// Both binaries call `ensure_stream` / `get_or_create_stream` on startup, so
/// whichever starts first creates the stream and the other gets the existing one.
/// This is the reverse of `binary_worker_starts_late_message_is_delivered`.
#[tokio::test]
async fn binary_worker_starts_before_proxy_request_succeeds() {
    let (_nats_container, nats_port) = start_nats().await;

    let mock_server = httpmock::MockServer::start_async().await;
    let ai_mock = mock_server
        .mock_async(|when, then| {
            when.method(httpmock::Method::POST)
                .path("/v1/messages")
                .header("authorization", "Bearer sk-ant-worker-first");
            then.status(200)
                .header("content-type", "application/json")
                .body(r#"{"id":"msg_worker_first"}"#);
        })
        .await;

    let token = "tok_anthropic_prod_wfirst01";

    // Start worker FIRST — it creates the stream and registers its consumer.
    let _worker = spawn_worker(nats_port, "binary-workers-wfirst", token, "sk-ant-worker-first");
    tokio::time::sleep(Duration::from_millis(800)).await;

    // Start proxy AFTER — it must find the stream the worker already created.
    let proxy_port = free_port();
    let _proxy = spawn_proxy(nats_port, proxy_port, &mock_server.base_url());
    wait_for_port(proxy_port, Duration::from_secs(15)).await;

    let resp = reqwest::Client::new()
        .post(format!(
            "http://127.0.0.1:{}/anthropic/v1/messages",
            proxy_port
        ))
        .header("Authorization", format!("Bearer {}", token))
        .header("Content-Type", "application/json")
        .body("{}")
        .timeout(Duration::from_secs(15))
        .send()
        .await
        .expect("Request to proxy failed");

    assert_eq!(
        resp.status(),
        200,
        "Request must succeed when worker started before proxy"
    );
    let body: serde_json::Value = resp.json().await.unwrap();
    assert_eq!(body["id"], "msg_worker_first");
    ai_mock.assert_async().await;
}
