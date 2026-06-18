//! Native in-process stdio MCP client.
//!
//! Speaks JSON-RPC over a subprocess's stdin/stdout (newline-delimited messages,
//! the MCP stdio transport). This lets stdio MCP servers run directly inside the
//! runner process — no intermediate local HTTP bridge process (the extra hop the
//! HTTP-only [`crate::McpClient`] would otherwise require for a stdio server).

use std::future::Future;
use std::pin::Pin;
use std::time::Duration;

use serde::Deserialize;
use serde_json::{Value, json};
use tokio::io::{AsyncBufReadExt, AsyncRead, AsyncWrite, AsyncWriteExt, BufReader, Lines};
use tokio::process::{Child, Command};
use tokio::sync::Mutex;
use tracing::debug;

use crate::client::{McpCallTool, McpTool, next_id};

/// Default per-request timeout so a hung server can never stall the agent loop.
const DEFAULT_STDIO_TIMEOUT: Duration = Duration::from_secs(30);

#[derive(Deserialize)]
struct ListToolsResult {
    #[serde(default)]
    tools: Vec<McpTool>,
}

#[derive(Deserialize)]
struct CallToolResult {
    #[serde(default)]
    content: Vec<StdioContentBlock>,
    #[serde(rename = "isError", default)]
    is_error: bool,
}

#[derive(Deserialize)]
struct StdioContentBlock {
    #[serde(rename = "type")]
    block_type: String,
    text: Option<String>,
}

/// One JSON-RPC-over-stdio connection. The byte streams are boxed trait objects
/// so the transport logic can be unit-tested over in-memory pipes (no subprocess).
struct StdioConn {
    stdin: Box<dyn AsyncWrite + Send + Unpin>,
    stdout: Lines<BufReader<Box<dyn AsyncRead + Send + Unpin>>>,
    /// Kept alive so the child isn't reaped while the client is in use (`kill_on_drop`
    /// then terminates it when the client is dropped). `None` in unit tests.
    _child: Option<Child>,
}

impl StdioConn {
    /// Send one JSON-RPC message. For a request (has `id`), read newline-delimited
    /// responses until one matches the request id, skipping server notifications.
    /// For a notification (no `id`), write and return immediately.
    async fn request(&mut self, body: Value, timeout: Duration) -> Result<Value, String> {
        let id = body.get("id").cloned();
        let mut line =
            serde_json::to_string(&body).map_err(|e| format!("MCP stdio encode error: {e}"))?;
        line.push('\n');
        tokio::time::timeout(timeout, async {
            self.stdin.write_all(line.as_bytes()).await?;
            self.stdin.flush().await
        })
        .await
        .map_err(|_| "MCP stdio write timed out".to_string())?
        .map_err(|e| format!("MCP stdio write error: {e}"))?;
        // A notification expects no response.
        if id.is_none() {
            return Ok(Value::Null);
        }
        loop {
            let next = tokio::time::timeout(timeout, self.stdout.next_line())
                .await
                .map_err(|_| "MCP stdio read timed out".to_string())?
                .map_err(|e| format!("MCP stdio read error: {e}"))?;
            let Some(text) = next else {
                return Err("MCP stdio server closed the connection".to_string());
            };
            if text.trim().is_empty() {
                continue;
            }
            let Ok(v) = serde_json::from_str::<Value>(&text) else {
                debug!(line = %text, "MCP stdio: skipping non-JSON stdout line");
                continue;
            };
            // Skip server-initiated notifications and id mismatches.
            if v.get("id") == id.as_ref() {
                return Ok(v);
            }
        }
    }
}

/// A native MCP client that drives a server subprocess over stdio.
///
/// ```no_run
/// # async fn example() -> Result<(), String> {
/// let client = trogon_mcp::StdioMcpClient::spawn(
///     "npx", &["-y".into(), "@modelcontextprotocol/server-filesystem".into(), ".".into()],
///     &[], None,
/// ).await?;
/// client.initialize().await?;
/// let tools = client.list_tools().await?;
/// let out = client.call_tool("read_file", &serde_json::json!({"path": "x"})).await?;
/// # let _ = (tools, out); Ok(()) }
/// ```
pub struct StdioMcpClient {
    conn: Mutex<StdioConn>,
    timeout: Duration,
}

