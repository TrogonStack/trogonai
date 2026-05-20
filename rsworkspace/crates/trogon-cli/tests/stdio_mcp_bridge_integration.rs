//! Integration tests for `StdioMcpBridge`.
//!
//! Each test spawns a real shell script that mimics a minimal MCP stdio server
//! and exercises the bridge's HTTP proxy through actual HTTP requests.
//!
//! Run with:
//!   cargo test -p trogon-cli --test stdio_mcp_bridge_integration

use std::sync::atomic::{AtomicU64, Ordering};

use serde_json::Value;
use trogon_cli::StdioMcpBridge;

// ── helpers ───────────────────────────────────────────────────────────────────

static SEQ: AtomicU64 = AtomicU64::new(0);

/// Write and chmod a shell script to a unique temp path.
async fn write_script(name: &str, body: &[u8]) -> std::path::PathBuf {
    use std::os::unix::fs::PermissionsExt as _;
    let n = SEQ.fetch_add(1, Ordering::Relaxed);
    let pid = std::process::id();
    let path = std::env::temp_dir().join(format!("trogon_mcp_integ_{name}_{pid}_{n}.sh"));
    tokio::fs::write(&path, body).await.unwrap();
    let mut perms = tokio::fs::metadata(&path).await.unwrap().permissions();
    perms.set_mode(0o755);
    tokio::fs::set_permissions(&path, perms).await.unwrap();
    path
}

/// Standard echo server: for each JSON-RPC request read from stdin, reply with
/// the same `id` and an empty `result`.
async fn echo_script() -> std::path::PathBuf {
    write_script(
        "echo",
        b"#!/bin/sh\n\
          while IFS= read -r line; do \
            id=$(echo \"$line\" | sed 's/.*\"id\":[[:space:]]*\\([0-9]*\\).*/\\1/'); \
            printf '{\"jsonrpc\":\"2.0\",\"id\":%s,\"result\":{}}\n' \"$id\"; \
          done\n",
    )
    .await
}

/// Echo server that prepends one non-JSON garbage line before every response.
async fn noisy_echo_script() -> std::path::PathBuf {
    write_script(
        "noisy",
        b"#!/bin/sh\n\
          while IFS= read -r line; do \
            printf 'this-is-not-json\n'; \
            id=$(echo \"$line\" | sed 's/.*\"id\":[[:space:]]*\\([0-9]*\\).*/\\1/'); \
            printf '{\"jsonrpc\":\"2.0\",\"id\":%s,\"result\":{}}\n' \"$id\"; \
          done\n",
    )
    .await
}

/// Echo server that prepends a valid JSON-RPC notification (no `id`) before each response.
async fn notification_echo_script() -> std::path::PathBuf {
    write_script(
        "notif",
        b"#!/bin/sh\n\
          while IFS= read -r line; do \
            printf '{\"jsonrpc\":\"2.0\",\"method\":\"notifications/test\"}\n'; \
            id=$(echo \"$line\" | sed 's/.*\"id\":[[:space:]]*\\([0-9]*\\).*/\\1/'); \
            printf '{\"jsonrpc\":\"2.0\",\"id\":%s,\"result\":{}}\n' \"$id\"; \
          done\n",
    )
    .await
}

// ── tests ─────────────────────────────────────────────────────────────────────

/// `spawn` returns a URL on the local loopback interface.
#[tokio::test]
async fn bridge_spawns_and_returns_local_url() {
    let script = echo_script().await;
    let bridge = StdioMcpBridge::spawn(script.to_str().unwrap(), &[], &[])
        .await
        .expect("spawn must succeed");
    assert!(
        bridge.url.starts_with("http://127.0.0.1:"),
        "url must be a local http address; got: {}",
        bridge.url
    );
    bridge.shutdown().await;
}

/// A well-formed JSON-RPC request is forwarded to the child and the response
/// is returned to the HTTP caller with the correct `id`.
#[tokio::test]
async fn bridge_proxies_json_rpc_request_to_child_and_returns_response() {
    let script = echo_script().await;
    let bridge = StdioMcpBridge::spawn(script.to_str().unwrap(), &[], &[])
        .await
        .expect("spawn must succeed");

    let resp: Value = reqwest::Client::new()
        .post(&bridge.url)
        .json(&serde_json::json!({
            "jsonrpc": "2.0",
            "id": 42,
            "method": "tools/list",
            "params": {}
        }))
        .send()
        .await
        .expect("HTTP request must succeed")
        .json()
        .await
        .expect("response must be valid JSON");

    assert_eq!(resp["id"], 42, "response id must match request id");
    assert!(resp.get("result").is_some(), "response must contain a result field");
    bridge.shutdown().await;
}

/// `spawn` returns `Err` when the command path does not exist.
#[tokio::test]
async fn bridge_spawn_fails_for_nonexistent_command() {
    let result =
        StdioMcpBridge::spawn("/nonexistent/binary/that/does/not/exist", &[], &[]).await;
    assert!(result.is_err(), "spawn must fail for a nonexistent command");
}

