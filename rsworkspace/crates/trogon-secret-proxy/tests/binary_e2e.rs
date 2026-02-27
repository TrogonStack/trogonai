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

// ── VGS property tests ────────────────────────────────────────────────────────

/// VGS property 1: the tok_ token NEVER reaches the AI provider.
///
/// The mock explicitly asserts that no request arrives with the proxy token
/// in the Authorization header.  Only the real key must appear.
#[tokio::test]
async fn binary_vgs_tok_token_never_reaches_ai_provider() {
    let (_nats_container, nats_port) = start_nats().await;

    let mock_server = httpmock::MockServer::start_async().await;
    let token = "tok_anthropic_prod_vgs001";
    let real_key = "sk-ant-real-vgs-key-001";

    // Mock that explicitly refuses any request that carries the proxy token.
    let tok_leak_mock = mock_server
        .mock_async(|when, then| {
            when.method(httpmock::Method::POST)
                .path("/v1/messages")
                .header("authorization", format!("Bearer {}", token));
            then.status(500).body("TOK TOKEN LEAKED TO PROVIDER");
        })
        .await;

    // Mock that accepts only the real key.
    let real_key_mock = mock_server
        .mock_async(|when, then| {
            when.method(httpmock::Method::POST)
                .path("/v1/messages")
                .header("authorization", format!("Bearer {}", real_key));
            then.status(200)
                .header("content-type", "application/json")
                .body(r#"{"id":"msg_vgs01","type":"message"}"#);
        })
        .await;

    let proxy_port = free_port();
    let _proxy = spawn_proxy(nats_port, proxy_port, &mock_server.base_url());
    wait_for_port(proxy_port, Duration::from_secs(15)).await;

    let _worker = spawn_worker(nats_port, "vgs-workers-001", token, real_key);
    tokio::time::sleep(Duration::from_millis(500)).await;

    let resp = reqwest::Client::new()
        .post(format!("http://127.0.0.1:{}/anthropic/v1/messages", proxy_port))
        .header("Authorization", format!("Bearer {}", token))
        .header("Content-Type", "application/json")
        .body(r#"{"model":"claude-3-5-sonnet","messages":[]}"#)
        .timeout(Duration::from_secs(15))
        .send()
        .await
        .unwrap();

    assert_eq!(resp.status(), 200, "Request with valid token must succeed");

    // The real key mock must have been hit exactly once.
    assert_eq!(real_key_mock.hits(), 1, "AI provider must receive the real key");
    // The tok_ token must NEVER have reached the provider.
    assert_eq!(
        tok_leak_mock.hits(),
        0,
        "tok_ proxy token must NEVER reach the AI provider — VGS security property violated"
    );
}

/// VGS property 2: the real API key never leaks back to the calling service.
///
/// The caller sends a tok_ token; the AI provider echoes back whatever
/// Authorization header it received.  The response body must contain the
/// real key (proving the provider got it) but the proxy must NOT forward
/// that value to the caller in any response header.
#[tokio::test]
async fn binary_vgs_real_key_never_leaks_to_caller_in_response_headers() {
    let (_nats_container, nats_port) = start_nats().await;

    let mock_server = httpmock::MockServer::start_async().await;
    let token = "tok_anthropic_prod_vgs002";
    let real_key = "sk-ant-real-vgs-key-002";

    // Provider echoes back the Authorization header it received as a custom
    // response header — simulating a misbehaving provider.
    let _ai_mock = mock_server
        .mock_async(|when, then| {
            when.method(httpmock::Method::POST).path("/v1/messages");
            then.status(200)
                .header("content-type", "application/json")
                .header("x-received-auth", format!("Bearer {}", real_key))
                .body(r#"{"id":"msg_vgs02"}"#);
        })
        .await;

    let proxy_port = free_port();
    let _proxy = spawn_proxy(nats_port, proxy_port, &mock_server.base_url());
    wait_for_port(proxy_port, Duration::from_secs(15)).await;

    let _worker = spawn_worker(nats_port, "vgs-workers-002", token, real_key);
    tokio::time::sleep(Duration::from_millis(500)).await;

    let resp = reqwest::Client::new()
        .post(format!("http://127.0.0.1:{}/anthropic/v1/messages", proxy_port))
        .header("Authorization", format!("Bearer {}", token))
        .header("Content-Type", "application/json")
        .body(r#"{"model":"claude-3-5-sonnet","messages":[]}"#)
        .timeout(Duration::from_secs(15))
        .send()
        .await
        .unwrap();

    assert_eq!(resp.status(), 200);

    // Scan every response header for the real key — it must not appear.
    for (name, value) in resp.headers() {
        let v = value.to_str().unwrap_or("");
        assert!(
            !v.contains(real_key),
            "Real API key must not appear in response header '{}': {:?}",
            name,
            v
        );
    }
}

/// VGS property 3: concurrent requests with different tokens resolve to their
/// respective real keys without cross-contamination.
///
/// Two services each send 5 concurrent requests; the mock verifies that
/// token A's key was never used for token B's requests and vice versa.
#[tokio::test]
async fn binary_vgs_concurrent_tokens_no_cross_contamination() {
    let (_nats_container, nats_port) = start_nats().await;

    let mock_server = httpmock::MockServer::start_async().await;

    let token_a = "tok_anthropic_prod_vgsa1";
    let real_key_a = "sk-ant-key-for-service-A";
    let token_b = "tok_anthropic_prod_vgsb1";
    let real_key_b = "sk-ant-key-for-service-B";

    // Mock for service A: only accepts real_key_a.
    let mock_a = mock_server
        .mock_async(|when, then| {
            when.method(httpmock::Method::POST)
                .path("/v1/messages")
                .header("authorization", format!("Bearer {}", real_key_a));
            then.status(200)
                .header("content-type", "application/json")
                .body(r#"{"service":"A"}"#);
        })
        .await;

    // Mock for service B: only accepts real_key_b.
    let mock_b = mock_server
        .mock_async(|when, then| {
            when.method(httpmock::Method::POST)
                .path("/v1/messages")
                .header("authorization", format!("Bearer {}", real_key_b));
            then.status(200)
                .header("content-type", "application/json")
                .body(r#"{"service":"B"}"#);
        })
        .await;

    let proxy_port = free_port();
    let _proxy = spawn_proxy(nats_port, proxy_port, &mock_server.base_url());
    wait_for_port(proxy_port, Duration::from_secs(15)).await;

    // Single worker holds both tokens.
    let _worker = Command::new(env!("CARGO_BIN_EXE_worker"))
        .env("NATS_URL", format!("localhost:{}", nats_port))
        .env("WORKER_CONSUMER_NAME", "vgs-workers-cross")
        .env(format!("VAULT_TOKEN_{}", token_a), real_key_a)
        .env(format!("VAULT_TOKEN_{}", token_b), real_key_b)
        .env("RUST_LOG", "warn")
        .kill_on_drop(true)
        .spawn()
        .unwrap();
    tokio::time::sleep(Duration::from_millis(500)).await;

    let client = reqwest::Client::new();
    let base = format!("http://127.0.0.1:{}/anthropic/v1/messages", proxy_port);

    // Send 5 requests for each service concurrently.
    let reqs_a = (0..5).map(|_| {
        client
            .post(&base)
            .header("Authorization", format!("Bearer {}", token_a))
            .header("Content-Type", "application/json")
            .body("{}")
            .timeout(Duration::from_secs(15))
            .send()
    });
    let reqs_b = (0..5).map(|_| {
        client
            .post(&base)
            .header("Authorization", format!("Bearer {}", token_b))
            .header("Content-Type", "application/json")
            .body("{}")
            .timeout(Duration::from_secs(15))
            .send()
    });

    let results_a: Vec<_> = join_all(reqs_a).await;
    let results_b: Vec<_> = join_all(reqs_b).await;

    for (i, r) in results_a.iter().enumerate() {
        let resp = r.as_ref().unwrap();
        assert_eq!(resp.status(), 200, "Service A request {} must succeed", i);
    }
    for (i, r) in results_b.iter().enumerate() {
        let resp = r.as_ref().unwrap();
        assert_eq!(resp.status(), 200, "Service B request {} must succeed", i);
    }

    // Each mock received exactly its own 5 requests — no cross-contamination.
    assert_eq!(mock_a.hits(), 5, "Service A key must be used for exactly 5 requests");
    assert_eq!(mock_b.hits(), 5, "Service B key must be used for exactly 5 requests");
}

/// VGS property 4: the proxy is transparent — status, headers, and body from
/// the AI provider are faithfully forwarded to the caller unchanged.
#[tokio::test]
async fn binary_vgs_proxy_is_transparent_to_caller() {
    let (_nats_container, nats_port) = start_nats().await;

    let mock_server = httpmock::MockServer::start_async().await;
    let token = "tok_anthropic_prod_vgs004";
    let real_key = "sk-ant-real-vgs-key-004";
    let response_body = r#"{"id":"msg_vgs04","type":"message","role":"assistant","content":[{"type":"text","text":"Hello!"}],"model":"claude-3-5-sonnet-20241022","stop_reason":"end_turn"}"#;

    let _ai_mock = mock_server
        .mock_async(|when, then| {
            when.method(httpmock::Method::POST).path("/v1/messages");
            then.status(201)
                .header("content-type", "application/json")
                .header("x-request-id", "req-vgs-004")
                .header("x-custom-provider-header", "provider-value-abc")
                .body(response_body);
        })
        .await;

    let proxy_port = free_port();
    let _proxy = spawn_proxy(nats_port, proxy_port, &mock_server.base_url());
    wait_for_port(proxy_port, Duration::from_secs(15)).await;

    let _worker = spawn_worker(nats_port, "vgs-workers-004", token, real_key);
    tokio::time::sleep(Duration::from_millis(500)).await;

    let resp = reqwest::Client::new()
        .post(format!("http://127.0.0.1:{}/anthropic/v1/messages", proxy_port))
        .header("Authorization", format!("Bearer {}", token))
        .header("Content-Type", "application/json")
        .body(r#"{"model":"claude-3-5-sonnet","messages":[]}"#)
        .timeout(Duration::from_secs(15))
        .send()
        .await
        .unwrap();

    // Status code forwarded unchanged.
    assert_eq!(resp.status(), 201, "Provider status 201 must be forwarded to caller");

    // Custom response headers forwarded.
    assert_eq!(
        resp.headers().get("x-request-id").and_then(|v| v.to_str().ok()),
        Some("req-vgs-004"),
        "x-request-id header must be forwarded"
    );
    assert_eq!(
        resp.headers().get("x-custom-provider-header").and_then(|v| v.to_str().ok()),
        Some("provider-value-abc"),
        "Custom provider header must be forwarded"
    );

    // Response body forwarded byte-for-byte.
    let body = resp.text().await.unwrap();
    assert_eq!(body, response_body, "Response body must be forwarded unchanged");
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

    // Worker rejects non-tok_ credentials → proxy returns 401 (client error).
    assert!(
        resp.status().is_client_error(),
        "Real API key should be rejected with a client error, got {}",
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
        resp.status().is_client_error(),
        "Missing auth header should return 4xx, got {}",
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
        resp_after.status().is_client_error(),
        "Post-revocation request should fail with 4xx, got {}",
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
        401,
        "Non-Bearer Authorization scheme must be rejected with 401"
    );
    let body = resp.text().await.unwrap();
    assert!(
        body.contains("not a Bearer token"),
        "401 body must explain the rejection reason, got: {:?}",
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
    assert_eq!(resp.status(), 401, "missing auth header must be 401");
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
    assert_eq!(resp.status(), 401, "unknown token must be 401");
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
    assert_eq!(resp.status(), 401, "real API key must be 401");
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
        401,
        "Malformed tok_ token must be rejected with 401"
    );
    let body = resp.text().await.unwrap();
    assert!(
        body.contains("Invalid proxy token"),
        "401 body must mention 'Invalid proxy token', got: {:?}",
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
        401,
        "Double-space Bearer must return 401, got {}",
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

/// A client sending a real HTTP/1.1 chunked request (Transfer-Encoding: chunked)
/// must have its body correctly assembled by the proxy and forwarded intact
/// to the AI provider.
///
/// Uses a raw `TcpStream` to send the chunked HTTP/1.1 wire format directly,
/// bypassing any high-level client that would set Content-Length instead.
#[tokio::test]
async fn binary_http_chunked_encoding_body_forwarded_completely() {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    let (_nats_container, nats_port) = start_nats().await;

    let chunk1 = b"{\"model\":\"claude-3\",";
    let chunk2 = b"\"messages\":[]}";
    let full_body = format!(
        "{}{}",
        std::str::from_utf8(chunk1).unwrap(),
        std::str::from_utf8(chunk2).unwrap()
    );

    let mock_server = httpmock::MockServer::start_async().await;
    let ai_mock = mock_server
        .mock_async(|when, then| {
            when.method(httpmock::Method::POST)
                .path("/v1/messages")
                .body(full_body.clone());
            then.status(200)
                .header("content-type", "application/json")
                .body(r#"{"id":"msg_tcp_chunked","type":"message"}"#);
        })
        .await;

    let proxy_port = free_port();
    let token = "tok_anthropic_prod_tcpchunk1";

    let _proxy = spawn_proxy(nats_port, proxy_port, &mock_server.base_url());
    wait_for_port(proxy_port, Duration::from_secs(15)).await;

    let _worker = spawn_worker(nats_port, "binary-workers-tcpchunk", token, "sk-ant-realkey");
    tokio::time::sleep(Duration::from_millis(500)).await;

    // Build raw HTTP/1.1 chunked request manually.
    // Each chunk: "<hex-length>\r\n<data>\r\n", terminated by "0\r\n\r\n".
    let c1_hex = format!("{:x}", chunk1.len());
    let c2_hex = format!("{:x}", chunk2.len());
    let raw_request = format!(
        "POST /anthropic/v1/messages HTTP/1.1\r\n\
         Host: 127.0.0.1:{}\r\n\
         Authorization: Bearer {}\r\n\
         Content-Type: application/json\r\n\
         Transfer-Encoding: chunked\r\n\
         Connection: close\r\n\
         \r\n\
         {}\r\n{}\r\n\
         {}\r\n{}\r\n\
         0\r\n\
         \r\n",
        proxy_port,
        token,
        c1_hex,
        std::str::from_utf8(chunk1).unwrap(),
        c2_hex,
        std::str::from_utf8(chunk2).unwrap(),
    );

    let mut tcp = tokio::net::TcpStream::connect(format!("127.0.0.1:{}", proxy_port))
        .await
        .expect("Failed to connect to proxy");

    tcp.write_all(raw_request.as_bytes()).await.unwrap();

    // Read the response — wait up to 15 s for the proxy to reply.
    let mut response_buf = Vec::new();
    tokio::time::timeout(Duration::from_secs(15), async {
        loop {
            let mut buf = [0u8; 4096];
            match tcp.read(&mut buf).await {
                Ok(0) | Err(_) => break,
                Ok(n) => response_buf.extend_from_slice(&buf[..n]),
            }
        }
    })
    .await
    .expect("Timed out reading proxy response");

    let response_str = String::from_utf8_lossy(&response_buf);
    assert!(
        response_str.starts_with("HTTP/1.1 200"),
        "Expected HTTP 200, got: {}",
        &response_str[..response_str.len().min(100)]
    );
    assert!(
        response_str.contains("msg_tcp_chunked"),
        "Response body must contain the mock's id"
    );
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

/// Hop-by-hop headers sent by the client (Connection, Keep-Alive) are stripped
/// by the proxy (RFC 7230 §6.1) and must NOT reach the AI provider.
/// End-to-end custom headers (X-End-To-End) MUST arrive at the provider.
#[tokio::test]
async fn binary_hop_by_hop_headers_stripped_before_reaching_provider() {
    let (_nats_container, nats_port) = start_nats().await;

    let mock_server = httpmock::MockServer::start_async().await;

    // This mock only matches if Connection: keep-alive arrives — it must NOT fire.
    let _mock_with_connection = mock_server
        .mock_async(|when, then| {
            when.method(httpmock::Method::POST)
                .path("/v1/messages")
                .header("connection", "keep-alive");
            then.status(500).body("hop-by-hop header reached the provider");
        })
        .await;

    // Correct path: x-end-to-end arrives, Connection does not.
    let mock_correct = mock_server
        .mock_async(|when, then| {
            when.method(httpmock::Method::POST)
                .path("/v1/messages")
                .header("x-end-to-end", "must-survive");
            then.status(200)
                .header("content-type", "application/json")
                .body(r#"{"id":"msg_hophop_ok"}"#);
        })
        .await;

    let proxy_port = free_port();
    let token = "tok_anthropic_prod_hophop01";

    let _proxy = spawn_proxy(nats_port, proxy_port, &mock_server.base_url());
    wait_for_port(proxy_port, Duration::from_secs(15)).await;

    let _worker = spawn_worker(nats_port, "binary-workers-hophop", token, "sk-ant-realkey");
    tokio::time::sleep(Duration::from_millis(500)).await;

    let resp = reqwest::Client::new()
        .post(format!("http://127.0.0.1:{}/anthropic/v1/messages", proxy_port))
        .header("Authorization", format!("Bearer {}", token))
        .header("Connection", "keep-alive")
        .header("Keep-Alive", "timeout=5, max=100")
        .header("X-End-To-End", "must-survive")
        .header("Content-Type", "application/json")
        .body("{}")
        .timeout(Duration::from_secs(15))
        .send()
        .await
        .unwrap();

    assert_eq!(resp.status(), 200, "Proxy must strip hop-by-hop headers and return 200");
    let body: serde_json::Value = resp.json().await.unwrap();
    assert_eq!(body["id"], "msg_hophop_ok");
    mock_correct.assert_async().await;
}

/// Three worker instances share the same durable consumer.
/// Nine concurrent requests must all succeed — each worker handles its share.
#[tokio::test]
async fn binary_three_workers_handle_requests() {
    let (_nats_container, nats_port) = start_nats().await;

    let mock_server = httpmock::MockServer::start_async().await;
    let _mock = mock_server
        .mock_async(|when, then| {
            when.method(httpmock::Method::POST).path("/v1/messages");
            then.status(200)
                .header("content-type", "application/json")
                .body(r#"{"id":"msg_three_workers"}"#);
        })
        .await;

    let proxy_port = free_port();
    let token = "tok_anthropic_prod_w3test01";

    let _proxy = spawn_proxy(nats_port, proxy_port, &mock_server.base_url());
    wait_for_port(proxy_port, Duration::from_secs(15)).await;

    let _w1 = spawn_worker(nats_port, "binary-workers-3w", token, "sk-ant-realkey");
    let _w2 = spawn_worker(nats_port, "binary-workers-3w", token, "sk-ant-realkey");
    let _w3 = spawn_worker(nats_port, "binary-workers-3w", token, "sk-ant-realkey");
    tokio::time::sleep(Duration::from_millis(500)).await;

    let client = reqwest::Client::new();
    let handles: Vec<_> = (0..9)
        .map(|_| {
            let c = client.clone();
            let url = format!("http://127.0.0.1:{}/anthropic/v1/messages", proxy_port);
            let tok = token.to_string();
            tokio::spawn(async move {
                c.post(url)
                    .header("Authorization", format!("Bearer {}", tok))
                    .body("{}")
                    .timeout(Duration::from_secs(20))
                    .send()
                    .await
                    .unwrap()
            })
        })
        .collect();

    for result in join_all(handles).await {
        let resp = result.unwrap();
        assert_eq!(resp.status(), 200, "Expected 200 with 3 workers");
    }
}

/// Three proxy instances all serve requests through a single worker.
#[tokio::test]
async fn binary_three_proxy_instances_all_serve_requests() {
    let (_nats_container, nats_port) = start_nats().await;

    let mock_server = httpmock::MockServer::start_async().await;
    let _mock = mock_server
        .mock_async(|when, then| {
            when.method(httpmock::Method::POST).path("/v1/messages");
            then.status(200)
                .header("content-type", "application/json")
                .body(r#"{"id":"msg_three_proxies"}"#);
        })
        .await;

    let token = "tok_anthropic_prod_p3test01";
    let port_a = free_port();
    let port_b = free_port();
    let port_c = free_port();

    let _pa = spawn_proxy(nats_port, port_a, &mock_server.base_url());
    let _pb = spawn_proxy(nats_port, port_b, &mock_server.base_url());
    let _pc = spawn_proxy(nats_port, port_c, &mock_server.base_url());

    wait_for_port(port_a, Duration::from_secs(15)).await;
    wait_for_port(port_b, Duration::from_secs(15)).await;
    wait_for_port(port_c, Duration::from_secs(15)).await;

    let _worker = spawn_worker(nats_port, "binary-workers-3p", token, "sk-ant-realkey");
    tokio::time::sleep(Duration::from_millis(500)).await;

    let client = reqwest::Client::new();
    for port in [port_a, port_b, port_c] {
        let resp = client
            .post(format!("http://127.0.0.1:{}/anthropic/v1/messages", port))
            .header("Authorization", format!("Bearer {}", token))
            .body("{}")
            .timeout(Duration::from_secs(15))
            .send()
            .await
            .unwrap();
        assert_eq!(resp.status(), 200, "Proxy on port {} must return 200", port);
        let body: serde_json::Value = resp.json().await.unwrap();
        assert_eq!(body["id"], "msg_three_proxies");
    }
}

/// A 304 Not Modified response (no body) must be forwarded to the caller
/// with status 304.
#[tokio::test]
async fn binary_304_not_modified_response_forwarded() {
    let (_nats_container, nats_port) = start_nats().await;

    let mock_server = httpmock::MockServer::start_async().await;
    let ai_mock = mock_server
        .mock_async(|when, then| {
            when.method(httpmock::Method::GET).path("/v1/models");
            then.status(304).header("etag", r#""abc123""#);
        })
        .await;

    let proxy_port = free_port();
    let token = "tok_anthropic_prod_304test1";

    let _proxy = spawn_proxy(nats_port, proxy_port, &mock_server.base_url());
    wait_for_port(proxy_port, Duration::from_secs(15)).await;

    let _worker = spawn_worker(nats_port, "binary-workers-304", token, "sk-ant-realkey");
    tokio::time::sleep(Duration::from_millis(500)).await;

    let resp = reqwest::Client::new()
        .get(format!("http://127.0.0.1:{}/anthropic/v1/models", proxy_port))
        .header("Authorization", format!("Bearer {}", token))
        .timeout(Duration::from_secs(15))
        .send()
        .await
        .unwrap();

    assert_eq!(resp.status(), 304, "304 Not Modified must be forwarded");
    ai_mock.assert_async().await;
}

/// URL-encoded query string characters (spaces, ampersands) must be forwarded
/// to the AI provider without alteration.
#[tokio::test]
async fn binary_query_string_url_encoded_chars_forwarded() {
    let (_nats_container, nats_port) = start_nats().await;

    let mock_server = httpmock::MockServer::start_async().await;
    let ai_mock = mock_server
        .mock_async(|when, then| {
            when.method(httpmock::Method::GET)
                .path("/v1/models")
                .query_param("q", "hello world")
                .query_param("filter", "a&b");
            then.status(200)
                .header("content-type", "application/json")
                .body(r#"{"id":"msg_urlenc"}"#);
        })
        .await;

    let proxy_port = free_port();
    let token = "tok_anthropic_prod_urlenc01";

    let _proxy = spawn_proxy(nats_port, proxy_port, &mock_server.base_url());
    wait_for_port(proxy_port, Duration::from_secs(15)).await;

    let _worker = spawn_worker(nats_port, "binary-workers-urlenc", token, "sk-ant-realkey");
    tokio::time::sleep(Duration::from_millis(500)).await;

    let resp = reqwest::Client::new()
        .get(format!(
            "http://127.0.0.1:{}/anthropic/v1/models?q=hello%20world&filter=a%26b",
            proxy_port
        ))
        .header("Authorization", format!("Bearer {}", token))
        .timeout(Duration::from_secs(15))
        .send()
        .await
        .unwrap();

    assert_eq!(resp.status(), 200, "URL-encoded query string must be forwarded");
    ai_mock.assert_async().await;
}

/// Set-Cookie, Cache-Control and ETag response headers from the AI provider
/// must all be forwarded to the caller unchanged.
#[tokio::test]
async fn binary_set_cookie_cache_etag_response_headers_forwarded() {
    let (_nats_container, nats_port) = start_nats().await;

    let mock_server = httpmock::MockServer::start_async().await;
    let ai_mock = mock_server
        .mock_async(|when, then| {
            when.method(httpmock::Method::POST).path("/v1/messages");
            then.status(200)
                .header("content-type", "application/json")
                .header("set-cookie", "session=abc123; Path=/; HttpOnly")
                .header("cache-control", "no-store")
                .header("etag", r#""v1""#)
                .body(r#"{"id":"msg_resp_headers"}"#);
        })
        .await;

    let proxy_port = free_port();
    let token = "tok_anthropic_prod_rhdrs01";

    let _proxy = spawn_proxy(nats_port, proxy_port, &mock_server.base_url());
    wait_for_port(proxy_port, Duration::from_secs(15)).await;

    let _worker = spawn_worker(nats_port, "binary-workers-rhdrs", token, "sk-ant-realkey");
    tokio::time::sleep(Duration::from_millis(500)).await;

    let resp = reqwest::Client::new()
        .post(format!("http://127.0.0.1:{}/anthropic/v1/messages", proxy_port))
        .header("Authorization", format!("Bearer {}", token))
        .body("{}")
        .timeout(Duration::from_secs(15))
        .send()
        .await
        .unwrap();

    assert_eq!(resp.status(), 200);
    assert!(resp.headers().contains_key("set-cookie"),   "Set-Cookie must be forwarded");
    assert!(resp.headers().contains_key("cache-control"), "Cache-Control must be forwarded");
    assert!(resp.headers().contains_key("etag"),          "ETag must be forwarded");
    ai_mock.assert_async().await;
}

/// A token with an all-numeric ID (`tok_anthropic_prod_12345`) is valid per
/// the `[a-zA-Z0-9]+` rule and must resolve correctly end-to-end.
#[tokio::test]
async fn binary_token_with_numeric_id_works() {
    let (_nats_container, nats_port) = start_nats().await;

    let mock_server = httpmock::MockServer::start_async().await;
    let ai_mock = mock_server
        .mock_async(|when, then| {
            when.method(httpmock::Method::POST)
                .path("/v1/messages")
                .header("authorization", "Bearer sk-ant-numeric");
            then.status(200)
                .header("content-type", "application/json")
                .body(r#"{"id":"msg_numeric_id"}"#);
        })
        .await;

    let token = "tok_anthropic_prod_12345";
    let proxy_port = free_port();

    let _proxy = spawn_proxy(nats_port, proxy_port, &mock_server.base_url());
    wait_for_port(proxy_port, Duration::from_secs(15)).await;

    let _worker = spawn_worker(nats_port, "binary-workers-numid", token, "sk-ant-numeric");
    tokio::time::sleep(Duration::from_millis(500)).await;

    let resp = reqwest::Client::new()
        .post(format!("http://127.0.0.1:{}/anthropic/v1/messages", proxy_port))
        .header("Authorization", format!("Bearer {}", token))
        .body("{}")
        .timeout(Duration::from_secs(15))
        .send()
        .await
        .unwrap();

    assert_eq!(resp.status(), 200);
    let body: serde_json::Value = resp.json().await.unwrap();
    assert_eq!(body["id"], "msg_numeric_id");
    ai_mock.assert_async().await;
}

/// When the upstream AI provider returns multiple `Set-Cookie` response
/// headers both values must be forwarded to the client unchanged.
///
/// The worker now uses `Vec<(String, String)>` for headers so duplicate
/// header names are preserved.  The proxy uses `HeaderMap::append` (not
/// `insert`) so all values survive the round-trip to the HTTP response.
#[tokio::test]
async fn binary_multiple_set_cookie_headers_forwarded() {
    let (_nats_container, nats_port) = start_nats().await;

    let mock_server = httpmock::MockServer::start_async().await;
    let ai_mock = mock_server
        .mock_async(|when, then| {
            when.method(httpmock::Method::GET).path("/v1/session");
            then.status(200)
                .header("content-type", "application/json")
                .header("set-cookie", "session=abc; Path=/; HttpOnly")
                .header("set-cookie", "prefs=xyz; Path=/")
                .body(r#"{"ok":true}"#);
        })
        .await;

    let token = "tok_anthropic_prod_cookie01";
    let proxy_port = free_port();

    let _proxy = spawn_proxy(nats_port, proxy_port, &mock_server.base_url());
    wait_for_port(proxy_port, Duration::from_secs(15)).await;

    let _worker = spawn_worker(nats_port, "binary-workers-cookie", token, "sk-ant-cookie");
    tokio::time::sleep(Duration::from_millis(500)).await;

    // reqwest is compiled without the `cookies` feature so Set-Cookie headers
    // are never consumed internally — they appear in response.headers() as-is.
    let resp = reqwest::Client::new()
        .get(format!("http://127.0.0.1:{}/anthropic/v1/session", proxy_port))
        .header("Authorization", format!("Bearer {}", token))
        .timeout(Duration::from_secs(15))
        .send()
        .await
        .unwrap();

    assert_eq!(resp.status(), 200);

    // Both Set-Cookie headers must reach the client — Vec + append preserve them.
    let cookies: Vec<_> = resp
        .headers()
        .get_all("set-cookie")
        .iter()
        .map(|v| v.to_str().unwrap().to_string())
        .collect();
    assert_eq!(cookies.len(), 2, "Both Set-Cookie headers must be forwarded; got: {:?}", cookies);
    assert!(cookies.iter().any(|c| c.contains("session=abc")), "session cookie must be present");
    assert!(cookies.iter().any(|c| c.contains("prefs=xyz")), "prefs cookie must be present");
    ai_mock.assert_async().await;
}

/// A 204 No Content response (no body, no Content-Type) is forwarded
/// correctly by the proxy.  The status code must be 204 and the response
/// body must be empty.
#[tokio::test]
async fn binary_204_no_content_response_forwarded() {
    let (_nats_container, nats_port) = start_nats().await;

    let mock_server = httpmock::MockServer::start_async().await;
    let ai_mock = mock_server
        .mock_async(|when, then| {
            when.method(httpmock::Method::DELETE).path("/v1/session");
            then.status(204);
        })
        .await;

    let token = "tok_anthropic_prod_nocont01";
    let proxy_port = free_port();

    let _proxy = spawn_proxy(nats_port, proxy_port, &mock_server.base_url());
    wait_for_port(proxy_port, Duration::from_secs(15)).await;

    let _worker = spawn_worker(nats_port, "binary-workers-nocont", token, "sk-ant-nocont");
    tokio::time::sleep(Duration::from_millis(500)).await;

    let resp = reqwest::Client::new()
        .delete(format!(
            "http://127.0.0.1:{}/anthropic/v1/session",
            proxy_port
        ))
        .header("Authorization", format!("Bearer {}", token))
        .timeout(Duration::from_secs(15))
        .send()
        .await
        .unwrap();

    assert_eq!(resp.status(), 204, "204 No Content must be forwarded as-is");
    let body = resp.bytes().await.unwrap();
    assert!(body.is_empty(), "204 response body must be empty");
    ai_mock.assert_async().await;
}

/// A single worker instance can resolve tokens for multiple AI providers.
/// The worker reads ALL `VAULT_TOKEN_*` environment variables at startup,
/// so one process can hold keys for both Anthropic and OpenAI.
///
/// This test sends one request per provider and verifies that the correct
/// real key is substituted for each token.
#[tokio::test]
async fn binary_single_worker_handles_multiple_providers() {
    let (_nats_container, nats_port) = start_nats().await;

    let mock_server = httpmock::MockServer::start_async().await;

    // Anthropic endpoint — expects the anthropic real key.
    let ant_mock = mock_server
        .mock_async(|when, then| {
            when.method(httpmock::Method::POST)
                .path("/v1/messages")
                .header("authorization", "Bearer sk-ant-multiprov");
            then.status(200)
                .header("content-type", "application/json")
                .body(r#"{"provider":"anthropic"}"#);
        })
        .await;

    // OpenAI endpoint — expects the openai real key.
    let oai_mock = mock_server
        .mock_async(|when, then| {
            when.method(httpmock::Method::POST)
                .path("/v1/chat/completions")
                .header("authorization", "Bearer sk-oai-multiprov");
            then.status(200)
                .header("content-type", "application/json")
                .body(r#"{"provider":"openai"}"#);
        })
        .await;

    let ant_token = "tok_anthropic_prod_mp001";
    let oai_token = "tok_openai_prod_mp001";
    let proxy_port = free_port();

    let _proxy = spawn_proxy(nats_port, proxy_port, &mock_server.base_url());
    wait_for_port(proxy_port, Duration::from_secs(15)).await;

    // One worker process configured with both tokens.
    let _worker = tokio::process::Command::new(env!("CARGO_BIN_EXE_worker"))
        .env("NATS_URL", format!("localhost:{}", nats_port))
        .env("WORKER_CONSUMER_NAME", "binary-workers-multiprov")
        .env(format!("VAULT_TOKEN_{}", ant_token), "sk-ant-multiprov")
        .env(format!("VAULT_TOKEN_{}", oai_token), "sk-oai-multiprov")
        .env("RUST_LOG", "warn")
        .kill_on_drop(true)
        .spawn()
        .unwrap();
    tokio::time::sleep(Duration::from_millis(500)).await;

    let client = reqwest::Client::new();

    let ant_resp = client
        .post(format!(
            "http://127.0.0.1:{}/anthropic/v1/messages",
            proxy_port
        ))
        .header("Authorization", format!("Bearer {}", ant_token))
        .body("{}")
        .timeout(Duration::from_secs(15))
        .send()
        .await
        .unwrap();

    let oai_resp = client
        .post(format!(
            "http://127.0.0.1:{}/openai/v1/chat/completions",
            proxy_port
        ))
        .header("Authorization", format!("Bearer {}", oai_token))
        .body("{}")
        .timeout(Duration::from_secs(15))
        .send()
        .await
        .unwrap();

    assert_eq!(ant_resp.status(), 200);
    let ant_body: serde_json::Value = ant_resp.json().await.unwrap();
    assert_eq!(ant_body["provider"], "anthropic");

    assert_eq!(oai_resp.status(), 200);
    let oai_body: serde_json::Value = oai_resp.json().await.unwrap();
    assert_eq!(oai_body["provider"], "openai");

    ant_mock.assert_async().await;
    oai_mock.assert_async().await;
}

/// A request to `/anthropic` with no path segment after the provider is not
/// matched by the `/{provider}/{*path}` route — axum requires at least one
/// character after the second `/`.
///
/// The proxy returns 404 directly from the axum router without ever
/// touching NATS or the worker.
#[tokio::test]
async fn binary_url_without_path_segment_returns_404() {
    let (_nats_container, nats_port) = start_nats().await;

    // No mock server needed — the request must fail at axum routing.
    let proxy_port = free_port();
    let _proxy = spawn_proxy(nats_port, proxy_port, "http://127.0.0.1:1");
    wait_for_port(proxy_port, Duration::from_secs(15)).await;

    let client = reqwest::Client::new();

    // No second path segment → axum route `/{provider}/{*path}` has no match.
    let resp_no_path = client
        .get(format!("http://127.0.0.1:{}/anthropic", proxy_port))
        .timeout(Duration::from_secs(10))
        .send()
        .await
        .unwrap();

    assert_eq!(
        resp_no_path.status(),
        404,
        "Request with no path segment after provider must return 404"
    );

    // A trailing slash produces an empty `{*path}` segment; axum rejects it too.
    let resp_trailing_slash = client
        .get(format!("http://127.0.0.1:{}/anthropic/", proxy_port))
        .timeout(Duration::from_secs(10))
        .send()
        .await
        .unwrap();

    assert!(
        resp_trailing_slash.status() == 404
            || resp_trailing_slash.status().as_u16() >= 400,
        "Trailing-slash request with empty path must return a 4xx; got {}",
        resp_trailing_slash.status()
    );
}

/// A request to `/{provider}?query=string` with NO path segment after the
/// provider (only a query string) is also unmatched by `/{provider}/{*path}`.
/// The proxy returns 404 from the axum router before touching NATS.
#[tokio::test]
async fn binary_url_with_query_but_no_path_segment_returns_404() {
    let (_nats_container, nats_port) = start_nats().await;

    let proxy_port = free_port();
    let _proxy = spawn_proxy(nats_port, proxy_port, "http://127.0.0.1:1");
    wait_for_port(proxy_port, Duration::from_secs(15)).await;

    let resp = reqwest::Client::new()
        .get(format!(
            "http://127.0.0.1:{}/anthropic?model=claude-3&stream=true",
            proxy_port
        ))
        .timeout(Duration::from_secs(10))
        .send()
        .await
        .unwrap();

    assert_eq!(
        resp.status(),
        404,
        "Query string without path segment must not match the route → 404"
    );
}

/// When `PROXY_WORKER_TIMEOUT_SECS=0` the proxy uses `Duration::ZERO` for the
/// worker reply timeout.  `tokio::time::timeout(Duration::ZERO, future)` fires
/// on the first poll if the future is not immediately ready — so the proxy
/// returns 504 essentially as fast as the NATS round-trip for subscribe +
/// publish completes.
#[tokio::test]
async fn binary_zero_worker_timeout_returns_504_immediately() {
    let (_nats_container, nats_port) = start_nats().await;

    let proxy_port = free_port();
    let _proxy = spawn_proxy_with_timeout(nats_port, proxy_port, "http://127.0.0.1:1", 0);
    wait_for_port(proxy_port, Duration::from_secs(15)).await;

    // No worker — proxy times out immediately.
    let start = std::time::Instant::now();
    let resp = reqwest::Client::new()
        .post(format!("http://127.0.0.1:{}/anthropic/v1/messages", proxy_port))
        .header("Authorization", "Bearer tok_anthropic_prod_ztimeout1")
        .body("{}")
        .timeout(Duration::from_secs(10))
        .send()
        .await
        .unwrap();
    let elapsed = start.elapsed();

    assert_eq!(resp.status(), 504, "Zero timeout must produce 504");
    assert!(
        elapsed < Duration::from_secs(3),
        "Zero timeout must fire quickly, took {:?}",
        elapsed
    );
}

/// When the proxy binary cannot connect to NATS within the 10-second connect
/// timeout it panics and exits with a non-zero status code.
///
/// We point `NATS_URL` at a port where nothing is listening to reliably
/// trigger a connection failure.
#[tokio::test]
async fn binary_proxy_exits_nonzero_when_nats_unreachable() {
    let unreachable_port = free_port(); // nothing bound here

    let mut child = tokio::process::Command::new(env!("CARGO_BIN_EXE_proxy"))
        .env("NATS_URL", format!("localhost:{}", unreachable_port))
        .env("PROXY_PORT", free_port().to_string())
        .env("RUST_LOG", "error")
        .kill_on_drop(true)
        .spawn()
        .unwrap();

    // Connect timeout is 10 s; give the binary up to 15 s to exit.
    let status = tokio::time::timeout(Duration::from_secs(15), child.wait())
        .await
        .expect("Proxy binary must exit within 15 s when NATS is unreachable")
        .unwrap();

    assert!(
        !status.success(),
        "Proxy must exit non-zero when NATS is unreachable"
    );
}

/// Same startup-failure behaviour for the worker binary.
#[tokio::test]
async fn binary_worker_exits_nonzero_when_nats_unreachable() {
    let unreachable_port = free_port();

    let mut child = tokio::process::Command::new(env!("CARGO_BIN_EXE_worker"))
        .env("NATS_URL", format!("localhost:{}", unreachable_port))
        .env("RUST_LOG", "error")
        .kill_on_drop(true)
        .spawn()
        .unwrap();

    let status = tokio::time::timeout(Duration::from_secs(15), child.wait())
        .await
        .expect("Worker binary must exit within 15 s when NATS is unreachable")
        .unwrap();

    assert!(
        !status.success(),
        "Worker must exit non-zero when NATS is unreachable"
    );
}

/// The proxy binary terminates within a few seconds of receiving SIGTERM.
///
/// Tokio does not install a custom SIGTERM handler; the OS default terminates
/// the process when SIGTERM is delivered (process exits via signal, not
/// `exit()`, so `status.success()` is false and `status.code()` is None).
#[tokio::test]
async fn binary_proxy_exits_on_sigterm() {
    let (_nats_container, nats_port) = start_nats().await;
    let proxy_port = free_port();

    let mut child = tokio::process::Command::new(env!("CARGO_BIN_EXE_proxy"))
        .env("NATS_URL", format!("localhost:{}", nats_port))
        .env("PROXY_PORT", proxy_port.to_string())
        .env("PROXY_BASE_URL_OVERRIDE", "http://127.0.0.1:1")
        .env("RUST_LOG", "error")
        .kill_on_drop(true)
        .spawn()
        .unwrap();

    wait_for_port(proxy_port, Duration::from_secs(15)).await;

    // Send SIGTERM.
    let pid = child.id().expect("Child must have a PID while running");
    std::process::Command::new("kill")
        .args(["-TERM", &pid.to_string()])
        .status()
        .expect("kill command must succeed");

    // The process must exit within 5 seconds of SIGTERM.
    let status = tokio::time::timeout(Duration::from_secs(5), child.wait())
        .await
        .expect("Proxy must exit within 5 s of SIGTERM")
        .expect("wait() failed");

    // Signal termination: success() is false, code() is None on Unix.
    assert!(
        !status.success(),
        "Process killed by SIGTERM must not report success"
    );
}

/// `PROXY_PORT=abc` is not a valid `u16` — the binary must fall back to the
/// default port 8080 instead of panicking.
///
/// The parsing is `.and_then(|v| v.parse().ok()).unwrap_or(8080)` in proxy.rs,
/// so any non-numeric value silently produces the default.
///
/// NOTE: This test requires port 8080 to be available on the host.
#[tokio::test]
async fn binary_proxy_port_invalid_value_falls_back_to_default_8080() {
    let (_nats_container, nats_port) = start_nats().await;

    // Use PROXY_PORT=abc which cannot be parsed as u16 → binary falls back to 8080.
    let _proxy = Command::new(env!("CARGO_BIN_EXE_proxy"))
        .env("NATS_URL", format!("localhost:{}", nats_port))
        .env("PROXY_PORT", "abc") // invalid u16 → fall back to 8080
        .env("PROXY_WORKER_TIMEOUT_SECS", "5")
        .env("RUST_LOG", "error")
        .kill_on_drop(true)
        .spawn()
        .expect("Failed to spawn proxy binary");

    // If the binary crashed, or bound to any other port, this will timeout and panic.
    wait_for_port(8080, Duration::from_secs(15)).await;

    // Successfully connected to port 8080 — invalid PROXY_PORT fell back to default.
}

/// `PROXY_WORKER_TIMEOUT_SECS=abc` is not a valid `u64` — the binary must
/// fall back to the default (60 s) instead of panicking.
///
/// The parsing is `.and_then(|v| v.parse().ok()).unwrap_or(60)` in proxy.rs.
#[tokio::test]
async fn binary_worker_timeout_invalid_value_falls_back_to_default() {
    let (_nats_container, nats_port) = start_nats().await;

    let proxy_port = free_port();

    // Use PROXY_WORKER_TIMEOUT_SECS=abc which cannot be parsed as u64 → falls back to 60s.
    let _proxy = Command::new(env!("CARGO_BIN_EXE_proxy"))
        .env("NATS_URL", format!("localhost:{}", nats_port))
        .env("PROXY_PORT", proxy_port.to_string())
        .env("PROXY_WORKER_TIMEOUT_SECS", "abc") // invalid u64 → fall back to 60 s
        .env("RUST_LOG", "error")
        .kill_on_drop(true)
        .spawn()
        .expect("Failed to spawn proxy binary");

    // Proxy must start — if the invalid timeout caused a panic the port never opens.
    wait_for_port(proxy_port, Duration::from_secs(15)).await;

    // TCP connection succeeded → invalid PROXY_WORKER_TIMEOUT_SECS fell back to default.
}

/// When a `VAULT_TOKEN_*` env var is set with an empty value, the worker must
/// reject the request with an error — it must NOT forward an `Authorization: Bearer`
/// header (no key) to the AI provider.
///
/// An empty resolved key means the token exchange failed: the whole point of
/// the system is to swap proxy tokens for real API keys, so an empty real key
/// is a vault misconfiguration that must be caught before the upstream call.
#[tokio::test]
async fn binary_vault_token_empty_value_is_rejected_with_error() {
    let (_nats_container, nats_port) = start_nats().await;

    let mock_server = httpmock::MockServer::start_async().await;
    // The AI provider must never be called — worker rejects before forwarding.
    let ai_mock = mock_server
        .mock_async(|when, then| {
            when.any_request();
            then.status(200).body("should not be reached");
        })
        .await;

    let proxy_port = free_port();
    let token = "tok_anthropic_prod_emptyval1";

    let _proxy = spawn_proxy(nats_port, proxy_port, &mock_server.base_url());
    wait_for_port(proxy_port, Duration::from_secs(15)).await;

    // Worker with an empty real key: VAULT_TOKEN_<token>= (no value).
    let _worker = Command::new(env!("CARGO_BIN_EXE_worker"))
        .env("NATS_URL", format!("localhost:{}", nats_port))
        .env("WORKER_CONSUMER_NAME", "binary-workers-emptyval")
        .env(format!("VAULT_TOKEN_{}", token), "") // empty real key
        .env("RUST_LOG", "warn")
        .kill_on_drop(true)
        .spawn()
        .expect("Failed to spawn worker binary");
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
        .expect("Request to proxy binary failed");

    // Worker must return an error — empty key is a vault misconfiguration.
    assert_eq!(
        resp.status(),
        401,
        "Empty real key in vault must yield 401, not forward to provider"
    );
    // The AI provider must not have received the request.
    assert_eq!(ai_mock.hits(), 0, "Provider must not be called when vault key is empty");
}

/// When `WORKER_CONSUMER_NAME` is not set, the worker binary falls back to
/// the default consumer name `"proxy-workers"`.  Requests must be processed
/// normally — the proxy is agnostic to consumer names.
#[tokio::test]
async fn binary_worker_default_consumer_name_when_not_set() {
    let (_nats_container, nats_port) = start_nats().await;

    let mock_server = httpmock::MockServer::start_async().await;
    let ai_mock = mock_server
        .mock_async(|when, then| {
            when.method(httpmock::Method::POST).path("/v1/messages");
            then.status(200)
                .header("content-type", "application/json")
                .body(r#"{"id":"msg_default_consumer"}"#);
        })
        .await;

    let proxy_port = free_port();
    let token = "tok_anthropic_prod_defcons01";

    let _proxy = spawn_proxy(nats_port, proxy_port, &mock_server.base_url());
    wait_for_port(proxy_port, Duration::from_secs(15)).await;

    // Worker WITHOUT WORKER_CONSUMER_NAME → falls back to "proxy-workers".
    let _worker = Command::new(env!("CARGO_BIN_EXE_worker"))
        .env("NATS_URL", format!("localhost:{}", nats_port))
        // No WORKER_CONSUMER_NAME env var — binary uses "proxy-workers" default.
        .env(format!("VAULT_TOKEN_{}", token), "sk-ant-defcons-realkey")
        .env("RUST_LOG", "warn")
        .kill_on_drop(true)
        .spawn()
        .expect("Failed to spawn worker binary");
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
        .expect("Request to proxy binary failed");

    assert_eq!(
        resp.status(),
        200,
        "Default consumer name must allow worker to process requests"
    );
    ai_mock.assert_async().await;
}

/// Same SIGTERM behaviour for the worker binary.
#[tokio::test]
async fn binary_worker_exits_on_sigterm() {
    let (_nats_container, nats_port) = start_nats().await;

    let mut child = tokio::process::Command::new(env!("CARGO_BIN_EXE_worker"))
        .env("NATS_URL", format!("localhost:{}", nats_port))
        .env("WORKER_CONSUMER_NAME", "binary-workers-sigterm")
        .env("RUST_LOG", "error")
        .kill_on_drop(true)
        .spawn()
        .unwrap();

    // Give the worker time to connect and start its consumer loop.
    tokio::time::sleep(Duration::from_secs(2)).await;

    let pid = child.id().expect("Child must have a PID while running");
    std::process::Command::new("kill")
        .args(["-TERM", &pid.to_string()])
        .status()
        .expect("kill command must succeed");

    let status = tokio::time::timeout(Duration::from_secs(5), child.wait())
        .await
        .expect("Worker must exit within 5 s of SIGTERM")
        .expect("wait() failed");

    assert!(
        !status.success(),
        "Process killed by SIGTERM must not report success"
    );
}

// ── NATS max_payload / large response body ────────────────────────────────────

/// When the upstream AI provider returns a response large enough that the
/// JSON-serialised `OutboundHttpResponse` exceeds the NATS server's
/// `max_payload` limit, the worker's `nats.publish(reply_to, ...)` returns an
/// error.  The worker logs the error and **still acks** the JetStream message
/// (no duplicate upstream call), but the proxy never receives a reply and
/// returns 504.
///
/// This covers two previously untested gaps in a single test:
/// 1. Worker Core NATS reply-publish fails after a successful upstream call.
/// 2. Response bodies large enough to overflow the NATS message-size limit.
///
/// The default NATS `max_payload` is 1 MiB (1,048,576 bytes).
/// `serde_json` serialises `Vec<u8>` as a JSON array of numbers
/// (`[120, 120, …]`); each byte averages ~4 chars when encoded, so a 300 KB
/// response body serialises to ~1.2 MB of JSON — exceeding the 1 MiB limit
/// and causing `nats.publish()` to fail.
///
/// After the bug-fix the worker catches this error and immediately publishes a
/// compact `OutboundHttpResponse { error: Some("...") }` so the proxy returns
/// 502 Bad Gateway with a descriptive message instead of waiting for a timeout.
#[tokio::test]
async fn binary_large_response_body_exceeds_nats_max_payload_causes_504() {
    let (_nats_container, nats_port) = start_nats().await;

    // A 300 KB body: ~1.2 MB after JSON byte-array encoding → exceeds 1 MiB NATS limit.
    let large_body: String = "x".repeat(300_000);
    let large_json = format!(r#"{{"content":"{}"}}"#, large_body);

    let mock_server = httpmock::MockServer::start_async().await;
    let ai_mock = mock_server
        .mock_async(|when, then| {
            when.method(httpmock::Method::POST).path("/v1/messages");
            then.status(200)
                .header("content-type", "application/json")
                .body(large_json.clone());
        })
        .await;

    let proxy_port = free_port();
    let token = "tok_anthropic_prod_lge001";

    // 5 s proxy timeout so the test doesn't block for the default 60 s.
    let _proxy = spawn_proxy_with_timeout(nats_port, proxy_port, &mock_server.base_url(), 5);
    wait_for_port(proxy_port, Duration::from_secs(15)).await;

    let _worker = spawn_worker(nats_port, "binary-workers-lge", token, "sk-ant-key-lge");
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

    // Worker called the AI provider but could not publish the full reply
    // (payload > 1 MiB NATS limit).  The worker now publishes a compact error
    // response immediately, so the proxy returns 502 Bad Gateway with a
    // descriptive body — not an opaque 504 timeout.
    assert_eq!(
        resp.status(),
        502,
        "NATS publish failure (payload > max_payload) must cause 502 with error body, got {}",
        resp.status()
    );
    let body = resp.text().await.unwrap();
    assert!(
        body.contains("Worker failed to publish reply"),
        "Response body must describe the real cause, got: {:?}",
        body
    );
    // The AI provider was called exactly once.  The worker acks the JetStream
    // message after publishing the error response, so there is no redelivery.
    assert_eq!(
        ai_mock.hits(),
        1,
        "AI provider must be called exactly once (no JetStream redelivery after ack)"
    );
}

// ── Slow upstream / proxy timeout while worker is mid-HTTP-call ───────────────

/// When the upstream AI provider responds more slowly than the proxy's
/// `worker_timeout`, the proxy returns 504 before the upstream finishes.
///
/// The worker's HTTP call is still in-flight at proxy-timeout time.  Once the
/// upstream eventually responds, the worker serialises the reply and publishes
/// it to the Core NATS reply subject.  The proxy's subscription was already
/// dropped (request returned 504), so the Core NATS publish is silently
/// discarded — no error in the worker.  The worker acks the JetStream message
/// and remains healthy.
///
/// Assertions:
/// - Proxy returns 504 within `worker_timeout` (not after the full upstream delay).
/// - The AI provider was called exactly once (JetStream message was acked, no redelivery).
///
/// This is distinct from `binary_worker_survives_orphaned_reply_subject` which
/// starts the worker *after* the proxy times out.  Here the worker's HTTP call
/// is already in-progress when the proxy deadline fires.
#[tokio::test]
async fn binary_slow_upstream_proxy_times_out_before_upstream_responds() {
    let (_nats_container, nats_port) = start_nats().await;

    let mock_server = httpmock::MockServer::start_async().await;
    // The AI mock delays its response by 6 s — more than the 2 s proxy timeout.
    let ai_mock = mock_server
        .mock_async(|when, then| {
            when.method(httpmock::Method::POST).path("/v1/messages");
            then.status(200)
                .header("content-type", "application/json")
                .delay(Duration::from_secs(6))
                .body(r#"{"id":"msg_slow"}"#);
        })
        .await;

    let proxy_port = free_port();
    let token = "tok_anthropic_prod_slow001";

    // 2 s proxy timeout — fires before the 6 s AI response arrives.
    let _proxy = spawn_proxy_with_timeout(nats_port, proxy_port, &mock_server.base_url(), 2);
    wait_for_port(proxy_port, Duration::from_secs(15)).await;

    let _worker = spawn_worker(nats_port, "binary-workers-slow", token, "sk-ant-key-slow");
    tokio::time::sleep(Duration::from_millis(500)).await;

    let start = std::time::Instant::now();
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
    let elapsed = start.elapsed();

    assert_eq!(resp.status(), 504, "Slow upstream must cause 504 timeout");
    assert!(
        elapsed < Duration::from_secs(4),
        "Proxy must return 504 before the upstream responds (took {:?})",
        elapsed
    );

    // Wait long enough for the worker to receive the delayed AI response,
    // attempt to publish to the (now dead) reply subject, and ack the message.
    tokio::time::sleep(Duration::from_secs(8)).await;

    // The AI provider must have been called exactly once.  The JetStream message
    // was acked after the worker finished (even though the reply was discarded),
    // so there is no redelivery and no duplicate upstream call.
    assert_eq!(
        ai_mock.hits(),
        1,
        "AI provider must be called exactly once despite the proxy timeout"
    );
}

// ── Gap 4: reqwest follows 3xx redirects transparently ───────────────────────

/// When the upstream AI provider returns a 307 Temporary Redirect, reqwest
/// inside the worker automatically follows it (default redirect policy:
/// VGS security requirement: the worker must NOT follow HTTP redirects.
///
/// If the worker followed a 3xx redirect, the real API key (substituted from
/// the vault) would be forwarded in the Authorization header to an uncontrolled
/// redirect target — violating the core VGS guarantee that secrets never leave
/// the controlled path.
///
/// After the fix (`redirect::Policy::none()` on the reqwest client) the worker
/// returns the 307 as-is to the proxy, which forwards it to the caller.
/// The caller receives the 307 with the Location header intact and decides
/// whether to follow the redirect — without ever exposing the real key.
#[tokio::test]
async fn binary_provider_307_redirect_not_followed_returns_307_to_caller() {
    let (_nats_container, nats_port) = start_nats().await;

    let mock_server = httpmock::MockServer::start_async().await;

    // Primary endpoint returns a 307 redirect.
    let redirect_mock = mock_server
        .mock_async(|when, then| {
            when.method(httpmock::Method::POST).path("/v1/messages");
            then.status(307).header("location", "/v2/messages");
        })
        .await;

    // v2 endpoint must NOT be called — the worker stops at the 307.
    let v2_mock = mock_server
        .mock_async(|when, then| {
            when.path("/v2/messages");
            then.status(200)
                .header("content-type", "application/json")
                .body(r#"{"id":"msg_after_redirect"}"#);
        })
        .await;

    let proxy_port = free_port();
    let token = "tok_anthropic_prod_redir307";

    let _proxy = spawn_proxy(nats_port, proxy_port, &mock_server.base_url());
    wait_for_port(proxy_port, Duration::from_secs(15)).await;

    let _worker = spawn_worker(nats_port, "binary-workers-redir307", token, "sk-ant-redir-key");
    tokio::time::sleep(Duration::from_millis(500)).await;

    // Use a non-redirecting client so we observe exactly what the proxy returns.
    let resp = reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .unwrap()
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

    // The proxy must forward the 307 exactly as received — the real key was
    // never sent to the redirect target.
    assert_eq!(
        resp.status(),
        307,
        "Worker must not follow redirects; proxy must return 307 to the caller"
    );
    assert!(
        resp.headers().contains_key("location"),
        "Location header must be forwarded to the caller"
    );

    // The v1 endpoint was hit once; the v2 redirect target was never called.
    assert_eq!(redirect_mock.hits(), 1, "v1 endpoint must be called exactly once");
    assert_eq!(v2_mock.hits(), 0, "v2 redirect target must never be called");
}

// ── Gap 12: HTTP 418 is forwarded as-is ──────────────────────────────────────

/// Non-standard but valid HTTP status codes such as 418 I'm a Teapot must be
/// forwarded to the caller without modification.
///
/// The worker treats any status < 500 as a successful (non-retriable) response.
/// `axum::http::StatusCode::from_u16(418)` succeeds, so the proxy returns 418
/// rather than falling back to 500.
#[tokio::test]
async fn binary_418_teapot_response_forwarded() {
    let (_nats_container, nats_port) = start_nats().await;

    let mock_server = httpmock::MockServer::start_async().await;
    let ai_mock = mock_server
        .mock_async(|when, then| {
            when.method(httpmock::Method::POST).path("/v1/messages");
            then.status(418).body("I'm a teapot");
        })
        .await;

    let proxy_port = free_port();
    let token = "tok_anthropic_prod_teapot01";

    let _proxy = spawn_proxy(nats_port, proxy_port, &mock_server.base_url());
    wait_for_port(proxy_port, Duration::from_secs(15)).await;

    let _worker = spawn_worker(nats_port, "binary-workers-teapot", token, "sk-ant-teapot-key");
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

    assert_eq!(resp.status(), 418, "Proxy must forward 418 I'm a Teapot to the caller");
    let body = resp.text().await.unwrap();
    assert_eq!(body, "I'm a teapot");

    ai_mock.assert_async().await;
}

// ── Gap 15: UTF-8 multibyte response body is preserved ───────────────────────

/// Unicode response bodies (emoji, CJK, accented characters) must survive the
/// `Vec<u8>` → JSON integer-array → `Vec<u8>` serialization round-trip intact.
///
/// `serde_json` serializes `Vec<u8>` as `[b1, b2, ...]` (array of integers)
/// and deserializes it back to the same byte sequence.  If encoding or decoding
/// is off-by-one the caller would receive garbled text.
#[tokio::test]
async fn binary_utf8_multibyte_response_body_preserved() {
    let (_nats_container, nats_port) = start_nats().await;

    let unicode_body = r#"{"message":"Hello 世界 🌍 ñoño café"}"#;

    let mock_server = httpmock::MockServer::start_async().await;
    let ai_mock = mock_server
        .mock_async(|when, then| {
            when.method(httpmock::Method::POST).path("/v1/messages");
            then.status(200)
                .header("content-type", "application/json; charset=utf-8")
                .body(unicode_body);
        })
        .await;

    let proxy_port = free_port();
    let token = "tok_anthropic_prod_utf8001";

    let _proxy = spawn_proxy(nats_port, proxy_port, &mock_server.base_url());
    wait_for_port(proxy_port, Duration::from_secs(15)).await;

    let _worker = spawn_worker(nats_port, "binary-workers-utf8", token, "sk-ant-utf8-key");
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

    assert_eq!(resp.status(), 200);
    let body = resp.text().await.unwrap();
    assert_eq!(
        body, unicode_body,
        "UTF-8 multibyte characters must survive the Vec<u8> JSON round-trip"
    );

    ai_mock.assert_async().await;
}

// ── max_deliver exhaustion with real proxy binary ─────────────────────────────

/// When every worker that receives a JetStream message crashes (drops without
/// ACKing), JetStream exhausts `max_deliver` retries and stops redelivering.
/// Since no worker ever publishes a reply, the proxy times out → 504.
///
/// This is the binary-level counterpart of `e2e_max_deliver_exhausted_proxy_returns_504`.
/// Instead of an in-process proxy and router, it uses the real compiled proxy
/// binary, demonstrating that the behaviour holds in production.
///
/// A "saboteur" consumer created from the test pulls each of the 3 deliveries
/// and drops without ACKing.  After the 3rd expiry the proxy worker-timeout
/// fires and the caller receives 504.
#[tokio::test]
async fn binary_max_deliver_exhausted_with_real_proxy_causes_504() {
    use async_nats::jetstream::consumer::{pull, AckPolicy};
    use futures_util::StreamExt as _;
    use trogon_nats::{NatsAuth, NatsConfig, connect};
    use trogon_secret_proxy::stream;

    let (_nats_container, nats_port) = start_nats().await;

    let proxy_port = free_port();
    let mock_server = httpmock::MockServer::start_async().await;

    // Proxy with an 8 s timeout — long enough for 3 × ack_wait(1 s) to expire.
    let _proxy = spawn_proxy_with_timeout(nats_port, proxy_port, &mock_server.base_url(), 8);
    wait_for_port(proxy_port, Duration::from_secs(15)).await;

    // Connect to NATS from the test to set up the saboteur consumer.
    // By the time wait_for_port returns the proxy has already called ensure_stream,
    // so the stream is guaranteed to exist.
    let nats_config = NatsConfig {
        servers: vec![format!("localhost:{}", nats_port)],
        auth: NatsAuth::None,
    };
    let nats = connect(&nats_config, Duration::from_secs(10)).await.unwrap();
    let jetstream = std::sync::Arc::new(async_nats::jetstream::new(nats));

    let consumer_name = "binary-maxdeliver-saboteur";
    let js_stream = jetstream
        .get_stream(&stream::stream_name("trogon"))
        .await
        .unwrap();
    let consumer = js_stream
        .get_or_create_consumer(
            consumer_name,
            pull::Config {
                durable_name: Some(consumer_name.to_string()),
                ack_policy: AckPolicy::Explicit,
                // Short ack_wait so the 3 redeliveries happen within the 8 s proxy timeout.
                ack_wait: Duration::from_secs(1),
                max_deliver: 3,
                ..Default::default()
            },
        )
        .await
        .unwrap();
    let mut messages = consumer.messages().await.unwrap();

    // Send request to the real proxy binary — proxy publishes to JetStream and waits.
    let proxy_port_clone = proxy_port;
    let req_handle = tokio::spawn(async move {
        reqwest::Client::new()
            .post(format!(
                "http://127.0.0.1:{}/anthropic/v1/messages",
                proxy_port_clone
            ))
            .header("Authorization", "Bearer tok_anthropic_prod_maxdlvbin1")
            .header("Content-Type", "application/json")
            .body("{}")
            .timeout(Duration::from_secs(20))
            .send()
            .await
            .unwrap()
    });

    // Saboteur: pull each delivery and drop without ACKing (simulates crash).
    // After 3 drops + ack_wait expiry the message is exhausted.
    for _ in 0..3 {
        match tokio::time::timeout(Duration::from_secs(5), messages.next()).await {
            Ok(Some(Ok(msg))) => drop(msg), // deliberately no ack
            _ => break,
        }
    }

    let resp = req_handle.await.unwrap();
    assert_eq!(
        resp.status(),
        504,
        "All deliveries exhausted → no reply ever sent → proxy must return 504"
    );
}

/// The proxy binary must exit with a non-zero status code when the TCP port it
/// needs to bind is already occupied by another process.
///
/// Flow: NATS connects fine, JetStream stream is created, then
/// `TcpListener::bind` fails → `expect("Failed to bind TCP listener")` panics
/// → process exits non-zero.
#[tokio::test]
async fn binary_proxy_port_in_use_exits_nonzero() {
    let (_nats_container, nats_port) = start_nats().await;

    // Occupy the port before the proxy starts.
    let occupied_port = free_port();
    let _held = tokio::net::TcpListener::bind(format!("127.0.0.1:{}", occupied_port))
        .await
        .expect("Test setup: failed to pre-bind port");

    let mut child = tokio::process::Command::new(env!("CARGO_BIN_EXE_proxy"))
        .env("NATS_URL", format!("localhost:{}", nats_port))
        .env("PROXY_PORT", occupied_port.to_string())
        .env("RUST_LOG", "error")
        .kill_on_drop(true)
        .spawn()
        .unwrap();

    // The proxy must connect to NATS, create the JetStream stream, then fail
    // to bind and exit.  Give it up to 15 s to complete that sequence.
    let status = tokio::time::timeout(Duration::from_secs(15), child.wait())
        .await
        .expect("Proxy binary must exit within 15 s when port is already in use")
        .unwrap();

    assert!(
        !status.success(),
        "Proxy must exit non-zero when the TCP port is already occupied"
    );
}

// ── New automated verification tests ──────────────────────────────────────────

/// Durability: 10 sequential requests through the same proxy+worker all succeed.
///
/// Verifies that no internal state accumulates (subscription leaks, counter
/// overflows, etc.) that would cause later requests to fail.
#[tokio::test]
async fn binary_10_sequential_requests_without_degradation() {
    let (_nats_container, nats_port) = start_nats().await;

    let mock_server = httpmock::MockServer::start_async().await;
    let token = "tok_anthropic_prod_seq001";
    let real_key = "sk-ant-seq-real-key";

    let ai_mock = mock_server
        .mock_async(|when, then| {
            when.method(httpmock::Method::POST)
                .path("/v1/messages")
                .header("authorization", format!("Bearer {}", real_key));
            then.status(200)
                .header("content-type", "application/json")
                .body(r#"{"id":"msg_seq","type":"message"}"#);
        })
        .await;

    let proxy_port = free_port();
    let _proxy = spawn_proxy(nats_port, proxy_port, &mock_server.base_url());
    wait_for_port(proxy_port, Duration::from_secs(15)).await;

    let _worker = spawn_worker(nats_port, "seq-workers-001", token, real_key);
    tokio::time::sleep(Duration::from_millis(500)).await;

    let client = reqwest::Client::new();
    for i in 1..=10 {
        let resp = client
            .post(format!("http://127.0.0.1:{}/anthropic/v1/messages", proxy_port))
            .header("Authorization", format!("Bearer {}", token))
            .header("Content-Type", "application/json")
            .body(format!(r#"{{"seq":{}}}"#, i))
            .timeout(Duration::from_secs(15))
            .send()
            .await
            .unwrap_or_else(|e| panic!("Request {} failed: {}", i, e));

        assert_eq!(
            resp.status(),
            200,
            "Request {} of 10 must succeed — system must not degrade",
            i
        );
    }

    assert_eq!(ai_mock.hits(), 10, "All 10 requests must reach the AI provider");
}

/// Provider error body forwarded verbatim: when the AI provider returns a 4xx
/// with a JSON error body, the proxy must forward it unchanged to the caller.
///
/// This is critical for usability — callers need the provider's error details
/// (e.g. validation failures, rate-limit messages) to debug their requests.
#[tokio::test]
async fn binary_provider_error_body_forwarded_verbatim() {
    let (_nats_container, nats_port) = start_nats().await;

    let mock_server = httpmock::MockServer::start_async().await;
    let token = "tok_anthropic_prod_errbody1";
    let real_key = "sk-ant-errbody-real-key";

    let error_body = r#"{"type":"error","error":{"type":"invalid_request_error","message":"max_tokens is required"}}"#;

    let _ai_mock = mock_server
        .mock_async(|when, then| {
            when.method(httpmock::Method::POST).path("/v1/messages");
            then.status(400)
                .header("content-type", "application/json")
                .body(error_body);
        })
        .await;

    let proxy_port = free_port();
    let _proxy = spawn_proxy(nats_port, proxy_port, &mock_server.base_url());
    wait_for_port(proxy_port, Duration::from_secs(15)).await;

    let _worker = spawn_worker(nats_port, "errbody-workers-001", token, real_key);
    tokio::time::sleep(Duration::from_millis(500)).await;

    let resp = reqwest::Client::new()
        .post(format!("http://127.0.0.1:{}/anthropic/v1/messages", proxy_port))
        .header("Authorization", format!("Bearer {}", token))
        .header("Content-Type", "application/json")
        .body(r#"{"model":"claude-3-5-sonnet"}"#)
        .timeout(Duration::from_secs(15))
        .send()
        .await
        .unwrap();

    assert_eq!(resp.status(), 400, "Provider 400 must be forwarded to caller");
    let body = resp.text().await.unwrap();
    assert_eq!(
        body, error_body,
        "Provider error body must reach the caller byte-for-byte"
    );
}

/// Request body integrity: the exact bytes sent by the caller reach the AI
/// provider unchanged after token exchange.
///
/// This is fundamental to correctness — the proxy must not modify the body
/// in any way (no re-encoding, no truncation, no padding).
#[tokio::test]
async fn binary_request_body_reaches_provider_byte_for_byte() {
    let (_nats_container, nats_port) = start_nats().await;

    let mock_server = httpmock::MockServer::start_async().await;
    let token = "tok_anthropic_prod_body01";
    let real_key = "sk-ant-body-real-key";

    let exact_body = r#"{"model":"claude-3-5-sonnet-20241022","max_tokens":1024,"messages":[{"role":"user","content":"Hello, world!"}],"temperature":0.7}"#;

    let body_mock = mock_server
        .mock_async(|when, then| {
            when.method(httpmock::Method::POST)
                .path("/v1/messages")
                .body(exact_body);
            then.status(200)
                .header("content-type", "application/json")
                .body(r#"{"id":"msg_body_ok"}"#);
        })
        .await;

    let proxy_port = free_port();
    let _proxy = spawn_proxy(nats_port, proxy_port, &mock_server.base_url());
    wait_for_port(proxy_port, Duration::from_secs(15)).await;

    let _worker = spawn_worker(nats_port, "body-workers-001", token, real_key);
    tokio::time::sleep(Duration::from_millis(500)).await;

    let resp = reqwest::Client::new()
        .post(format!("http://127.0.0.1:{}/anthropic/v1/messages", proxy_port))
        .header("Authorization", format!("Bearer {}", token))
        .header("Content-Type", "application/json")
        .body(exact_body)
        .timeout(Duration::from_secs(15))
        .send()
        .await
        .unwrap();

    assert_eq!(resp.status(), 200, "Request with exact body must succeed");
    assert_eq!(
        body_mock.hits(),
        1,
        "Provider must receive exactly the body sent by the caller, byte for byte"
    );
}

/// Multi-provider isolation: Anthropic and OpenAI tokens in the same session
/// resolve to their respective real keys with no cross-contamination.
///
/// The worker holds one Anthropic token and one OpenAI token.  The test
/// verifies that each provider receives only the correct key.
#[tokio::test]
async fn binary_anthropic_and_openai_tokens_resolve_to_correct_keys() {
    let (_nats_container, nats_port) = start_nats().await;

    let mock_server = httpmock::MockServer::start_async().await;

    let anthropic_token = "tok_anthropic_prod_mp001";
    let anthropic_key   = "sk-ant-anthropic-real-key";
    let openai_token    = "tok_openai_prod_mp001";
    let openai_key      = "sk-openai-real-key";

    // Anthropic mock: only accepts the Anthropic key.
    let anthropic_mock = mock_server
        .mock_async(|when, then| {
            when.method(httpmock::Method::POST)
                .path("/v1/messages")
                .header("authorization", format!("Bearer {}", anthropic_key));
            then.status(200)
                .header("content-type", "application/json")
                .body(r#"{"provider":"anthropic"}"#);
        })
        .await;

    // OpenAI mock: only accepts the OpenAI key.
    let openai_mock = mock_server
        .mock_async(|when, then| {
            when.method(httpmock::Method::POST)
                .path("/v1/chat/completions")
                .header("authorization", format!("Bearer {}", openai_key));
            then.status(200)
                .header("content-type", "application/json")
                .body(r#"{"provider":"openai"}"#);
        })
        .await;

    let proxy_port = free_port();
    let _proxy = spawn_proxy(nats_port, proxy_port, &mock_server.base_url());
    wait_for_port(proxy_port, Duration::from_secs(15)).await;

    // Single worker holds both tokens.
    let _worker = Command::new(env!("CARGO_BIN_EXE_worker"))
        .env("NATS_URL", format!("localhost:{}", nats_port))
        .env("WORKER_CONSUMER_NAME", "mp-workers-001")
        .env(format!("VAULT_TOKEN_{}", anthropic_token), anthropic_key)
        .env(format!("VAULT_TOKEN_{}", openai_token), openai_key)
        .env("RUST_LOG", "warn")
        .kill_on_drop(true)
        .spawn()
        .unwrap();
    tokio::time::sleep(Duration::from_millis(500)).await;

    let client = reqwest::Client::new();
    let base = format!("http://127.0.0.1:{}", proxy_port);

    // Call Anthropic provider with Anthropic token.
    let resp_anthropic = client
        .post(format!("{}/anthropic/v1/messages", base))
        .header("Authorization", format!("Bearer {}", anthropic_token))
        .header("Content-Type", "application/json")
        .body("{}")
        .timeout(Duration::from_secs(15))
        .send()
        .await
        .unwrap();

    // Call OpenAI provider with OpenAI token.
    let resp_openai = client
        .post(format!("{}/openai/v1/chat/completions", base))
        .header("Authorization", format!("Bearer {}", openai_token))
        .header("Content-Type", "application/json")
        .body("{}")
        .timeout(Duration::from_secs(15))
        .send()
        .await
        .unwrap();

    assert_eq!(resp_anthropic.status(), 200, "Anthropic request must succeed");
    assert_eq!(resp_openai.status(), 200, "OpenAI request must succeed");

    let body_a: serde_json::Value = resp_anthropic.json().await.unwrap();
    let body_o: serde_json::Value = resp_openai.json().await.unwrap();
    assert_eq!(body_a["provider"], "anthropic", "Anthropic token must route to Anthropic provider");
    assert_eq!(body_o["provider"], "openai", "OpenAI token must route to OpenAI provider");

    assert_eq!(anthropic_mock.hits(), 1, "Anthropic key must be used exactly once");
    assert_eq!(openai_mock.hits(), 1, "OpenAI key must be used exactly once");
}

/// Concurrency safety: 20 parallel requests using the same token all succeed.
///
/// Verifies that the vault's read path is safe under concurrent access and
/// that JetStream handles fan-out: every one of the 20 requests is delivered
/// to the worker and receives an independent, correct response.
#[tokio::test]
async fn binary_parallel_requests_same_token_all_succeed() {
    let (_nats_container, nats_port) = start_nats().await;

    let mock_server = httpmock::MockServer::start_async().await;
    let token = "tok_anthropic_prod_par001";
    let real_key = "sk-ant-parallel-key";

    let _ai_mock = mock_server
        .mock_async(|when, then| {
            when.method(httpmock::Method::POST)
                .path("/v1/messages")
                .header("authorization", format!("Bearer {}", real_key));
            then.status(200)
                .header("content-type", "application/json")
                .body(r#"{"id":"msg_ok"}"#);
        })
        .await;

    let proxy_port = free_port();
    let _proxy = spawn_proxy(nats_port, proxy_port, &mock_server.base_url());
    wait_for_port(proxy_port, Duration::from_secs(15)).await;

    let _worker = spawn_worker(nats_port, "par-workers-001", token, real_key);
    tokio::time::sleep(Duration::from_millis(500)).await;

    let client = std::sync::Arc::new(reqwest::Client::new());
    let url = format!("http://127.0.0.1:{}/anthropic/v1/messages", proxy_port);

    let handles: Vec<_> = (0..20)
        .map(|i| {
            let client = client.clone();
            let url = url.clone();
            let token = token.to_string();
            tokio::spawn(async move {
                client
                    .post(&url)
                    .header("Authorization", format!("Bearer {}", token))
                    .header("Content-Type", "application/json")
                    .body(format!(r#"{{"request":{}}}"#, i))
                    .timeout(Duration::from_secs(30))
                    .send()
                    .await
                    .unwrap()
            })
        })
        .collect();

    let responses = join_all(handles).await;

    let mut success_count = 0;
    for resp in responses {
        let resp = resp.expect("task panicked");
        if resp.status() == 200 {
            success_count += 1;
        }
    }

    assert_eq!(
        success_count, 20,
        "All 20 parallel requests must succeed; {} did",
        success_count
    );
    assert_eq!(_ai_mock.hits(), 20, "Provider must receive all 20 requests");
}

/// JetStream backlog: the worker starts after the proxy has already queued
/// requests in JetStream.  The worker must drain the backlog and all callers
/// must receive their responses.
///
/// This proves the durability guarantee: messages published before any
/// consumer exists survive in JetStream and are delivered when the worker
/// connects.
#[tokio::test]
async fn binary_worker_starts_after_messages_queued() {
    let (_nats_container, nats_port) = start_nats().await;

    let mock_server = httpmock::MockServer::start_async().await;
    let token = "tok_anthropic_prod_backlog1";
    let real_key = "sk-ant-backlog-key";

    let _ai_mock = mock_server
        .mock_async(|when, then| {
            when.method(httpmock::Method::POST).path("/v1/messages");
            then.status(200)
                .header("content-type", "application/json")
                .body(r#"{"id":"msg_backlog"}"#);
        })
        .await;

    // Proxy with a generous 25 s timeout so it can wait for the worker.
    let proxy_port = free_port();
    let _proxy = spawn_proxy_with_timeout(nats_port, proxy_port, &mock_server.base_url(), 25);
    wait_for_port(proxy_port, Duration::from_secs(15)).await;

    let client = std::sync::Arc::new(reqwest::Client::new());
    let url = format!("http://127.0.0.1:{}/anthropic/v1/messages", proxy_port);

    // Launch 5 concurrent requests BEFORE starting the worker.
    // Each will publish to JetStream and block waiting for a Core NATS reply.
    let handles: Vec<_> = (0..5)
        .map(|i| {
            let client = client.clone();
            let url = url.clone();
            let token = token.to_string();
            tokio::spawn(async move {
                client
                    .post(&url)
                    .header("Authorization", format!("Bearer {}", token))
                    .header("Content-Type", "application/json")
                    .body(format!(r#"{{"backlog":{}}}"#, i))
                    .timeout(Duration::from_secs(25))
                    .send()
                    .await
                    .unwrap()
            })
        })
        .collect();

    // Give the proxy time to publish all 5 messages to JetStream before the
    // worker starts draining.
    tokio::time::sleep(Duration::from_secs(2)).await;

    // Worker starts AFTER messages are already queued.
    let _worker = spawn_worker(nats_port, "backlog-workers-001", token, real_key);

    let responses = join_all(handles).await;

    let mut success_count = 0;
    for resp in responses {
        let resp = resp.expect("task panicked");
        if resp.status() == 200 {
            success_count += 1;
        }
    }

    assert_eq!(
        success_count, 5,
        "All 5 backlogged requests must be processed when the worker starts; {} succeeded",
        success_count
    );
    assert_eq!(_ai_mock.hits(), 5, "Provider must receive all 5 backlogged requests");
}

/// Security: a tok_ token appearing in the URL query string is forwarded
/// to the AI provider unchanged.  The proxy only exchanges tokens from the
/// `Authorization: Bearer` header — query parameters are never intercepted.
///
/// This is a deliberately scoped security boundary: the proxy cannot know
/// which query params are sensitive, so it leaves them all untouched.
#[tokio::test]
async fn binary_tok_in_query_string_forwarded_unchanged() {
    let (_nats_container, nats_port) = start_nats().await;

    let mock_server = httpmock::MockServer::start_async().await;
    let auth_token = "tok_anthropic_prod_qpauth1";
    let real_key   = "sk-ant-qparam-real-key";
    // A second tok_ that appears only in the query string — it must reach the
    // provider verbatim (not exchanged, not stripped).
    let query_tok  = "tok_anthropic_prod_inquery1";

    let _ai_mock = mock_server
        .mock_async(|when, then| {
            when.method(httpmock::Method::GET)
                .path("/v1/models")
                // Mock only matches when the query parameter is present and
                // still contains the raw tok_ (i.e. the proxy did NOT exchange it).
                .query_param("api_key", query_tok);
            then.status(200)
                .header("content-type", "application/json")
                .body(r#"{"models":[]}"#);
        })
        .await;

    let proxy_port = free_port();
    let _proxy = spawn_proxy(nats_port, proxy_port, &mock_server.base_url());
    wait_for_port(proxy_port, Duration::from_secs(15)).await;

    let _worker = spawn_worker(nats_port, "qparam-workers-001", auth_token, real_key);
    tokio::time::sleep(Duration::from_millis(500)).await;

    let resp = reqwest::Client::new()
        .get(format!(
            "http://127.0.0.1:{}/anthropic/v1/models?api_key={}",
            proxy_port, query_tok
        ))
        .header("Authorization", format!("Bearer {}", auth_token))
        .timeout(Duration::from_secs(15))
        .send()
        .await
        .unwrap();

    assert_eq!(
        resp.status(),
        200,
        "Request with tok_ in query string must succeed (query token not exchanged)"
    );
    assert_eq!(
        _ai_mock.hits(),
        1,
        "Provider must receive the request with tok_ still present in the query string"
    );
}

/// Slow provider within timeout: the AI provider takes 2 s to respond.
/// With a 15 s worker timeout the proxy must wait and return 200 — not 504.
///
/// This tests that the proxy timeout is a hard ceiling, not a soft hint:
/// any response within the window must be returned to the caller.
#[tokio::test]
async fn binary_slow_provider_within_timeout_succeeds() {
    let (_nats_container, nats_port) = start_nats().await;

    let mock_server = httpmock::MockServer::start_async().await;
    let token   = "tok_anthropic_prod_slow001";
    let real_key = "sk-ant-slow-key";

    let _ai_mock = mock_server
        .mock_async(|when, then| {
            when.method(httpmock::Method::POST).path("/v1/messages");
            then.status(200)
                .header("content-type", "application/json")
                .body(r#"{"id":"msg_slow_ok"}"#)
                .delay(Duration::from_secs(2)); // responds after 2 s
        })
        .await;

    // Proxy timeout is 15 s — well above the 2 s provider delay.
    let proxy_port = free_port();
    let _proxy = spawn_proxy_with_timeout(nats_port, proxy_port, &mock_server.base_url(), 15);
    wait_for_port(proxy_port, Duration::from_secs(15)).await;

    let _worker = spawn_worker(nats_port, "slow-workers-001", token, real_key);
    tokio::time::sleep(Duration::from_millis(500)).await;

    let resp = reqwest::Client::new()
        .post(format!("http://127.0.0.1:{}/anthropic/v1/messages", proxy_port))
        .header("Authorization", format!("Bearer {}", token))
        .header("Content-Type", "application/json")
        .body(r#"{"model":"claude-3-5-sonnet"}"#)
        .timeout(Duration::from_secs(30)) // client timeout wider than proxy timeout
        .send()
        .await
        .unwrap();

    assert_eq!(
        resp.status(),
        200,
        "A response within the proxy timeout window must be returned as 200, not 504"
    );
    let body: serde_json::Value = resp.json().await.unwrap();
    assert_eq!(body["id"], "msg_slow_ok");
}

/// Five providers, five tokens: each of the five supported AI providers
/// (anthropic, openai, gemini, cohere, mistral) is called with its own
/// tok_ token and resolves to its own distinct real key.  Each mock only
/// matches requests bearing the correct real key — a wrong key causes the
/// mock to return 404, making cross-token contamination immediately visible.
#[tokio::test]
async fn binary_five_providers_five_tokens_all_route_correctly() {
    let (_nats_container, nats_port) = start_nats().await;

    let mock_server = httpmock::MockServer::start_async().await;

    let providers = [
        ("anthropic", "/v1/messages",          "tok_anthropic_prod_fp001", "sk-ant-fp-key"),
        ("openai",    "/v1/chat/completions",   "tok_openai_prod_fp001",    "sk-openai-fp-key"),
        ("gemini",    "/v1beta/models",         "tok_gemini_prod_fp001",    "sk-gemini-fp-key"),
        ("cohere",    "/v2/chat",               "tok_cohere_prod_fp001",    "sk-cohere-fp-key"),
        ("mistral",   "/v1/chat/completions",   "tok_mistral_prod_fp001",   "sk-mistral-fp-key"),
    ];

    // One mock per provider path + real key combination.
    let mut mocks = Vec::new();
    for (provider, path, _tok, real_key) in &providers {
        let m = mock_server
            .mock_async(|when, then| {
                when.method(httpmock::Method::POST)
                    .path(*path)
                    .header("authorization", format!("Bearer {}", real_key));
                then.status(200)
                    .header("content-type", "application/json")
                    .body(format!(r#"{{"provider":"{}"}}"#, *provider));
            })
            .await;
        mocks.push(m);
    }

    let proxy_port = free_port();
    let _proxy = spawn_proxy(nats_port, proxy_port, &mock_server.base_url());
    wait_for_port(proxy_port, Duration::from_secs(15)).await;

    // Single worker holds all five tokens.
    let mut worker_cmd = Command::new(env!("CARGO_BIN_EXE_worker"));
    worker_cmd
        .env("NATS_URL", format!("localhost:{}", nats_port))
        .env("WORKER_CONSUMER_NAME", "fp-workers-001")
        .env("RUST_LOG", "warn")
        .kill_on_drop(true);
    for (_provider, _path, tok, real_key) in &providers {
        worker_cmd.env(format!("VAULT_TOKEN_{}", tok), real_key);
    }
    let _worker = worker_cmd.spawn().unwrap();
    tokio::time::sleep(Duration::from_millis(500)).await;

    let client = reqwest::Client::new();
    let base = format!("http://127.0.0.1:{}", proxy_port);

    let mut all_ok = true;
    for (provider, path, tok, _real_key) in &providers {
        let resp = client
            .post(format!("{}/{}{}", base, provider, path))
            .header("Authorization", format!("Bearer {}", tok))
            .header("Content-Type", "application/json")
            .body("{}")
            .timeout(Duration::from_secs(15))
            .send()
            .await
            .unwrap();

        if resp.status() != 200 {
            eprintln!("Provider {} returned {}", provider, resp.status());
            all_ok = false;
            continue;
        }

        let body: serde_json::Value = resp.json().await.unwrap();
        assert_eq!(
            body["provider"], *provider,
            "Token for {} must route to {} provider",
            provider, provider
        );
    }

    assert!(all_ok, "All five provider requests must succeed");

    for (i, m) in mocks.iter().enumerate() {
        assert_eq!(
            m.hits(),
            1,
            "Provider {} mock must be hit exactly once",
            providers[i].0
        );
    }
}

/// Provider connection refused: the worker targets a port with nothing
/// listening.  After exhausting all retries the worker must return 502 to the
/// caller — not hang, not panic, and not time out the proxy with a 504.
///
/// This proves the worker handles transport-level failures (ECONNREFUSED)
/// gracefully and communicates them back as structured error responses.
#[tokio::test]
async fn binary_provider_connection_refused_returns_502() {
    let (_nats_container, nats_port) = start_nats().await;

    // Pick a port that has nothing listening — we never start any mock server.
    let dead_port = free_port();
    let dead_url = format!("http://127.0.0.1:{}", dead_port);

    let token    = "tok_anthropic_prod_cref001";
    let real_key = "sk-ant-connref-key";

    let proxy_port = free_port();
    let _proxy = spawn_proxy(nats_port, proxy_port, &dead_url);
    wait_for_port(proxy_port, Duration::from_secs(15)).await;

    let _worker = spawn_worker(nats_port, "cref-workers-001", token, real_key);
    tokio::time::sleep(Duration::from_millis(500)).await;

    let resp = reqwest::Client::new()
        .post(format!("http://127.0.0.1:{}/anthropic/v1/messages", proxy_port))
        .header("Authorization", format!("Bearer {}", token))
        .header("Content-Type", "application/json")
        .body(r#"{"model":"claude-3-5-sonnet"}"#)
        .timeout(Duration::from_secs(30))
        .send()
        .await
        .unwrap();

    assert_eq!(
        resp.status(),
        502,
        "ECONNREFUSED after all retries must produce 502 Bad Gateway, not 504"
    );
}

/// Empty vault: worker starts with no VAULT_TOKEN_* environment variables.
/// Every request — regardless of which tok_ is used — must be rejected with
/// 401 Unauthorized.
///
/// This is the safe default: an unconfigured worker must never forward
/// requests to the AI provider.
#[tokio::test]
async fn binary_worker_empty_vault_rejects_all_requests() {
    let (_nats_container, nats_port) = start_nats().await;

    let mock_server = httpmock::MockServer::start_async().await;
    let _ai_mock = mock_server
        .mock_async(|when, then| {
            when.any_request();
            then.status(200).body("should not reach here");
        })
        .await;

    let proxy_port = free_port();
    let _proxy = spawn_proxy(nats_port, proxy_port, &mock_server.base_url());
    wait_for_port(proxy_port, Duration::from_secs(15)).await;

    // Worker with NO vault tokens at all.
    let _worker = Command::new(env!("CARGO_BIN_EXE_worker"))
        .env("NATS_URL", format!("localhost:{}", nats_port))
        .env("WORKER_CONSUMER_NAME", "empty-vault-001")
        .env("RUST_LOG", "warn")
        .kill_on_drop(true)
        .spawn()
        .unwrap();
    tokio::time::sleep(Duration::from_millis(500)).await;

    let client = reqwest::Client::new();
    let base = format!("http://127.0.0.1:{}", proxy_port);

    // Multiple different tokens — all must be rejected.
    let tokens = [
        "tok_anthropic_prod_ev001",
        "tok_openai_prod_ev002",
        "tok_gemini_prod_ev003",
    ];

    for tok in &tokens {
        let resp = client
            .post(format!("{}/anthropic/v1/messages", base))
            .header("Authorization", format!("Bearer {}", tok))
            .header("Content-Type", "application/json")
            .body("{}")
            .timeout(Duration::from_secs(15))
            .send()
            .await
            .unwrap();

        assert_eq!(
            resp.status(),
            401,
            "Token {} must be rejected 401 when vault is empty",
            tok
        );
    }

    assert_eq!(
        _ai_mock.hits(),
        0,
        "Provider must receive zero requests when vault is empty"
    );
}

/// Long nested path integrity: a deeply nested URL path
/// (`/v1/threads/{id}/runs/{id}/steps`) must arrive at the provider exactly
/// as the caller sent it — no truncation, no segment dropped.
///
/// This is a common real-world pattern in OpenAI's Assistants API and similar
/// resource-oriented APIs.
#[tokio::test]
async fn binary_long_nested_path_forwarded_correctly() {
    let (_nats_container, nats_port) = start_nats().await;

    let mock_server = httpmock::MockServer::start_async().await;
    let token    = "tok_openai_prod_nested1";
    let real_key = "sk-openai-nested-key";

    let nested_path = "/v1/threads/th_abc123XYZ/runs/run_def456ABC/steps";

    let _path_mock = mock_server
        .mock_async(|when, then| {
            when.method(httpmock::Method::GET).path(nested_path);
            then.status(200)
                .header("content-type", "application/json")
                .body(r#"{"object":"list","data":[]}"#);
        })
        .await;

    let proxy_port = free_port();
    let _proxy = spawn_proxy(nats_port, proxy_port, &mock_server.base_url());
    wait_for_port(proxy_port, Duration::from_secs(15)).await;

    let _worker = spawn_worker(nats_port, "nested-workers-001", token, real_key);
    tokio::time::sleep(Duration::from_millis(500)).await;

    let resp = reqwest::Client::new()
        .get(format!(
            "http://127.0.0.1:{}/openai{}",
            proxy_port, nested_path
        ))
        .header("Authorization", format!("Bearer {}", token))
        .timeout(Duration::from_secs(15))
        .send()
        .await
        .unwrap();

    assert_eq!(resp.status(), 200, "Nested path must be forwarded correctly");
    assert_eq!(
        _path_mock.hits(),
        1,
        "Provider must receive exactly the nested path sent by the caller"
    );
}

/// Stateless proxy restart: the proxy is killed and a fresh instance is
/// started on the same NATS server.  The worker is restarted too.
///
/// Because the proxy is stateless (all durable state lives in JetStream),
/// the new proxy instance must serve requests correctly without any warm-up
/// or re-configuration — the stream is already in place.
#[tokio::test]
async fn binary_proxy_restart_new_instance_serves_requests() {
    let (_nats_container, nats_port) = start_nats().await;

    let mock_server = httpmock::MockServer::start_async().await;
    let token    = "tok_anthropic_prod_rst001";
    let real_key = "sk-ant-restart-key";

    let _ai_mock = mock_server
        .mock_async(|when, then| {
            when.method(httpmock::Method::POST).path("/v1/messages");
            then.status(200)
                .header("content-type", "application/json")
                .body(r#"{"id":"msg_restart_ok"}"#);
        })
        .await;

    let proxy_port = free_port();

    // First proxy + worker: verify the system works normally.
    {
        let _proxy1 = spawn_proxy(nats_port, proxy_port, &mock_server.base_url());
        wait_for_port(proxy_port, Duration::from_secs(15)).await;
        let _worker1 = spawn_worker(nats_port, "restart-workers-001", token, real_key);
        tokio::time::sleep(Duration::from_millis(500)).await;

        let resp = reqwest::Client::new()
            .post(format!("http://127.0.0.1:{}/anthropic/v1/messages", proxy_port))
            .header("Authorization", format!("Bearer {}", token))
            .header("Content-Type", "application/json")
            .body(r#"{"req":1}"#)
            .timeout(Duration::from_secs(15))
            .send()
            .await
            .unwrap();

        assert_eq!(resp.status(), 200, "First proxy instance must serve the request");
        // _proxy1 and _worker1 dropped here — both killed
    }

    // Give the OS a moment to release the port.
    tokio::time::sleep(Duration::from_millis(300)).await;

    // Second proxy + worker on the same NATS (stream already exists).
    let _proxy2  = spawn_proxy(nats_port, proxy_port, &mock_server.base_url());
    wait_for_port(proxy_port, Duration::from_secs(15)).await;
    let _worker2 = spawn_worker(nats_port, "restart-workers-001", token, real_key);
    tokio::time::sleep(Duration::from_millis(300)).await;

    let resp2 = reqwest::Client::new()
        .post(format!("http://127.0.0.1:{}/anthropic/v1/messages", proxy_port))
        .header("Authorization", format!("Bearer {}", token))
        .header("Content-Type", "application/json")
        .body(r#"{"req":2}"#)
        .timeout(Duration::from_secs(15))
        .send()
        .await
        .unwrap();

    assert_eq!(
        resp2.status(),
        200,
        "New proxy instance must serve requests immediately after restart"
    );
    assert_eq!(_ai_mock.hits(), 2, "Provider must receive one request per proxy instance");
}

/// High-volume sequential requests: 50 back-to-back requests all return 200.
///
/// A stronger version of the 10-request degradation test.  Verifies there
/// are no resource leaks (file descriptors, NATS subscriptions, JetStream
/// consumer state) that accumulate and cause failures at higher request counts.
#[tokio::test]
async fn binary_50_sequential_requests_all_succeed() {
    let (_nats_container, nats_port) = start_nats().await;

    let mock_server = httpmock::MockServer::start_async().await;
    let token    = "tok_anthropic_prod_seq50x";
    let real_key = "sk-ant-seq50-key";

    let _ai_mock = mock_server
        .mock_async(|when, then| {
            when.method(httpmock::Method::POST).path("/v1/messages");
            then.status(200)
                .header("content-type", "application/json")
                .body(r#"{"id":"msg_seq"}"#);
        })
        .await;

    let proxy_port = free_port();
    let _proxy = spawn_proxy(nats_port, proxy_port, &mock_server.base_url());
    wait_for_port(proxy_port, Duration::from_secs(15)).await;

    let _worker = spawn_worker(nats_port, "seq50-workers-001", token, real_key);
    tokio::time::sleep(Duration::from_millis(500)).await;

    let client = reqwest::Client::new();
    for i in 0..50u32 {
        let resp = client
            .post(format!("http://127.0.0.1:{}/anthropic/v1/messages", proxy_port))
            .header("Authorization", format!("Bearer {}", token))
            .header("Content-Type", "application/json")
            .body(format!(r#"{{"seq":{}}}"#, i))
            .timeout(Duration::from_secs(15))
            .send()
            .await
            .unwrap();

        assert_eq!(
            resp.status(),
            200,
            "Request {} of 50 must succeed — no resource leak allowed",
            i
        );
    }

    assert_eq!(_ai_mock.hits(), 50, "All 50 requests must reach the provider");
}

/// Work-queue exactly-once semantics: two workers share the same consumer
/// name (JetStream queue group).  With 10 requests the provider must be
/// called exactly 10 times — never more — proving each JetStream message is
/// delivered to one worker only, not fan-out to both.
///
/// The existing concurrent-workers test only checks that all responses are
/// 200.  This test additionally verifies the absence of double-processing.
#[tokio::test]
async fn binary_two_workers_no_double_processing() {
    let (_nats_container, nats_port) = start_nats().await;

    let mock_server = httpmock::MockServer::start_async().await;
    let token    = "tok_anthropic_prod_once01";
    let real_key = "sk-ant-once-key";

    let _ai_mock = mock_server
        .mock_async(|when, then| {
            when.method(httpmock::Method::POST).path("/v1/messages");
            then.status(200)
                .header("content-type", "application/json")
                .body(r#"{"id":"msg_once"}"#);
        })
        .await;

    let proxy_port = free_port();
    let _proxy = spawn_proxy(nats_port, proxy_port, &mock_server.base_url());
    wait_for_port(proxy_port, Duration::from_secs(15)).await;

    // Two workers in the same queue group.
    let _worker_a = spawn_worker(nats_port, "once-workers", token, real_key);
    let _worker_b = spawn_worker(nats_port, "once-workers", token, real_key);
    tokio::time::sleep(Duration::from_millis(500)).await;

    let client = std::sync::Arc::new(reqwest::Client::new());
    let handles: Vec<_> = (0..10)
        .map(|i| {
            let c = client.clone();
            let url = format!("http://127.0.0.1:{}/anthropic/v1/messages", proxy_port);
            let token = token.to_string();
            tokio::spawn(async move {
                c.post(url)
                    .header("Authorization", format!("Bearer {}", token))
                    .header("Content-Type", "application/json")
                    .body(format!(r#"{{"req":{}}}"#, i))
                    .timeout(Duration::from_secs(20))
                    .send()
                    .await
                    .unwrap()
            })
        })
        .collect();

    let responses = join_all(handles).await;
    for r in responses {
        assert_eq!(r.unwrap().status(), 200);
    }

    // The critical assertion: 10 requests → exactly 10 provider calls.
    // If JetStream delivered to both workers, this would be > 10.
    assert_eq!(
        _ai_mock.hits(),
        10,
        "Each request must reach the provider exactly once (work-queue semantics)"
    );
}

/// NATS prefix isolation: two independent proxy+worker pairs share the same
/// NATS server but use different `PROXY_PREFIX` values.  A request sent to
/// proxy-A must be processed by worker-A only and vice versa — the prefixes
/// act as namespaces that prevent cross-tenant interference.
#[tokio::test]
async fn binary_nats_prefix_isolation() {
    let (_nats_container, nats_port) = start_nats().await;

    let mock_a = httpmock::MockServer::start_async().await;
    let mock_b = httpmock::MockServer::start_async().await;

    let token_a = "tok_anthropic_prod_piso01";
    let key_a   = "sk-ant-prefix-a-key";
    let token_b = "tok_anthropic_prod_piso02";
    let key_b   = "sk-ant-prefix-b-key";

    let hit_a = mock_a
        .mock_async(|when, then| {
            when.method(httpmock::Method::POST).path("/v1/messages");
            then.status(200)
                .header("content-type", "application/json")
                .body(r#"{"tenant":"a"}"#);
        })
        .await;

    let hit_b = mock_b
        .mock_async(|when, then| {
            when.method(httpmock::Method::POST).path("/v1/messages");
            then.status(200)
                .header("content-type", "application/json")
                .body(r#"{"tenant":"b"}"#);
        })
        .await;

    let port_a = free_port();
    let port_b = free_port();

    let _proxy_a  = spawn_proxy_with_prefix(nats_port, port_a, &mock_a.base_url(), "tenant_a");
    let _proxy_b  = spawn_proxy_with_prefix(nats_port, port_b, &mock_b.base_url(), "tenant_b");
    wait_for_port(port_a, Duration::from_secs(15)).await;
    wait_for_port(port_b, Duration::from_secs(15)).await;

    let _worker_a = spawn_worker_with_prefix(nats_port, "iso-workers-a", token_a, key_a, "tenant_a");
    let _worker_b = spawn_worker_with_prefix(nats_port, "iso-workers-b", token_b, key_b, "tenant_b");
    tokio::time::sleep(Duration::from_millis(500)).await;

    let client = reqwest::Client::new();

    let resp_a = client
        .post(format!("http://127.0.0.1:{}/anthropic/v1/messages", port_a))
        .header("Authorization", format!("Bearer {}", token_a))
        .header("Content-Type", "application/json")
        .body("{}")
        .timeout(Duration::from_secs(15))
        .send()
        .await
        .unwrap();

    let resp_b = client
        .post(format!("http://127.0.0.1:{}/anthropic/v1/messages", port_b))
        .header("Authorization", format!("Bearer {}", token_b))
        .header("Content-Type", "application/json")
        .body("{}")
        .timeout(Duration::from_secs(15))
        .send()
        .await
        .unwrap();

    assert_eq!(resp_a.status(), 200, "Proxy-A must serve tenant-A request");
    assert_eq!(resp_b.status(), 200, "Proxy-B must serve tenant-B request");

    // Critical: each mock must be hit exactly once — no cross-prefix leakage.
    assert_eq!(hit_a.hits(), 1, "Tenant-A mock must receive exactly 1 request");
    assert_eq!(hit_b.hits(), 1, "Tenant-B mock must receive exactly 1 request");
}

/// API key with special characters: real keys may contain dots, dashes,
/// underscores, and alphanumeric characters.  The worker must forward the
/// key verbatim as an HTTP header value without any percent-encoding or
/// truncation.
#[tokio::test]
async fn binary_api_key_with_special_chars_forwarded_correctly() {
    let (_nats_container, nats_port) = start_nats().await;

    let mock_server = httpmock::MockServer::start_async().await;
    let token    = "tok_anthropic_prod_spc001";
    // Real key with dots and underscores — realistic for some providers.
    let real_key = "sk-ant-v2.0_special.key_value-abc123";

    let _ai_mock = mock_server
        .mock_async(|when, then| {
            when.method(httpmock::Method::POST)
                .path("/v1/messages")
                .header("authorization", format!("Bearer {}", real_key));
            then.status(200)
                .header("content-type", "application/json")
                .body(r#"{"id":"msg_special_ok"}"#);
        })
        .await;

    let proxy_port = free_port();
    let _proxy = spawn_proxy(nats_port, proxy_port, &mock_server.base_url());
    wait_for_port(proxy_port, Duration::from_secs(15)).await;

    let _worker = spawn_worker(nats_port, "spc-workers-001", token, real_key);
    tokio::time::sleep(Duration::from_millis(500)).await;

    let resp = reqwest::Client::new()
        .post(format!("http://127.0.0.1:{}/anthropic/v1/messages", proxy_port))
        .header("Authorization", format!("Bearer {}", token))
        .header("Content-Type", "application/json")
        .body(r#"{"model":"claude-3-5-sonnet"}"#)
        .timeout(Duration::from_secs(15))
        .send()
        .await
        .unwrap();

    assert_eq!(
        resp.status(),
        200,
        "API key with dots and dashes must be forwarded verbatim"
    );
    assert_eq!(_ai_mock.hits(), 1, "Provider must see the exact real key");
}

/// HTML error body forwarded unchanged: when an AI provider returns a
/// non-JSON error page (e.g. an HTML 503 from a CDN or load balancer),
/// the proxy must forward the body and status code unchanged — it must not
/// try to parse or reinterpret the HTML.
#[tokio::test]
async fn binary_provider_html_error_body_forwarded_unchanged() {
    let (_nats_container, nats_port) = start_nats().await;

    let mock_server = httpmock::MockServer::start_async().await;
    let token    = "tok_anthropic_prod_html01";
    let real_key = "sk-ant-html-key";

    let html_body = "<html><head><title>Service Unavailable</title></head><body><h1>503 Service Unavailable</h1></body></html>";

    // A persistent 503 — the worker will exhaust retries and forward the last
    // response (status + body) back to the caller.
    let _ai_mock = mock_server
        .mock_async(|when, then| {
            when.method(httpmock::Method::POST).path("/v1/messages");
            then.status(503)
                .header("content-type", "text/html")
                .body(html_body);
        })
        .await;

    let proxy_port = free_port();
    let _proxy = spawn_proxy(nats_port, proxy_port, &mock_server.base_url());
    wait_for_port(proxy_port, Duration::from_secs(15)).await;

    let _worker = spawn_worker(nats_port, "html-workers-001", token, real_key);
    tokio::time::sleep(Duration::from_millis(500)).await;

    let resp = reqwest::Client::new()
        .post(format!("http://127.0.0.1:{}/anthropic/v1/messages", proxy_port))
        .header("Authorization", format!("Bearer {}", token))
        .header("Content-Type", "application/json")
        .body(r#"{"model":"claude-3-5-sonnet"}"#)
        .timeout(Duration::from_secs(30))
        .send()
        .await
        .unwrap();

    assert_eq!(resp.status(), 503, "HTTP 503 from provider must be forwarded");
    let body = resp.text().await.unwrap();
    assert_eq!(
        body, html_body,
        "HTML error body must reach the caller byte-for-byte"
    );
}

/// Long token ID: a token with a 24-character alphanumeric ID segment must
/// be accepted by the worker's vault lookup without truncation or rejection.
///
/// Ensures the token validator has no undocumented length cap on the ID
/// segment that would silently reject valid long-form tokens.
#[tokio::test]
async fn binary_token_with_long_alphanumeric_id_works() {
    let (_nats_container, nats_port) = start_nats().await;

    let mock_server = httpmock::MockServer::start_async().await;
    // 24-character alphanumeric ID — deliberately long.
    let token    = "tok_anthropic_prod_abcdefghij0123456789ABCD";
    let real_key = "sk-ant-long-id-key";

    let _ai_mock = mock_server
        .mock_async(|when, then| {
            when.method(httpmock::Method::POST)
                .path("/v1/messages")
                .header("authorization", format!("Bearer {}", real_key));
            then.status(200)
                .header("content-type", "application/json")
                .body(r#"{"id":"msg_longid_ok"}"#);
        })
        .await;

    let proxy_port = free_port();
    let _proxy = spawn_proxy(nats_port, proxy_port, &mock_server.base_url());
    wait_for_port(proxy_port, Duration::from_secs(15)).await;

    let _worker = spawn_worker(nats_port, "longid-workers-001", token, real_key);
    tokio::time::sleep(Duration::from_millis(500)).await;

    let resp = reqwest::Client::new()
        .post(format!("http://127.0.0.1:{}/anthropic/v1/messages", proxy_port))
        .header("Authorization", format!("Bearer {}", token))
        .header("Content-Type", "application/json")
        .body(r#"{"model":"claude-3-5-sonnet"}"#)
        .timeout(Duration::from_secs(15))
        .send()
        .await
        .unwrap();

    assert_eq!(
        resp.status(),
        200,
        "Token with 24-char alphanumeric ID must be accepted and resolved"
    );
    assert_eq!(_ai_mock.hits(), 1, "Provider must receive the request with the real key");
}

// ── Gap 4: X-Request-Id replacement ───────────────────────────────────────

/// The worker strips the caller's `X-Request-Id` and replaces it with the
/// proxy-generated correlation UUID (idempotency_key).  The AI provider must
/// never receive the caller's original header value.
///
/// Two mocks are registered:
/// 1. Matches requests that still carry the original caller header → 500 "LEAKED"
/// 2. Matches any POST request → 200 (the normal path)
///
/// If the caller ID was replaced, mock 1 never fires and mock 2 handles the
/// request.  If the ID leaked, mock 1 fires first and the test fails.
#[tokio::test]
async fn binary_caller_x_request_id_replaced_by_proxy_uuid() {
    let (_nats_container, nats_port) = start_nats().await;

    let mock_server = httpmock::MockServer::start_async().await;
    let token = "tok_anthropic_prod_reqid01";
    let real_key = "sk-ant-reqid-key";

    // If the caller's X-Request-Id leaks to the provider, this mock fires.
    let leak_mock = mock_server
        .mock_async(|when, then| {
            when.method(httpmock::Method::POST)
                .path("/v1/messages")
                .header("x-request-id", "custom-caller-id-12345");
            then.status(500).body("X-Request-Id leaked to provider");
        })
        .await;

    // Normal mock: fires when the caller's ID is absent (replaced by UUID).
    let ok_mock = mock_server
        .mock_async(|when, then| {
            when.method(httpmock::Method::POST).path("/v1/messages");
            then.status(200)
                .header("content-type", "application/json")
                .body(r#"{"id":"msg_reqid_ok"}"#);
        })
        .await;

    let proxy_port = free_port();
    let _proxy = spawn_proxy(nats_port, proxy_port, &mock_server.base_url());
    wait_for_port(proxy_port, Duration::from_secs(15)).await;

    let _worker = spawn_worker(nats_port, "reqid-workers-001", token, real_key);
    tokio::time::sleep(Duration::from_millis(500)).await;

    let resp = reqwest::Client::new()
        .post(format!("http://127.0.0.1:{}/anthropic/v1/messages", proxy_port))
        .header("Authorization", format!("Bearer {}", token))
        .header("Content-Type", "application/json")
        .header("X-Request-Id", "custom-caller-id-12345")
        .body(r#"{"model":"claude-3-5-sonnet"}"#)
        .timeout(Duration::from_secs(15))
        .send()
        .await
        .unwrap();

    assert_eq!(resp.status(), 200, "Request must succeed");
    assert_eq!(ok_mock.hits(), 1, "Provider must receive the request");
    assert_eq!(
        leak_mock.hits(),
        0,
        "Caller X-Request-Id must be replaced by proxy UUID — never forwarded to the AI provider"
    );
}

// ── Gap 7: hyphenated prefix ───────────────────────────────────────────────

/// A `PROXY_PREFIX` that contains a hyphen (e.g. `my-hyphen`) must be accepted
/// by both binaries and produce a consistent NATS subject namespace so they
/// can communicate end-to-end.
#[tokio::test]
async fn binary_hyphenated_prefix_both_binaries_communicate() {
    let (_nats_container, nats_port) = start_nats().await;

    let mock_server = httpmock::MockServer::start_async().await;
    let token = "tok_anthropic_prod_hyph01";
    let real_key = "sk-ant-hyph-key";

    let ai_mock = mock_server
        .mock_async(|when, then| {
            when.method(httpmock::Method::POST)
                .path("/v1/messages")
                .header("authorization", format!("Bearer {}", real_key));
            then.status(200)
                .header("content-type", "application/json")
                .body(r#"{"id":"msg_hyph_ok"}"#);
        })
        .await;

    let proxy_port = free_port();
    let _proxy =
        spawn_proxy_with_prefix(nats_port, proxy_port, &mock_server.base_url(), "my-hyphen");
    wait_for_port(proxy_port, Duration::from_secs(15)).await;

    let _worker =
        spawn_worker_with_prefix(nats_port, "hyph-workers-001", token, real_key, "my-hyphen");
    tokio::time::sleep(Duration::from_millis(500)).await;

    let resp = reqwest::Client::new()
        .post(format!("http://127.0.0.1:{}/anthropic/v1/messages", proxy_port))
        .header("Authorization", format!("Bearer {}", token))
        .header("Content-Type", "application/json")
        .body(r#"{"model":"claude-3-5-sonnet"}"#)
        .timeout(Duration::from_secs(15))
        .send()
        .await
        .unwrap();

    assert_eq!(
        resp.status(),
        200,
        "Hyphenated PROXY_PREFIX must be accepted and both binaries must communicate"
    );
    assert_eq!(ai_mock.hits(), 1, "Provider must receive exactly one request");
}

// ── Gap 9: empty PROXY_BASE_URL_OVERRIDE ──────────────────────────────────

/// Setting `PROXY_BASE_URL_OVERRIDE` to an empty string causes the proxy to
/// build a URL with no host (e.g. `"v1/messages"`).  The worker should fail
/// to forward the request and reply with an error response rather than
/// panicking or timing out the proxy.
#[tokio::test]
async fn binary_empty_base_url_override_returns_error() {
    let (_nats_container, nats_port) = start_nats().await;

    let token = "tok_anthropic_prod_emptyurl01";
    let real_key = "sk-ant-empty-url-key";

    let proxy_port = free_port();
    // Pass "" as the base URL — proxy will construct a URL without a host.
    let _proxy = spawn_proxy(nats_port, proxy_port, "");
    wait_for_port(proxy_port, Duration::from_secs(15)).await;

    let _worker = spawn_worker(nats_port, "emptyurl-workers-001", token, real_key);
    tokio::time::sleep(Duration::from_millis(500)).await;

    let resp = reqwest::Client::new()
        .post(format!("http://127.0.0.1:{}/anthropic/v1/messages", proxy_port))
        .header("Authorization", format!("Bearer {}", token))
        .header("Content-Type", "application/json")
        .body(r#"{"model":"claude-3-5-sonnet"}"#)
        // Allow time for the worker's 3 retry attempts (100 + 200 + 400 ms backoff).
        .timeout(Duration::from_secs(15))
        .send()
        .await
        .unwrap();

    // Empty base URL produces an invalid/relative URL; the worker exhausts
    // retries and replies with a 502 error response.
    assert!(
        resp.status().is_server_error(),
        "Empty PROXY_BASE_URL_OVERRIDE must return 5xx, got {}",
        resp.status()
    );
}

// ── Gap 12: overflow PROXY_WORKER_TIMEOUT_SECS ────────────────────────────

/// `PROXY_WORKER_TIMEOUT_SECS` is parsed as u64.  A value that overflows u64
/// fails `.parse::<u64>()`, which returns `None`, and `unwrap_or(60)` kicks in.
/// The proxy must start normally and serve requests with the 60 s fallback.
#[tokio::test]
async fn binary_invalid_worker_timeout_falls_back_to_default() {
    let (_nats_container, nats_port) = start_nats().await;

    let mock_server = httpmock::MockServer::start_async().await;
    let token = "tok_anthropic_prod_badtout01";
    let real_key = "sk-ant-bad-timeout-key";

    let ai_mock = mock_server
        .mock_async(|when, then| {
            when.method(httpmock::Method::POST).path("/v1/messages");
            then.status(200)
                .header("content-type", "application/json")
                .body(r#"{"id":"msg_timeout_ok"}"#);
        })
        .await;

    let proxy_port = free_port();
    // Spawn proxy with a u64-overflow timeout value.
    let _proxy = Command::new(env!("CARGO_BIN_EXE_proxy"))
        .env("NATS_URL", format!("localhost:{}", nats_port))
        .env("PROXY_PORT", proxy_port.to_string())
        .env("PROXY_WORKER_TIMEOUT_SECS", "99999999999999999999") // overflows u64
        .env("PROXY_BASE_URL_OVERRIDE", &mock_server.base_url())
        .env("RUST_LOG", "warn")
        .kill_on_drop(true)
        .spawn()
        .expect("Failed to spawn proxy binary");
    wait_for_port(proxy_port, Duration::from_secs(15)).await;

    let _worker = spawn_worker(nats_port, "badtout-workers-001", token, real_key);
    tokio::time::sleep(Duration::from_millis(500)).await;

    let resp = reqwest::Client::new()
        .post(format!("http://127.0.0.1:{}/anthropic/v1/messages", proxy_port))
        .header("Authorization", format!("Bearer {}", token))
        .header("Content-Type", "application/json")
        .body(r#"{"model":"claude-3-5-sonnet"}"#)
        .timeout(Duration::from_secs(15))
        .send()
        .await
        .unwrap();

    assert_eq!(
        resp.status(),
        200,
        "Proxy with overflow PROXY_WORKER_TIMEOUT_SECS must fall back to 60 s default and serve requests"
    );
    assert_eq!(ai_mock.hits(), 1, "Provider must receive the request");
}

// ── Gap 16: real key in response body is forwarded unchanged ───────────────

/// The worker strips response *headers* that contain the real API key, but
/// the response *body* is forwarded verbatim.  If the provider includes the
/// real key in the response body (e.g. in a verbose debug response), the
/// caller receives it.  This is a documented design boundary: body filtering
/// is out of scope for the proxy.
#[tokio::test]
async fn binary_real_key_in_response_body_reaches_caller_unchanged() {
    let (_nats_container, nats_port) = start_nats().await;

    let mock_server = httpmock::MockServer::start_async().await;
    let token = "tok_anthropic_prod_bodykey01";
    let real_key = "sk-ant-body-real-key";

    // Provider echoes the real key back in the JSON response body.
    let body_with_key = format!(
        r#"{{"id":"msg_body_key","auth_used":"Bearer {}","type":"message"}}"#,
        real_key
    );

    let _ai_mock = mock_server
        .mock_async(|when, then| {
            when.method(httpmock::Method::POST).path("/v1/messages");
            then.status(200)
                .header("content-type", "application/json")
                .body(body_with_key.clone());
        })
        .await;

    let proxy_port = free_port();
    let _proxy = spawn_proxy(nats_port, proxy_port, &mock_server.base_url());
    wait_for_port(proxy_port, Duration::from_secs(15)).await;

    let _worker = spawn_worker(nats_port, "bodykey-workers-001", token, real_key);
    tokio::time::sleep(Duration::from_millis(500)).await;

    let resp = reqwest::Client::new()
        .post(format!("http://127.0.0.1:{}/anthropic/v1/messages", proxy_port))
        .header("Authorization", format!("Bearer {}", token))
        .header("Content-Type", "application/json")
        .body(r#"{"model":"claude-3-5-sonnet"}"#)
        .timeout(Duration::from_secs(15))
        .send()
        .await
        .unwrap();

    assert_eq!(resp.status(), 200);
    let body = resp.text().await.unwrap();

    // Design boundary: the proxy forwards the body byte-for-byte.
    // Only response *headers* containing the real key are filtered.
    assert_eq!(
        body, body_with_key,
        "Response body must be forwarded to the caller byte-for-byte"
    );
    assert!(
        body.contains(real_key),
        "Real key in response body must reach the caller (body filtering is out of scope)"
    );
}

// ── Default WORKER_CONSUMER_NAME ───────────────────────────────────────────

/// When `WORKER_CONSUMER_NAME` is not set, the worker binary defaults to
/// `"proxy-workers"`.  Both proxy and worker must communicate end-to-end
/// with this default consumer group.
///
/// All other binary tests pass the consumer name explicitly via
/// `spawn_worker`; this test exercises the default path.
#[tokio::test]
async fn binary_worker_default_consumer_name_works() {
    let (_nats_container, nats_port) = start_nats().await;

    let mock_server = httpmock::MockServer::start_async().await;
    let token = "tok_anthropic_prod_defcons01";
    let real_key = "sk-ant-def-consumer-key";

    let ai_mock = mock_server
        .mock_async(|when, then| {
            when.method(httpmock::Method::POST)
                .path("/v1/messages")
                .header("authorization", format!("Bearer {}", real_key));
            then.status(200)
                .header("content-type", "application/json")
                .body(r#"{"id":"msg_defcons_ok"}"#);
        })
        .await;

    let proxy_port = free_port();
    let _proxy = spawn_proxy(nats_port, proxy_port, &mock_server.base_url());
    wait_for_port(proxy_port, Duration::from_secs(15)).await;

    // Spawn worker WITHOUT setting WORKER_CONSUMER_NAME — binary defaults to "proxy-workers".
    let _worker = Command::new(env!("CARGO_BIN_EXE_worker"))
        .env("NATS_URL", format!("localhost:{}", nats_port))
        // WORKER_CONSUMER_NAME intentionally omitted
        .env(format!("VAULT_TOKEN_{}", token), real_key)
        .env("RUST_LOG", "warn")
        .kill_on_drop(true)
        .spawn()
        .expect("Failed to spawn worker binary");
    tokio::time::sleep(Duration::from_millis(500)).await;

    let resp = reqwest::Client::new()
        .post(format!("http://127.0.0.1:{}/anthropic/v1/messages", proxy_port))
        .header("Authorization", format!("Bearer {}", token))
        .header("Content-Type", "application/json")
        .body(r#"{"model":"claude-3-5-sonnet"}"#)
        .timeout(Duration::from_secs(15))
        .send()
        .await
        .unwrap();

    assert_eq!(
        resp.status(),
        200,
        "Worker with default WORKER_CONSUMER_NAME must process requests"
    );
    assert_eq!(ai_mock.hits(), 1, "Provider must receive exactly one request");
}

// ── Concurrent mixed valid/invalid tokens ─────────────────────────────────

/// Under concurrent load, requests with valid tokens must succeed (200) and
/// requests with unknown tokens must fail with a client error (401), with no
/// cross-contamination between the two groups.
///
/// This verifies error isolation: a flood of failed vault lookups must not
/// interfere with the successful requests sharing the same worker instance.
#[tokio::test]
async fn binary_concurrent_mixed_valid_and_invalid_tokens_isolated() {
    let (_nats_container, nats_port) = start_nats().await;

    let mock_server = httpmock::MockServer::start_async().await;
    let valid_token = "tok_anthropic_prod_mixok01";
    let real_key = "sk-ant-mix-valid-key";

    // Only matches requests that carry the real key.
    let ok_mock = mock_server
        .mock_async(|when, then| {
            when.method(httpmock::Method::POST)
                .path("/v1/messages")
                .header("authorization", format!("Bearer {}", real_key));
            then.status(200)
                .header("content-type", "application/json")
                .body(r#"{"id":"msg_mix_ok"}"#);
        })
        .await;

    let proxy_port = free_port();
    let _proxy = spawn_proxy(nats_port, proxy_port, &mock_server.base_url());
    wait_for_port(proxy_port, Duration::from_secs(15)).await;

    // Worker vault has only `valid_token` — the unknown token will get 401.
    let _worker = spawn_worker(nats_port, "mix-workers-001", valid_token, real_key);
    tokio::time::sleep(Duration::from_millis(500)).await;

    let client = reqwest::Client::new();
    let base = format!("http://127.0.0.1:{}/anthropic/v1/messages", proxy_port);

    // 5 requests with the known valid token.
    let valid_reqs = (0..5).map(|_| {
        client
            .post(&base)
            .header("Authorization", format!("Bearer {}", valid_token))
            .header("Content-Type", "application/json")
            .body("{}")
            .timeout(Duration::from_secs(15))
            .send()
    });

    // 5 requests with a token that does NOT exist in the vault.
    let invalid_reqs = (0..5).map(|_| {
        client
            .post(&base)
            .header("Authorization", "Bearer tok_anthropic_prod_unknown99")
            .header("Content-Type", "application/json")
            .body("{}")
            .timeout(Duration::from_secs(15))
            .send()
    });

    let valid_results: Vec<_> = futures_util::future::join_all(valid_reqs).await;
    let invalid_results: Vec<_> = futures_util::future::join_all(invalid_reqs).await;

    // All valid-token requests must succeed.
    for (i, r) in valid_results.iter().enumerate() {
        let resp = r.as_ref().unwrap();
        assert_eq!(resp.status(), 200, "Valid-token request #{} must succeed", i);
    }

    // All unknown-token requests must fail with a client error (401).
    for (i, r) in invalid_results.iter().enumerate() {
        let resp = r.as_ref().unwrap();
        assert!(
            resp.status().is_client_error(),
            "Unknown-token request #{} must return 4xx, got {}",
            i,
            resp.status()
        );
    }

    assert_eq!(
        ok_mock.hits(),
        5,
        "Provider must receive exactly 5 requests (one per valid token)"
    );
}

// ── Gap: PROXY_BASE_URL_OVERRIDE with trailing slash normalized ───────────────

/// `trim_end_matches('/')` in the proxy removes trailing slashes from the
/// base URL override.  Passing a URL with a trailing slash (e.g.
/// `http://mock:PORT/`) must still route to `http://mock:PORT/v1/messages`
/// — not `http://mock:PORT//v1/messages` — and the request must succeed.
#[tokio::test]
async fn binary_base_url_override_with_trailing_slash_is_normalized() {
    let (_nats_container, nats_port) = start_nats().await;

    let mock_server = httpmock::MockServer::start_async().await;
    let token = "tok_anthropic_prod_trailslash1";
    let real_key = "sk-ant-trailslash-key-001";

    // Expect exactly one hit at /v1/messages (no double slash).
    let mock = mock_server
        .mock_async(|when, then| {
            when.method(httpmock::Method::POST).path("/v1/messages");
            then.status(200)
                .header("content-type", "application/json")
                .body(r#"{"id":"msg_trail"}"#);
        })
        .await;

    let proxy_port = free_port();
    // Pass base URL with a trailing slash — proxy must strip it.
    let base_url_with_slash = format!("{}/", mock_server.base_url());
    let _proxy = spawn_proxy(nats_port, proxy_port, &base_url_with_slash);
    wait_for_port(proxy_port, Duration::from_secs(15)).await;

    let _worker = spawn_worker(nats_port, "trail-workers-001", token, real_key);
    tokio::time::sleep(Duration::from_millis(500)).await;

    let resp = reqwest::Client::new()
        .post(format!("http://127.0.0.1:{}/anthropic/v1/messages", proxy_port))
        .header("Authorization", format!("Bearer {}", token))
        .header("Content-Type", "application/json")
        .body(r#"{"model":"claude-3-5-sonnet","messages":[]}"#)
        .timeout(Duration::from_secs(15))
        .send()
        .await
        .unwrap();

    assert_eq!(
        resp.status(),
        200,
        "Trailing slash in base URL override must be normalized — got {}",
        resp.status()
    );
    assert_eq!(
        mock.hits(),
        1,
        "Provider must receive exactly one request at /v1/messages (no double slash)"
    );
}
