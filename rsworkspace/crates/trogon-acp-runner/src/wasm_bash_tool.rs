use std::future::Future;
use std::path::PathBuf;
use std::pin::Pin;
use std::sync::Arc;
use std::time::Duration;

use agent_client_protocol::{
    CreateTerminalRequest, CreateTerminalResponse, TerminalOutputRequest,
};
use serde_json::Value;
use trogon_agent_core::tools::ToolDef;
use trogon_mcp::McpCallTool;

use crate::session_store::SessionStore;

const POLL_INTERVAL: Duration = Duration::from_millis(100);
const EXIT_MARKER_PREFIX: &str = "__EXIT_";
const EXIT_MARKER_SUFFIX: &str = "__";

/// Implements the `bash` tool by delegating execution to a running `trogon-wasm-runtime`
/// instance discovered via the agent registry.
///
/// Maintains a persistent bash terminal per session — the first call creates it
/// (using `sandbox_dir` as the working directory) and saves its ID in `SessionState`
/// (persisted to NATS KV). Subsequent calls reuse the same terminal via
/// `terminal.write_stdin`, using the demarcation protocol
/// `<command>; echo "__EXIT_$?__"` to detect completion.
pub struct WasmRuntimeBashTool<S> {
    nats: async_nats::Client,
    wasm_prefix: String,
    session_id: String,
    sandbox_dir: PathBuf,
    timeout: Duration,
    store: S,
}

impl<S: SessionStore> WasmRuntimeBashTool<S> {
    pub fn new(
        nats: async_nats::Client,
        wasm_prefix: impl Into<String>,
        session_id: impl Into<String>,
        sandbox_dir: PathBuf,
        timeout: Duration,
        store: S,
    ) -> Self {
        Self {
            nats,
            wasm_prefix: wasm_prefix.into(),
            session_id: session_id.into(),
            sandbox_dir,
            timeout,
            store,
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

    pub fn into_dispatch(self) -> (String, String, Arc<dyn McpCallTool>)
    where
        S: Send + Sync + 'static,
    {
        ("bash".to_string(), "bash".to_string(), Arc::new(self))
    }
}

impl<S: SessionStore> McpCallTool for WasmRuntimeBashTool<S> {
    fn call_tool<'a>(
        &'a self,
        _name: &'a str,
        arguments: &'a Value,
    ) -> Pin<Box<dyn Future<Output = Result<String, String>> + Send + 'a>> {
        let nats = self.nats.clone();
        let wasm_prefix = self.wasm_prefix.clone();
        let session_id = self.session_id.clone();
        let sandbox_dir = self.sandbox_dir.clone();
        let timeout = self.timeout;
        let store = self.store.clone();

        Box::pin(async move {
            let command = arguments["command"]
                .as_str()
                .ok_or_else(|| "missing command argument".to_string())?
                .to_string();

            let term_base = format!("{wasm_prefix}.session.{session_id}.client.terminal");
            let ext_base = format!("{wasm_prefix}.session.{session_id}.client.ext");

            // ── Obtain or create the persistent terminal ──────────────────────
            let mut state = store.load(&session_id).await.map_err(|e| e.to_string())?;

            let terminal_id: String = if let Some(tid) = &state.terminal_id {
                tid.clone()
            } else {
                let create_req = CreateTerminalRequest::new(session_id.clone(), "bash")
                    .cwd(sandbox_dir.clone());
                let payload =
                    serde_json::to_vec(&create_req).map_err(|e| e.to_string())?;
                let msg = nats
                    .request(format!("{term_base}.create"), payload.into())
                    .await
                    .map_err(|e| e.to_string())?;
                let resp: CreateTerminalResponse =
                    serde_json::from_slice(&msg.payload).map_err(|e| e.to_string())?;
                let tid = resp.terminal_id.0.to_string();
                state.terminal_id = Some(tid.clone());
                store.save(&session_id, &state).await.map_err(|e| e.to_string())?;
                tid
            };

            // ── Snapshot baseline output length ───────────────────────────────
            let baseline_len = {
                let req = TerminalOutputRequest::new(
                    session_id.clone(),
                    terminal_id.clone(),
                );
                let payload = serde_json::to_vec(&req).map_err(|e| e.to_string())?;
                let msg = nats
                    .request(format!("{term_base}.output"), payload.into())
                    .await
                    .map_err(|e| e.to_string())?;
                let resp: serde_json::Value =
                    serde_json::from_slice(&msg.payload).map_err(|e| e.to_string())?;
                resp["output"].as_str().unwrap_or("").len()
            };

            // ── Write command with demarcation marker ─────────────────────────
            let cmd_with_marker = format!("{command}; echo \"{EXIT_MARKER_PREFIX}$?{EXIT_MARKER_SUFFIX}\"\n");
            let write_req = serde_json::json!({
                "terminal_id": terminal_id,
                "data": cmd_with_marker.as_bytes()
            });
            let payload = serde_json::to_vec(&write_req).map_err(|e| e.to_string())?;
            nats.request(
                format!("{ext_base}.terminal.write_stdin"),
                payload.into(),
            )
            .await
            .map_err(|e| e.to_string())?;

            // ── Poll for output until marker found or timeout ─────────────────
            let deadline = tokio::time::Instant::now() + timeout;

            loop {
                tokio::time::sleep(POLL_INTERVAL).await;

                let req = TerminalOutputRequest::new(
                    session_id.clone(),
                    terminal_id.clone(),
                );
                let payload = serde_json::to_vec(&req).map_err(|e| e.to_string())?;
                let msg = nats
                    .request(format!("{term_base}.output"), payload.into())
                    .await
                    .map_err(|e| e.to_string())?;
                let resp: serde_json::Value =
                    serde_json::from_slice(&msg.payload).map_err(|e| e.to_string())?;
                let full_output = resp["output"].as_str().unwrap_or("").to_string();

                if full_output.len() > baseline_len {
                    let new_output = &full_output[baseline_len..];
                    if let Some(output) = extract_before_marker(new_output) {
                        return Ok(output);
                    }
                }

                if tokio::time::Instant::now() >= deadline {
                    let new_output = if full_output.len() > baseline_len {
                        full_output[baseline_len..].to_string()
                    } else {
                        String::new()
                    };
                    return Err(format!(
                        "timeout after {}s. Partial output:\n{new_output}",
                        timeout.as_secs()
                    ));
                }
            }
        })
    }
}