/// `shutdown` completes within a reasonable timeout, confirming the child
/// process is killed cleanly.
#[tokio::test]
async fn bridge_shutdown_kills_child_and_completes() {
    let script = echo_script().await;
    let bridge = StdioMcpBridge::spawn(script.to_str().unwrap(), &[], &[])
        .await
        .expect("spawn must succeed");
    tokio::time::timeout(std::time::Duration::from_secs(5), bridge.shutdown())
        .await
        .expect("shutdown must complete within 5 s");
}

/// Non-JSON lines emitted by the child on stdout are silently discarded;
/// the correct JSON-RPC response is still matched and returned.
#[tokio::test]
async fn bridge_skips_non_json_lines_from_child() {
    let script = noisy_echo_script().await;
    let bridge = StdioMcpBridge::spawn(script.to_str().unwrap(), &[], &[])
        .await
        .expect("spawn must succeed");

    let resp: Value = reqwest::Client::new()
        .post(&bridge.url)
        .json(&serde_json::json!({ "jsonrpc": "2.0", "id": 7, "method": "tools/list", "params": {} }))
        .send()
        .await
        .expect("request must succeed")
        .json()
        .await
        .expect("response must be JSON");

    assert_eq!(resp["id"], 7, "garbage lines must be skipped; id must match");
    bridge.shutdown().await;
}

/// JSON-RPC notification lines from the child (no `id` field) are discarded
/// without blocking the pending request.
#[tokio::test]
async fn bridge_discards_notification_lines_from_child() {
    let script = notification_echo_script().await;
    let bridge = StdioMcpBridge::spawn(script.to_str().unwrap(), &[], &[])
        .await
        .expect("spawn must succeed");

    let resp: Value = reqwest::Client::new()
        .post(&bridge.url)
        .json(&serde_json::json!({ "jsonrpc": "2.0", "id": 8, "method": "tools/list", "params": {} }))
        .send()
        .await
        .expect("request must succeed")
        .json()
        .await
        .expect("response must be JSON");

    assert_eq!(resp["id"], 8, "notification lines must be discarded; id must match");
    bridge.shutdown().await;
}

/// Two concurrent requests are routed to the correct responses despite being
/// serialised through a single stdin/stdout pipe.
#[tokio::test]
async fn bridge_routes_concurrent_requests_to_correct_responses() {
    let script = echo_script().await;
    let bridge = StdioMcpBridge::spawn(script.to_str().unwrap(), &[], &[])
        .await
        .expect("spawn must succeed");

    let url = bridge.url.clone();
    let client = reqwest::Client::new();
    let (r1, r2) = tokio::join!(
        {
            let client = client.clone();
            let url = url.clone();
            async move {
                client
                    .post(&url)
                    .json(&serde_json::json!({ "jsonrpc": "2.0", "id": 100, "method": "m" }))
                    .send()
                    .await
                    .unwrap()
                    .json::<Value>()
                    .await
                    .unwrap()
            }
        },
        async move {
            client
                .post(&url)
                .json(&serde_json::json!({ "jsonrpc": "2.0", "id": 200, "method": "m" }))
                .send()
                .await
                .unwrap()
                .json::<Value>()
                .await
                .unwrap()
        }
    );
    assert_eq!(r1["id"], 100, "first response must have id 100");
    assert_eq!(r2["id"], 200, "second response must have id 200");
    bridge.shutdown().await;
}

/// A request body that is not valid JSON returns HTTP 400.
#[tokio::test]
async fn bridge_rejects_invalid_json_body_with_400() {
    let script = echo_script().await;
    let bridge = StdioMcpBridge::spawn(script.to_str().unwrap(), &[], &[])
        .await
        .expect("spawn must succeed");

    let status = reqwest::Client::new()
        .post(&bridge.url)
        .body("{not valid json")
        .header("content-type", "application/json")
        .send()
        .await
        .expect("HTTP request must succeed")
        .status();

    assert_eq!(status.as_u16(), 400, "invalid JSON must return 400");
    bridge.shutdown().await;
}

/// A notification (no `id`) sent by the HTTP client gets a 200 response
/// with no body (fire-and-forget semantics).
#[tokio::test]
async fn bridge_returns_200_for_client_notification_without_id() {
    let script = echo_script().await;
    let bridge = StdioMcpBridge::spawn(script.to_str().unwrap(), &[], &[])
        .await
        .expect("spawn must succeed");

    let resp = reqwest::Client::new()
        .post(&bridge.url)
        .json(&serde_json::json!({
            "jsonrpc": "2.0",
            "method": "notifications/initialized"
        }))
        .send()
        .await
        .expect("HTTP request must succeed");

    assert_eq!(resp.status().as_u16(), 200, "notification must return 200");
    bridge.shutdown().await;
}