impl StdioMcpClient {
    /// Spawn `command args…` with extra `env`, connecting over its stdin/stdout.
    /// `cwd` sets the child working directory when provided.
    pub async fn spawn(
        command: &str,
        args: &[String],
        env: &[(String, String)],
        cwd: Option<&str>,
    ) -> Result<Self, String> {
        let mut cmd = Command::new(command);
        cmd.args(args)
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::null())
            .kill_on_drop(true);
        for (k, v) in env {
            cmd.env(k, v);
        }
        if let Some(dir) = cwd {
            cmd.current_dir(dir);
        }
        let mut child = cmd
            .spawn()
            .map_err(|e| format!("MCP stdio spawn `{command}` failed: {e}"))?;
        let stdin = child.stdin.take().ok_or("MCP stdio: no child stdin")?;
        let stdout = child.stdout.take().ok_or("MCP stdio: no child stdout")?;
        Ok(Self {
            conn: Mutex::new(StdioConn {
                stdin: Box::new(stdin),
                stdout: BufReader::new(Box::new(stdout) as Box<dyn AsyncRead + Send + Unpin>)
                    .lines(),
                _child: Some(child),
            }),
            timeout: DEFAULT_STDIO_TIMEOUT,
        })
    }

    /// Override the per-request timeout (builder style).
    #[must_use]
    pub fn with_timeout(mut self, timeout: Duration) -> Self {
        self.timeout = timeout;
        self
    }

    /// Perform the MCP `initialize` handshake, then send the `initialized`
    /// notification. Must be called once before `list_tools`/`call_tool`.
    pub async fn initialize(&self) -> Result<(), String> {
        let mut conn = self.conn.lock().await;
        let timeout = self.timeout;
        let resp = conn
            .request(
                json!({
                "jsonrpc": "2.0",
                "id": next_id(),
                "method": "initialize",
                "params": {
                    "protocolVersion": "2024-11-05",
                    "capabilities": {},
                    "clientInfo": { "name": "trogon", "version": "0.1.0" }
                }
            }),
                timeout,
            )
            .await?;
        if let Some(err) = resp.get("error") {
            return Err(format!("MCP initialize error: {err}"));
        }
        // Per the MCP stdio spec, follow up with the initialized notification.
        conn.request(
            json!({
            "jsonrpc": "2.0",
            "method": "notifications/initialized"
        }),
            timeout,
        )
        .await?;
        Ok(())
    }

    /// Retrieve the list of tools the server exposes (`tools/list`).
    pub async fn list_tools(&self) -> Result<Vec<McpTool>, String> {
        let mut conn = self.conn.lock().await;
        let timeout = self.timeout;
        let mut resp = conn
            .request(
                json!({
                "jsonrpc": "2.0", "id": next_id(), "method": "tools/list", "params": {}
            }),
                timeout,
            )
            .await?;
        if let Some(err) = resp.get("error") {
            return Err(format!("MCP tools/list error: {err}"));
        }
        let result: ListToolsResult = serde_json::from_value(resp["result"].take())
            .map_err(|e| format!("MCP tools/list deserialize error: {e}"))?;
        Ok(result.tools)
    }

    /// Call a tool by its original (non-prefixed) name and return the text output.
    pub async fn call_tool(&self, name: &str, arguments: &Value) -> Result<String, String> {
        let mut conn = self.conn.lock().await;
        let timeout = self.timeout;
        let mut resp = conn
            .request(
                json!({
                "jsonrpc": "2.0", "id": next_id(), "method": "tools/call",
                "params": { "name": name, "arguments": arguments }
            }),
                timeout,
            )
            .await?;
        if let Some(err) = resp.get("error") {
            return Err(format!("MCP tool error: {err}"));
        }
        let result: CallToolResult = serde_json::from_value(resp["result"].take())
            .map_err(|e| format!("MCP tools/call deserialize error: {e}"))?;
        let text = result
            .content
            .iter()
            .filter(|b| b.block_type == "text")
            .filter_map(|b| b.text.as_deref())
            .collect::<Vec<_>>()
            .join("\n");
        if result.is_error { Err(text) } else { Ok(text) }
    }
}

