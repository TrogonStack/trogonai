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
    Command::new(env!("CARGO_BIN_EXE_proxy"))
        .env("NATS_URL", format!("localhost:{}", nats_port))
        .env("PROXY_PORT", proxy_port.to_string())
        .env("PROXY_WORKER_TIMEOUT_SECS", "15")
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
