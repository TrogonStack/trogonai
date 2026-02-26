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

use std::time::Duration;

use futures_util::future::join_all;
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

/// Bind to port 0, record the assigned port, then release it.
/// The binary will bind the same port moments later.
fn free_port() -> u16 {
    let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    l.local_addr().unwrap().port()
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
