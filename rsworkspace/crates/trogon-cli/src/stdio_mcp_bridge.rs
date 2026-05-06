//! stdio ↔ HTTP proxy for MCP servers.
//!
//! Spawns an MCP stdio process as a child, starts an axum HTTP server on a
//! random local port, and proxies JSON-RPC POST requests to the child's
//! stdin/stdout.  The caller registers the resulting `url` as a plain HTTP MCP
//! server — the NATS backend never needs to know about stdio.
//!
//! # Lifecycle
//! * Call [`StdioMcpBridge::spawn`] to create the bridge and get the URL.
//! * Call [`StdioMcpBridge::shutdown`] for a clean teardown (kills the child,
//!   stops the HTTP server).
//! * Dropping the bridge without calling `shutdown` aborts the HTTP server task;
//!   the child process is detached and will be reaped by the OS on process exit.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use axum::Router;
use axum::body::Bytes;
use axum::extract::State;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use serde_json::Value;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::{Child, ChildStdin, Command};
use tokio::sync::{Mutex, oneshot};
use tracing::warn;

// ── Error ─────────────────────────────────────────────────────────────────────

/// Errors that can occur when creating a [`StdioMcpBridge`].
#[derive(Debug)]
pub enum BridgeError {
    Spawn(std::io::Error),
    Bind(std::io::Error),
}

impl std::fmt::Display for BridgeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Spawn(e) => write!(f, "failed to spawn process: {e}"),
            Self::Bind(e) => write!(f, "failed to bind HTTP server: {e}"),
        }
    }
}

impl std::error::Error for BridgeError {}

// ── Shared bridge state ───────────────────────────────────────────────────────

struct Inner {
    /// Pending request channels, keyed by JSON-RPC id.
    pending: Mutex<HashMap<u64, oneshot::Sender<Value>>>,
    /// Write end of the child's stdin.
    stdin: Mutex<ChildStdin>,
}

// ── Bridge ────────────────────────────────────────────────────────────────────

/// A running stdio ↔ HTTP MCP bridge.
pub struct StdioMcpBridge {
    /// Local HTTP URL for the proxy, e.g. `http://127.0.0.1:54321`.
    pub url: String,
    #[allow(dead_code)]
    inner: Arc<Inner>,
    child: Child,
    server_abort: tokio::task::AbortHandle,
}

impl StdioMcpBridge {
    /// Spawn `command` with `args` and `env`, and start the HTTP proxy on a
    /// random local port.
    pub async fn spawn(
        command: &str,
        args: &[String],
        env: &[(String, String)],
    ) -> Result<Self, BridgeError> {
        let mut child = Command::new(command)
            .args(args)
            .envs(env.iter().map(|(k, v)| (k.as_str(), v.as_str())))
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::inherit())
            .spawn()
            .map_err(BridgeError::Spawn)?;

        let stdin = child.stdin.take().expect("stdin must be piped");
        let stdout = child.stdout.take().expect("stdout must be piped");

        let inner = Arc::new(Inner {
            pending: Mutex::new(HashMap::new()),
            stdin: Mutex::new(stdin),
        });

        // Background reader: route each newline-delimited JSON response from the
        // child to the pending HTTP handler that is waiting for it.
        let reader_inner = inner.clone();
        tokio::spawn(async move {
            let mut lines = BufReader::new(stdout).lines();
            loop {
                match lines.next_line().await {
                    Ok(Some(line)) => {
                        if line.trim().is_empty() {
                            continue;
                        }
                        let Ok(msg): Result<Value, _> = serde_json::from_str(&line) else {
                            continue;
                        };
                        let Some(id) = msg.get("id").and_then(Value::as_u64) else {
                            // Notification — no pending channel to unblock.
                            continue;
                        };
                        if let Some(tx) = reader_inner.pending.lock().await.remove(&id) {
                            let _ = tx.send(msg);
                        }
                    }
                    _ => break,
                }
            }
        });

        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .map_err(BridgeError::Bind)?;
        let port = listener.local_addr().unwrap().port();
        let url = format!("http://127.0.0.1:{port}");

        let app = Router::new()
            .fallback(proxy_handler)
            .with_state(inner.clone());

        let handle = tokio::spawn(async move {
            axum::serve(listener, app).await.ok();
        });
        let server_abort = handle.abort_handle();

        Ok(Self { url, inner, child, server_abort })
    }

    /// Shut down the HTTP server and kill the child process.
    pub async fn shutdown(mut self) {
        self.server_abort.abort();
        let _ = self.child.kill().await;
        let _ = self.child.wait().await;
    }
}