/// Searches `output` for `__EXIT_N__`, returns everything before the marker.
/// Returns `None` if the marker is not yet present.
fn extract_before_marker(output: &str) -> Option<String> {
    // Find the last occurrence so partial writes don't confuse us.
    let mut last_match: Option<usize> = None;
    let mut search = output;
    let mut offset = 0;
    while let Some(pos) = search.find(EXIT_MARKER_PREFIX) {
        let abs = offset + pos;
        let after = &output[abs + EXIT_MARKER_PREFIX.len()..];
        if let Some(end) = after.find(EXIT_MARKER_SUFFIX) {
            let code_str = &after[..end];
            if code_str.chars().all(|c| c.is_ascii_digit()) {
                last_match = Some(abs);
            }
        }
        offset = abs + EXIT_MARKER_PREFIX.len();
        search = &output[offset..];
    }
    last_match.map(|pos| {
        let before = &output[..pos];
        before.strip_suffix('\n').unwrap_or(before).to_string()
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extract_before_marker_finds_exit_zero() {
        let output = "hello\nworld\n__EXIT_0__\n";
        assert_eq!(
            extract_before_marker(output),
            Some("hello\nworld".to_string())
        );
    }

    #[test]
    fn extract_before_marker_finds_nonzero_exit_code() {
        let output = "error output\n__EXIT_1__\n";
        assert_eq!(
            extract_before_marker(output),
            Some("error output".to_string())
        );
    }

    #[test]
    fn extract_before_marker_returns_none_when_absent() {
        assert_eq!(extract_before_marker("no marker here"), None);
    }

    #[test]
    fn extract_before_marker_uses_last_marker() {
        let output = "__EXIT_0__\nmore output\n__EXIT_1__\n";
        let result = extract_before_marker(output).unwrap();
        assert!(result.contains("more output"), "got: {result}");
    }

    #[test]
    fn extract_before_marker_ignores_non_numeric_codes() {
        let output = "__EXIT_abc__\nreal output\n__EXIT_0__\n";
        let result = extract_before_marker(output).unwrap();
        assert!(result.contains("real output"), "got: {result}");
    }

    // ── tool_def ──────────────────────────────────────────────────────────────

    #[test]
    fn tool_def_name_is_bash() {
        use crate::session_store::mock::MemorySessionStore;
        let def = WasmRuntimeBashTool::<MemorySessionStore>::tool_def();
        assert_eq!(def.name, "bash");
    }

    #[test]
    fn tool_def_description_is_non_empty() {
        use crate::session_store::mock::MemorySessionStore;
        let def = WasmRuntimeBashTool::<MemorySessionStore>::tool_def();
        assert!(!def.description.is_empty());
    }

    #[test]
    fn tool_def_cache_control_is_none() {
        use crate::session_store::mock::MemorySessionStore;
        let def = WasmRuntimeBashTool::<MemorySessionStore>::tool_def();
        assert!(def.cache_control.is_none());
    }

    #[test]
    fn tool_def_schema_requires_command() {
        use crate::session_store::mock::MemorySessionStore;
        let def = WasmRuntimeBashTool::<MemorySessionStore>::tool_def();
        let required = def.input_schema["required"]
            .as_array()
            .expect("required must be an array");
        assert!(
            required.iter().any(|v| v.as_str() == Some("command")),
            "schema must require 'command', got: {required:?}"
        );
    }

    #[test]
    fn tool_def_schema_command_property_is_string_type() {
        use crate::session_store::mock::MemorySessionStore;
        let def = WasmRuntimeBashTool::<MemorySessionStore>::tool_def();
        let ty = def.input_schema["properties"]["command"]["type"]
            .as_str()
            .expect("command type must be a string");
        assert_eq!(ty, "string");
    }
}