impl McpCallTool for StdioMcpClient {
    fn call_tool<'a>(
        &'a self,
        name: &'a str,
        arguments: &'a Value,
    ) -> Pin<Box<dyn Future<Output = Result<String, String>> + Send + 'a>> {
        Box::pin(async move { StdioMcpClient::call_tool(self, name, arguments).await })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader as TokBufReader};

    /// Build a `StdioMcpClient` wired to an in-memory duplex pipe, plus the
    /// server-side halves a fake server task reads/writes.
    fn wired_client(timeout: Duration) -> (
        StdioMcpClient,
        impl AsyncWrite + Send + Unpin,
        impl tokio::io::AsyncRead + Send + Unpin,
    ) {
        let (client_io, server_io) = tokio::io::duplex(8192);
        let (cr, cw) = tokio::io::split(client_io);
        let (sr, sw) = tokio::io::split(server_io);
        let conn = StdioConn {
            stdin: Box::new(cw),
            stdout: BufReader::new(Box::new(cr) as Box<dyn AsyncRead + Send + Unpin>).lines(),
            _child: None,
        };
        (
            StdioMcpClient {
                conn: Mutex::new(conn),
                timeout,
            },
            sw,
            sr,
        )
    }

    /// Minimal fake MCP server: answers initialize/tools/list/tools/call, echoing
    /// the request id, and ignores the `initialized` notification.
    async fn fake_server(
        mut writer: impl AsyncWrite + Send + Unpin,
        reader: impl tokio::io::AsyncRead + Send + Unpin,
    ) {
        let mut lines = TokBufReader::new(reader).lines();
        while let Ok(Some(line)) = lines.next_line().await {
            if line.trim().is_empty() {
                continue;
            }
            let req: Value = serde_json::from_str(&line).unwrap();
            let method = req.get("method").and_then(|m| m.as_str()).unwrap_or("");
            // Notifications carry no id and want no reply.
            let Some(id) = req.get("id").cloned() else {
                continue;
            };
            let result = match method {
                "initialize" => json!({"protocolVersion": "2024-11-05", "capabilities": {}}),
                "tools/list" => json!({"tools": [
                    {"name": "echo", "description": "echoes", "inputSchema": {"type": "object"}}
                ]}),
                "tools/call" => {
                    let args = req
                        .get("params")
                        .and_then(|p| p.get("arguments"))
                        .cloned()
                        .unwrap_or(Value::Null);
                    json!({"content": [{"type": "text", "text": format!("called:{args}")}], "isError": false})
                }
                _ => json!({}),
            };
            let resp = json!({"jsonrpc": "2.0", "id": id, "result": result});
            let mut out = serde_json::to_string(&resp).unwrap();
            out.push('\n');
            writer.write_all(out.as_bytes()).await.unwrap();
            writer.flush().await.unwrap();
        }
    }

    #[tokio::test]
    async fn initialize_list_and_call_round_trip() {
        let (client, sw, sr) = wired_client(Duration::from_secs(5));
        tokio::spawn(fake_server(sw, sr));

        client.initialize().await.expect("initialize");

        let tools = client.list_tools().await.expect("list_tools");
        assert_eq!(tools.len(), 1);
        assert_eq!(tools[0].name, "echo");

        let out = client
            .call_tool("echo", &json!({"msg": "hi"}))
            .await
            .expect("call_tool");
        assert!(out.contains("called:"), "got: {out}");
        assert!(out.contains("\"msg\":\"hi\""), "got: {out}");
    }

    #[tokio::test]
    async fn call_tool_via_trait_object() {
        let (client, sw, sr) = wired_client(Duration::from_secs(5));
        tokio::spawn(fake_server(sw, sr));
        client.initialize().await.expect("initialize");

        let dynref: &dyn McpCallTool = &client;
        let out = dynref.call_tool("echo", &json!({"k": 1})).await.expect("call");
        assert!(out.contains("called:"));
    }

    #[tokio::test]
    async fn read_timeout_when_server_silent() {
        // No fake server attached → the read side never produces a line.
        let (client_io, _server_io) = tokio::io::duplex(64);
        let (cr, cw) = tokio::io::split(client_io);
        let conn = StdioConn {
            stdin: Box::new(cw),
            stdout: BufReader::new(Box::new(cr) as Box<dyn AsyncRead + Send + Unpin>).lines(),
            _child: None,
        };
        let client = StdioMcpClient {
            conn: Mutex::new(conn),
            timeout: Duration::from_millis(80),
        };
        let err = client.list_tools().await.expect_err("should time out");
        assert!(err.contains("timed out"), "got: {err}");
    }

    #[tokio::test]
    async fn skips_non_json_stdout_lines_before_response() {
        let (client, mut sw, sr) = wired_client(Duration::from_secs(5));
        tokio::spawn(async move {
            let mut lines = TokBufReader::new(sr).lines();
            while let Ok(Some(line)) = lines.next_line().await {
                if line.trim().is_empty() {
                    continue;
                }
                let req: Value = serde_json::from_str(&line).unwrap();
                let Some(id) = req.get("id").cloned() else {
                    continue;
                };
                sw.write_all(b"Server starting...\n").await.unwrap();
                sw.flush().await.unwrap();
                let resp = json!({"jsonrpc":"2.0","id":id,"result":{"tools":[
                    {"name":"echo","description":"","inputSchema":{"type":"object"}}
                ]}});
                let mut out = serde_json::to_string(&resp).unwrap();
                out.push('\n');
                sw.write_all(out.as_bytes()).await.unwrap();
                sw.flush().await.unwrap();
            }
        });

        client.initialize().await.expect("initialize");
        let tools = client
            .list_tools()
            .await
            .expect("list_tools after banner line");
        assert_eq!(tools.len(), 1);
        assert_eq!(tools[0].name, "echo");
    }

    #[tokio::test]
    async fn with_timeout_applies_to_requests() {
        let (client_io, _server_io) = tokio::io::duplex(64);
        let (cr, cw) = tokio::io::split(client_io);
        let conn = StdioConn {
            stdin: Box::new(cw),
            stdout: BufReader::new(Box::new(cr) as Box<dyn AsyncRead + Send + Unpin>).lines(),
            _child: None,
        };
        let client = StdioMcpClient {
            conn: Mutex::new(conn),
            timeout: DEFAULT_STDIO_TIMEOUT,
        }
        .with_timeout(Duration::from_millis(80));

        let err = client.list_tools().await.expect_err("should time out");
        assert!(err.contains("timed out"), "got: {err}");
    }
}