impl Drop for StdioMcpBridge {
    fn drop(&mut self) {
        // Abort the axum task; child is detached and will be reaped on process exit.
        self.server_abort.abort();
    }
}

// ── HTTP handler ──────────────────────────────────────────────────────────────

async fn proxy_handler(State(inner): State<Arc<Inner>>, body: Bytes) -> Response {
    let Ok(req): Result<Value, _> = serde_json::from_slice(&body) else {
        return (StatusCode::BAD_REQUEST, "invalid JSON\n").into_response();
    };

    // Notifications have no id — write and return immediately.
    let Some(id) = req.get("id").and_then(Value::as_u64) else {
        let line = serde_json::to_string(&req).unwrap_or_default() + "\n";
        let mut stdin = inner.stdin.lock().await;
        let _ = stdin.write_all(line.as_bytes()).await;
        let _ = stdin.flush().await;
        return StatusCode::OK.into_response();
    };

    let (tx, rx) = oneshot::channel();
    inner.pending.lock().await.insert(id, tx);

    let line = serde_json::to_string(&req).unwrap_or_default() + "\n";
    {
        let mut stdin = inner.stdin.lock().await;
        if stdin.write_all(line.as_bytes()).await.is_err()
            || stdin.flush().await.is_err()
        {
            inner.pending.lock().await.remove(&id);
            return (StatusCode::BAD_GATEWAY, "child stdin closed\n").into_response();
        }
    }

    match tokio::time::timeout(Duration::from_secs(30), rx).await {
        Ok(Ok(resp)) => axum::Json(resp).into_response(),
        Ok(Err(_)) => (StatusCode::BAD_GATEWAY, "child closed\n").into_response(),
        Err(_) => {
            inner.pending.lock().await.remove(&id);
            warn!(id, "stdio MCP bridge: child timed out");
            (StatusCode::GATEWAY_TIMEOUT, "child timeout\n").into_response()
        }
    }
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    /// A minimal MCP server script written to a temp file: echoes each JSON-RPC
    /// request back as a response with the same id and an empty result.
    ///
    /// Each call creates a unique file to avoid ETXTBSY races between parallel tests.
    async fn echo_script() -> std::path::PathBuf {
        use std::sync::atomic::{AtomicU64, Ordering};
        static SEQ: AtomicU64 = AtomicU64::new(0);
        let n = SEQ.fetch_add(1, Ordering::Relaxed);
        let pid = std::process::id();
        let path = std::env::temp_dir().join(format!("trogon_cli_echo_mcp_{pid}_{n}.sh"));
        tokio::fs::write(
            &path,
            b"#!/bin/sh\nwhile IFS= read -r line; do \
              id=$(echo \"$line\" | sed 's/.*\"id\":\\s*\\([0-9]*\\).*/\\1/'); \
              printf '{\"jsonrpc\":\"2.0\",\"id\":%s,\"result\":{}}\n' \"$id\"; \
            done\n",
        )
        .await
        .unwrap();
        use std::os::unix::fs::PermissionsExt;
        let mut perms = tokio::fs::metadata(&path).await.unwrap().permissions();
        perms.set_mode(0o755);
        tokio::fs::set_permissions(&path, perms).await.unwrap();
        path
    }

    #[tokio::test]
    async fn bridge_spawns_and_returns_url() {
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

    #[tokio::test]
    async fn bridge_proxies_json_rpc_request() {
        let script = echo_script().await;
        let bridge = StdioMcpBridge::spawn(script.to_str().unwrap(), &[], &[])
            .await
            .expect("spawn must succeed");

        let client = reqwest::Client::new();
        let body = serde_json::json!({
            "jsonrpc": "2.0",
            "id": 42,
            "method": "tools/list",
            "params": {}
        });
        let resp: Value = client
            .post(&bridge.url)
            .json(&body)
            .send()
            .await
            .expect("HTTP request must succeed")
            .json()
            .await
            .expect("response must be valid JSON");

        assert_eq!(resp["id"], 42, "response id must match request id");
        assert!(resp.get("result").is_some(), "response must have a result field");
        bridge.shutdown().await;
    }

    #[tokio::test]
    async fn bridge_spawn_fails_for_nonexistent_command() {
        let result = StdioMcpBridge::spawn(
            "/nonexistent/binary/that/does/not/exist",
            &[],
            &[],
        )
        .await;
        assert!(result.is_err(), "spawn must fail for a nonexistent command");
    }

    /// Script that emits one garbage non-JSON line before the real response.
    async fn noisy_echo_script() -> std::path::PathBuf {
        let path = std::env::temp_dir().join("trogon_cli_noisy_echo_mcp.sh");
        tokio::fs::write(
            &path,
            b"#!/bin/sh\nwhile IFS= read -r line; do \
              printf 'this-is-not-json\\n'; \
              id=$(echo \"$line\" | sed 's/.*\"id\":\\s*\\([0-9]*\\).*/\\1/'); \
              printf '{\"jsonrpc\":\"2.0\",\"id\":%s,\"result\":{}}\n' \"$id\"; \
            done\n",
        )
        .await
        .unwrap();
        use std::os::unix::fs::PermissionsExt;
        let mut perms = tokio::fs::metadata(&path).await.unwrap().permissions();
        perms.set_mode(0o755);
        tokio::fs::set_permissions(&path, perms).await.unwrap();
        path
    }

    /// Script that emits a notification line (valid JSON, no id) before the real response.
    async fn notification_echo_script() -> std::path::PathBuf {
        let path = std::env::temp_dir().join("trogon_cli_notif_echo_mcp.sh");
        tokio::fs::write(
            &path,
            b"#!/bin/sh\nwhile IFS= read -r line; do \
              printf '{\"jsonrpc\":\"2.0\",\"method\":\"notifications/test\"}\\n'; \
              id=$(echo \"$line\" | sed 's/.*\"id\":\\s*\\([0-9]*\\).*/\\1/'); \
              printf '{\"jsonrpc\":\"2.0\",\"id\":%s,\"result\":{}}\n' \"$id\"; \
            done\n",
        )
        .await
        .unwrap();
        use std::os::unix::fs::PermissionsExt;
        let mut perms = tokio::fs::metadata(&path).await.unwrap().permissions();
        perms.set_mode(0o755);
        tokio::fs::set_permissions(&path, perms).await.unwrap();
        path
    }

    #[tokio::test]
    async fn bridge_shutdown_kills_child() {
        let script = echo_script().await;
        let bridge = StdioMcpBridge::spawn(script.to_str().unwrap(), &[], &[])
            .await
            .expect("spawn must succeed");
        // shutdown must complete without hanging
        tokio::time::timeout(std::time::Duration::from_secs(5), bridge.shutdown())
            .await
            .expect("shutdown must complete within 5 s");
    }

    #[tokio::test]
    async fn bridge_ignores_non_json_lines_from_child() {
        let script = noisy_echo_script().await;
        let bridge = StdioMcpBridge::spawn(script.to_str().unwrap(), &[], &[])
            .await
            .expect("spawn must succeed");
        let client = reqwest::Client::new();
        let body = serde_json::json!({ "jsonrpc": "2.0", "id": 7, "method": "tools/list", "params": {} });
        let resp: serde_json::Value = client
            .post(&bridge.url)
            .json(&body)
            .send()
            .await
            .expect("request must succeed")
            .json()
            .await
            .expect("response must be JSON");
        assert_eq!(resp["id"], 7, "garbage lines must be skipped; id must match");
        bridge.shutdown().await;
    }

    #[tokio::test]
    async fn bridge_ignores_notification_lines_from_child() {
        let script = notification_echo_script().await;
        let bridge = StdioMcpBridge::spawn(script.to_str().unwrap(), &[], &[])
            .await
            .expect("spawn must succeed");
        let client = reqwest::Client::new();
        let body = serde_json::json!({ "jsonrpc": "2.0", "id": 8, "method": "tools/list", "params": {} });
        let resp: serde_json::Value = client
            .post(&bridge.url)
            .json(&body)
            .send()
            .await
            .expect("request must succeed")
            .json()
            .await
            .expect("response must be JSON");
        assert_eq!(resp["id"], 8, "notification lines must be discarded; id must match");
        bridge.shutdown().await;
    }

    #[tokio::test]
    async fn bridge_routes_concurrent_requests_to_correct_responses() {
        let script = echo_script().await;
        let bridge = StdioMcpBridge::spawn(script.to_str().unwrap(), &[], &[])
            .await
            .expect("spawn must succeed");
        let url = bridge.url.clone();
        let client = reqwest::Client::new();
        let url2 = url.clone();
        let client2 = client.clone();
        let (r1, r2) = tokio::join!(
            async move {
                client
                    .post(&url)
                    .json(&serde_json::json!({ "jsonrpc": "2.0", "id": 100, "method": "m" }))
                    .send()
                    .await
                    .unwrap()
                    .json::<serde_json::Value>()
                    .await
                    .unwrap()
            },
            async move {
                client2
                    .post(&url2)
                    .json(&serde_json::json!({ "jsonrpc": "2.0", "id": 200, "method": "m" }))
                    .send()
                    .await
                    .unwrap()
                    .json::<serde_json::Value>()
                    .await
                    .unwrap()
            }
        );
        assert_eq!(r1["id"], 100, "first response must have id 100");
        assert_eq!(r2["id"], 200, "second response must have id 200");
        bridge.shutdown().await;
    }

    #[test]
    fn bridge_error_display_spawn() {
        let err = BridgeError::Spawn(std::io::Error::new(std::io::ErrorKind::NotFound, "bin not found"));
        assert!(err.to_string().contains("failed to spawn process"), "got: {err}");
    }

    #[test]
    fn bridge_error_display_bind() {
        let err = BridgeError::Bind(std::io::Error::new(std::io::ErrorKind::AddrInUse, "port taken"));
        assert!(err.to_string().contains("failed to bind HTTP server"), "got: {err}");
    }

    #[tokio::test]
    async fn proxy_handler_rejects_invalid_json_with_400() {
        let script = echo_script().await;
        let bridge = StdioMcpBridge::spawn(script.to_str().unwrap(), &[], &[])
            .await
            .expect("spawn must succeed");
        let client = reqwest::Client::new();
        let resp = client
            .post(&bridge.url)
            .body("{not valid json")
            .send()
            .await
            .expect("HTTP request must succeed");
        assert_eq!(resp.status().as_u16(), 400, "invalid JSON must return 400");
        bridge.shutdown().await;
    }

    #[tokio::test]
    async fn proxy_handler_returns_200_for_notification_without_id() {
        let script = echo_script().await;
        let bridge = StdioMcpBridge::spawn(script.to_str().unwrap(), &[], &[])
            .await
            .expect("spawn must succeed");
        let client = reqwest::Client::new();
        let body = serde_json::json!({
            "jsonrpc": "2.0",
            "method": "notifications/initialized"
        });
        let resp = client
            .post(&bridge.url)
            .json(&body)
            .send()
            .await
            .expect("HTTP request must succeed");
        assert_eq!(resp.status().as_u16(), 200, "notification without id must return 200");
        bridge.shutdown().await;
    }

    #[tokio::test]
    async fn proxy_handler_returns_502_when_pending_sender_dropped() {
        let mut child = Command::new("sh")
            .args(["-c", "cat > /dev/null"])
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::null())
            .spawn()
            .unwrap();
        let stdin = child.stdin.take().unwrap();
        let inner = Arc::new(Inner {
            pending: Mutex::new(HashMap::new()),
            stdin: Mutex::new(stdin),
        });

        // Drop the sender for id=1 as soon as it appears in pending.
        let dropper = inner.clone();
        tokio::spawn(async move {
            loop {
                tokio::time::sleep(Duration::from_millis(10)).await;
                if dropper.pending.lock().await.remove(&1u64).is_some() {
                    break;
                }
            }
        });

        let body = Bytes::from(
            serde_json::to_vec(&serde_json::json!({
                "jsonrpc": "2.0",
                "id": 1,
                "method": "tools/list",
                "params": {}
            }))
            .unwrap(),
        );
        let resp = proxy_handler(State(inner), body).await;
        assert_eq!(resp.status(), StatusCode::BAD_GATEWAY, "dropped sender must return 502");
        let _ = child.kill().await;
    }

    #[tokio::test(flavor = "current_thread", start_paused = true)]
    async fn proxy_handler_returns_504_on_timeout() {
        let mut child = Command::new("sh")
            .args(["-c", "cat > /dev/null"])
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::null())
            .spawn()
            .unwrap();
        let stdin = child.stdin.take().unwrap();
        let inner = Arc::new(Inner {
            pending: Mutex::new(HashMap::new()),
            stdin: Mutex::new(stdin),
        });

        let body = Bytes::from(
            serde_json::to_vec(&serde_json::json!({
                "jsonrpc": "2.0",
                "id": 99,
                "method": "tools/list",
                "params": {}
            }))
            .unwrap(),
        );

        // Run the handler as a concurrent task so we can advance time from outside.
        let handler = tokio::spawn(proxy_handler(State(inner.clone()), body));
        // Yield enough times for the handler to complete its stdin write and reach
        // the 30-second timeout await.
        for _ in 0..20 {
            tokio::task::yield_now().await;
        }
        tokio::time::advance(Duration::from_secs(35)).await;
        for _ in 0..10 {
            tokio::task::yield_now().await;
        }

        let resp = handler.await.unwrap();
        assert_eq!(resp.status(), StatusCode::GATEWAY_TIMEOUT, "timeout must return 504");
        let _ = child.kill().await;
    }
}
