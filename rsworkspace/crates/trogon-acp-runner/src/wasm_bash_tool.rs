use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::time::Duration;

use agent_client_protocol::{
    CreateTerminalRequest, CreateTerminalResponse, ReleaseTerminalRequest, TerminalOutputRequest,
    TerminalOutputResponse, WaitForTerminalExitRequest,
};
use serde_json::Value;
use trogon_agent_core::tools::ToolDef;
use trogon_mcp::McpCallTool;

/// Implements the `bash` tool by delegating execution to a running `trogon-wasm-runtime`
/// instance discovered via the agent registry.
///
/// Uses `async_nats::Client::request` directly (not `NatsClientProxy`) so that
/// the returned future is `Send`, satisfying the `McpCallTool` trait bound.
pub struct WasmRuntimeBashTool {
    nats: async_nats::Client,
    wasm_prefix: String,
    session_id: String,
    timeout: Duration,
}

impl WasmRuntimeBashTool {
    pub fn new(
        nats: async_nats::Client,
        wasm_prefix: impl Into<String>,
        session_id: impl Into<String>,
        timeout: Duration,
    ) -> Self {
        Self {
            nats,
            wasm_prefix: wasm_prefix.into(),
            session_id: session_id.into(),
            timeout,
        }
    }

    pub fn tool_def() -> ToolDef {
        ToolDef {
            name: "bash".to_string(),
            description: "Run a shell command in the session sandbox and return its output."
                .to_string(),
            input_schema: serde_json::json!({
                "type": "object",
                "properties": {
                    "command": {
                        "type": "string",
                        "description": "The shell command to execute."
                    }
                },
                "required": ["command"]
            }),
            cache_control: None,
        }
    }

    /// Returns the dispatch tuple expected by `AgentRunner::add_mcp_tools`.
    ///
    /// The first element must match the `ToolDef` name exactly — `agent_loop.rs`
    /// looks up dispatch entries by that value.
    pub fn into_dispatch(self) -> (String, String, Arc<dyn McpCallTool>) {
        ("bash".to_string(), "bash".to_string(), Arc::new(self))
    }
}

impl McpCallTool for WasmRuntimeBashTool {
    fn call_tool<'a>(
        &'a self,
        _name: &'a str,
        arguments: &'a Value,
    ) -> Pin<Box<dyn Future<Output = Result<String, String>> + Send + 'a>> {
        // Clone owned values before entering the async block so the future does
        // not hold a &'a str derived from self — CreateTerminalRequest::new
        // requires Into<SessionId> with no borrowed lifetime.
        let nats = self.nats.clone();
        let wasm_prefix = self.wasm_prefix.clone();
        let session_id = self.session_id.clone();
        let timeout = self.timeout;

        Box::pin(async move {
            let command = arguments["command"]
                .as_str()
                .ok_or_else(|| "missing command argument".to_string())?
                .to_string();

            let base = format!("{wasm_prefix}.session.{session_id}.client.terminal");

            tokio::time::timeout(timeout, async move {
                // 1. create terminal — bash -c <command>
                let create_req = CreateTerminalRequest::new(session_id.clone(), "bash")
                    .args(vec!["-c".to_string(), command]);
                let payload = serde_json::to_vec(&create_req).map_err(|e| e.to_string())?;
                let msg = nats
                    .request(format!("{base}.create"), payload.into())
                    .await
                    .map_err(|e| e.to_string())?;
                let create_resp: CreateTerminalResponse =
                    serde_json::from_slice(&msg.payload).map_err(|e| e.to_string())?;
                let tid = create_resp.terminal_id.clone();

                // 2. wait for exit
                let wait_req = WaitForTerminalExitRequest::new(session_id.clone(), tid.clone());
                let payload = serde_json::to_vec(&wait_req).map_err(|e| e.to_string())?;
                nats.request(format!("{base}.wait_for_exit"), payload.into())
                    .await
                    .map_err(|e| e.to_string())?;

                // 3. collect output
                let out_req = TerminalOutputRequest::new(session_id.clone(), tid.clone());
                let payload = serde_json::to_vec(&out_req).map_err(|e| e.to_string())?;
                let msg = nats
                    .request(format!("{base}.output"), payload.into())
                    .await
                    .map_err(|e| e.to_string())?;
                let out: TerminalOutputResponse =
                    serde_json::from_slice(&msg.payload).map_err(|e| e.to_string())?;

                // 4. release (best-effort, fire-and-forget — no reply expected)
                let rel_req = ReleaseTerminalRequest::new(session_id, tid);
                if let Ok(payload) = serde_json::to_vec(&rel_req) {
                    let _ = nats.publish(format!("{base}.release"), payload.into()).await;
                }

                Ok(out.output)
            })
            .await
            .unwrap_or_else(|_| Err("bash execution timed out".to_string()))
        })
    }
}
