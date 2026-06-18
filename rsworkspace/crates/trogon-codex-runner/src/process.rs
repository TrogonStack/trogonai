use std::collections::HashMap;
use std::sync::Arc;
use std::sync::Mutex as StdMutex;
use std::sync::atomic::{AtomicBool, Ordering};

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::{Child, ChildStdin, ChildStdout, Command};
use tokio::sync::{Mutex, broadcast, oneshot};
use tracing::{debug, trace, warn};

use crate::traits::{CodexProcessClient, ProcessSpawner};

// ── JSON-RPC wire types ────────────────────────────────────────────────────────

#[derive(Serialize)]
struct Request<'a> {
    id: u64,
    method: &'a str,
    #[serde(skip_serializing_if = "Option::is_none")]
    params: Option<Value>,
}

#[derive(Serialize)]
struct Response<'a> {
    id: &'a Value,
    result: Value,
}

#[derive(Serialize)]
struct Notification<'a> {
    method: &'a str,
    #[serde(skip_serializing_if = "Option::is_none")]
    params: Option<Value>,
}

#[derive(Debug, Deserialize)]
struct IncomingMessage {
    #[serde(default)]
    pub id: Option<serde_json::Number>,
    #[serde(default)]
    pub method: Option<String>,
    #[serde(default)]
    pub result: Option<Value>,
    #[serde(default)]
    pub error: Option<Value>,
    #[serde(default)]
    pub params: Option<Value>,
}

// ── Codex event types ──────────────────────────────────────────────────────────

/// Parsed event emitted by the Codex app-server during a turn.
#[derive(Debug, Clone)]
pub enum CodexEvent {
    /// Incremental assistant text from `item/agentMessage/delta`.
    TextDelta { text: String },
    /// Incremental reasoning text from `item/reasoning/*Delta` notifications.
    ReasoningDelta { text: String },
    /// A tool/command execution started (`item/started`).
    ToolStarted {
        id: String,
        name: String,
        input: Value,
    },
    /// A tool/command completed (`item/completed`).
    ToolCompleted { id: String, output: String },
    /// Token usage from `thread/tokenUsage/updated`.
    Usage {
        input: u64,
        output: u64,
        total: u64,
        context_window: Option<u64>,
    },
    /// The turn finished (`turn/completed`).
    TurnCompleted,
    /// An error occurred during the turn.
    Error { message: String },
}

// ── CodexProcess ─────────────────────────────────────────────────────────────

type PendingMap = Arc<Mutex<HashMap<u64, oneshot::Sender<Result<Value, String>>>>>;

/// Per-thread event channels. Keyed by Codex `thread_id`.
type TurnSenders = Arc<Mutex<HashMap<String, broadcast::Sender<CodexEvent>>>>;

/// Trogon permission mode per active turn, used to auto-reply to server approval requests.
type ThreadModes = Arc<Mutex<HashMap<String, String>>>;

/// Manages a single `codex app-server` subprocess.
/// Speaks the Codex JSON-RPC protocol over stdin/stdout.
pub struct CodexProcess {
    stdin: Arc<Mutex<ChildStdin>>,
    next_id: Arc<Mutex<u64>>,
    pending: PendingMap,
    turn_senders: TurnSenders,
    thread_modes: ThreadModes,
    alive: Arc<AtomicBool>,
    _child: StdMutex<Child>,
}

impl Drop for CodexProcess {
    fn drop(&mut self) {
        if let Ok(mut child) = self._child.lock() {
            let _ = child.start_kill();
        }
    }
}

impl CodexProcess {
    pub fn is_alive(&self) -> bool {
        self.alive.load(Ordering::Relaxed)
    }

    pub fn alive_flag(&self) -> Arc<AtomicBool> {
        Arc::clone(&self.alive)
    }

    pub async fn spawn() -> Result<Self, Box<dyn std::error::Error + Send + Sync>> {
        let bin = std::env::var("CODEX_BIN").unwrap_or_else(|_| "codex".to_string());
        let mut child = Command::new(&bin)
            .arg("app-server")
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::inherit())
            .spawn()?;

        let stdin = child.stdin.take().expect("stdin piped");
        let stdout = child.stdout.take().expect("stdout piped");

        let pending: PendingMap = Arc::new(Mutex::new(HashMap::new()));
        let turn_senders: TurnSenders = Arc::new(Mutex::new(HashMap::new()));
        let thread_modes: ThreadModes = Arc::new(Mutex::new(HashMap::new()));
        let alive = Arc::new(AtomicBool::new(true));

        let stdin_arc = Arc::new(Mutex::new(stdin));

        let process = Self {
            stdin: stdin_arc.clone(),
            next_id: Arc::new(Mutex::new(1)),
            pending: pending.clone(),
            turn_senders: turn_senders.clone(),
            thread_modes: thread_modes.clone(),
            alive: alive.clone(),
            _child: StdMutex::new(child),
        };

        tokio::task::spawn_local(Self::read_loop(
            stdout,
            stdin_arc,
            pending,
            turn_senders,
            thread_modes,
            alive,
        ));

        let spawn_timeout_secs = std::env::var("CODEX_SPAWN_TIMEOUT_SECS")
            .ok()
            .and_then(|s| s.parse::<u64>().ok())
            .unwrap_or(30);
        tokio::time::timeout(
            std::time::Duration::from_secs(spawn_timeout_secs),
            process.initialize(),
        )
        .await
        .map_err(|_| {
            format!("codex app-server handshake timed out after {spawn_timeout_secs} s")
        })??;

        Ok(process)
    }

    async fn initialize(&self) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        let params = serde_json::json!({
            "clientInfo": {
                "name": "trogon-codex-runner",
                "version": env!("CARGO_PKG_VERSION")
            }
        });
        self.request("initialize", Some(params)).await?;
        self.notify("initialized", None).await?;
        Ok(())
    }

    /// Start a new thread (ACP session). Returns the Codex thread `id`.
    pub async fn thread_start(
        &self,
        cwd: &str,
    ) -> Result<String, Box<dyn std::error::Error + Send + Sync>> {
        let params = serde_json::json!({ "cwd": cwd });
        let result = self.request("thread/start", Some(params)).await?;
        Self::thread_id_from_result(&result)
    }

    /// Resume an existing thread. Returns the same `thread_id`.
    pub async fn thread_resume(
        &self,
        thread_id: &str,
    ) -> Result<String, Box<dyn std::error::Error + Send + Sync>> {
        let params = serde_json::json!({ "threadId": thread_id });
        let result = self.request("thread/resume", Some(params)).await?;
        Self::thread_id_from_result(&result).or(Ok(thread_id.to_string()))
    }

    /// Fork a thread. Returns the new thread `id`.
    pub async fn thread_fork(
        &self,
        thread_id: &str,
    ) -> Result<String, Box<dyn std::error::Error + Send + Sync>> {
        let params = serde_json::json!({ "threadId": thread_id });
        let result = self.request("thread/fork", Some(params)).await?;
        Self::thread_id_from_result(&result)
    }

    /// Start a turn and return a receiver for streaming `CodexEvent`s.
    pub async fn turn_start(
        &self,
        thread_id: &str,
        user_input: &str,
        model: Option<&str>,
        approval_policy: Option<&str>,
        permission_mode: Option<&str>,
    ) -> Result<broadcast::Receiver<CodexEvent>, Box<dyn std::error::Error + Send + Sync>> {
        let (tx, rx) = broadcast::channel(256);
        self.turn_senders
            .lock()
            .await
            .insert(thread_id.to_string(), tx);

        if let Some(mode) = permission_mode {
            self.thread_modes
                .lock()
                .await
                .insert(thread_id.to_string(), mode.to_string());
        } else {
            self.thread_modes.lock().await.remove(thread_id);
        }

        let input = serde_json::json!([{
            "type": "text",
            "text": user_input,
        }]);
        let mut params = serde_json::json!({
            "threadId": thread_id,
            "input": input,
        });
        if let Some(m) = model {
            params["model"] = Value::String(m.to_string());
        }
        if let Some(policy) = approval_policy {
            params["approvalPolicy"] = Value::String(policy.to_string());
        }
        if let Err(e) = self.request("turn/start", Some(params)).await {
            self.turn_senders.lock().await.remove(thread_id);
            self.thread_modes.lock().await.remove(thread_id);
            return Err(e);
        }
        Ok(rx)
    }

    pub async fn turn_interrupt(
        &self,
        thread_id: &str,
    ) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        let params = serde_json::json!({ "threadId": thread_id });
        self.request("turn/interrupt", Some(params)).await?;
        self.turn_senders.lock().await.remove(thread_id);
        self.thread_modes.lock().await.remove(thread_id);
        Ok(())
    }

    fn thread_id_from_result(
        result: &Value,
    ) -> Result<String, Box<dyn std::error::Error + Send + Sync>> {
        result["thread"]["id"]
            .as_str()
            .map(str::to_string)
            .ok_or_else(|| "codex response: missing thread.id".into())
    }

    async fn next_id(&self) -> u64 {
        let mut id = self.next_id.lock().await;
        let current = *id;
        *id += 1;
        current
    }

    async fn request(
        &self,
        method: &str,
        params: Option<Value>,
    ) -> Result<Value, Box<dyn std::error::Error + Send + Sync>> {
        let id = self.next_id().await;
        let (tx, rx) = oneshot::channel();
        self.pending.lock().await.insert(id, tx);

        let msg = Request { id, method, params };
        self.send_line(&serde_json::to_string(&msg)?).await?;

        rx.await
            .map_err(|_| "codex process closed unexpectedly".into())
            .and_then(|r| r.map_err(|e| e.into()))
    }

    async fn notify(
        &self,
        method: &str,
        params: Option<Value>,
    ) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        let msg = Notification { method, params };
        self.send_line(&serde_json::to_string(&msg)?).await
    }

    async fn send_line(&self, line: &str) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        let mut stdin = self.stdin.lock().await;
        stdin.write_all(line.as_bytes()).await?;
        stdin.write_all(b"\n").await?;
        stdin.flush().await?;
        debug!(line, "codex → sent");
        Ok(())
    }

    async fn read_loop(
        stdout: ChildStdout,
        stdin: Arc<Mutex<ChildStdin>>,
        pending: PendingMap,
        turn_senders: TurnSenders,
        thread_modes: ThreadModes,
        alive: Arc<AtomicBool>,
    ) {
        let mut reader = BufReader::new(stdout).lines();
        while let Ok(Some(line)) = reader.next_line().await {
            debug!(line = %line, "codex ← recv");
            let msg: IncomingMessage = match serde_json::from_str(&line) {
                Ok(m) => m,
                Err(e) => {
                    warn!(error = %e, line = %line, "codex: failed to parse message");
                    continue;
                }
            };

            if let Some(id) = &msg.id {
                if let Some(method) = &msg.method {
                    // Server→client request: must be answered or the turn deadlocks.
                    let id_val = Value::Number(id.clone());
                    if let Err(e) = Self::respond_server_request(
                        &stdin,
                        &id_val,
                        method,
                        msg.params.as_ref(),
                        &thread_modes,
                    )
                    .await
                    {
                        warn!(method = %method, error = %e, "codex: failed to reply to server request");
                    }
                    continue;
                }

                // Response to one of our outbound requests.
                let id_u64: u64 = match id.to_string().parse() {
                    Ok(n) => n,
                    Err(_) => {
                        warn!(id = %id, "codex: response has non-u64 id; ignoring");
                        continue;
                    }
                };
                let tx = pending.lock().await.remove(&id_u64);
                if let Some(tx) = tx {
                    let result = if let Some(err) = msg.error {
                        Err(err.to_string())
                    } else {
                        Ok(msg.result.unwrap_or(Value::Null))
                    };
                    let _ = tx.send(result);
                } else {
                    debug!(
                        id = id_u64,
                        "codex: response arrived for unknown/dropped request"
                    );
                }
            } else if let Some(method) = &msg.method
                && let Some((thread_id, event)) = Self::parse_event(method, msg.params.as_ref())
            {
                let is_terminal =
                    matches!(event, CodexEvent::TurnCompleted | CodexEvent::Error { .. });
                let mut senders = turn_senders.lock().await;
                if thread_id.is_empty() {
                    for tx in senders.values() {
                        let _ = tx.send(event.clone());
                    }
                    if is_terminal {
                        senders.clear();
                    }
                } else if let Some(tx) = senders.get(&thread_id) {
                    let _ = tx.send(event);
                    if is_terminal {
                        senders.remove(&thread_id);
                    }
                }
            }
        }

        alive.store(false, Ordering::Relaxed);
        let mut pending = pending.lock().await;
        for (_, tx) in pending.drain() {
            let _ = tx.send(Err("codex process terminated".to_string()));
        }
        drop(pending);
        let mut senders = turn_senders.lock().await;
        for tx in senders.values() {
            let _ = tx.send(CodexEvent::Error {
                message: "codex process terminated".to_string(),
            });
        }
        senders.clear();
        thread_modes.lock().await.clear();
    }

    /// Auto-reply decision for inbound approval/elicitation server requests.
    ///
    /// Phase 0.5: bridge to Trogon's permission UI is deferred — never leave a
    /// request unanswered. `default` mode also auto-approves for now (tracked).
    fn approval_decision_for_mode(mode: Option<&str>) -> &'static str {
        match mode {
            Some("bypassPermissions") | Some("acceptEdits") | Some("dontAsk") => "approved",
            // TODO(phase-7): route `default` through Trogon permission UI instead of auto-approve.
            Some("default") | None => "approved",
            _ => "approved",
        }
    }

    async fn respond_server_request(
        stdin: &Arc<Mutex<ChildStdin>>,
        id: &Value,
        method: &str,
        params: Option<&Value>,
        thread_modes: &ThreadModes,
    ) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        let thread_id = Self::thread_id_from_params(params);
        let mode_str = if thread_id.is_empty() {
            None
        } else {
            thread_modes.lock().await.get(&thread_id).cloned()
        };
        let mode = mode_str.as_deref();

        let result = match method {
            "execCommandApproval"
            | "applyPatchApproval"
            | "item/commandExecution/requestApproval"
            | "item/fileChange/requestApproval"
            | "item/permissions/requestApproval" => {
                let decision = Self::approval_decision_for_mode(mode);
                debug!(method = %method, thread_id = %thread_id, decision, "codex: auto-replying to approval request");
                serde_json::json!({ "decision": decision })
            }
            "item/tool/requestUserInput" => {
                debug!(method = %method, "codex: auto-replying to tool user-input request with empty answers");
                serde_json::json!({ "answers": {} })
            }
            "mcpServer/elicitation/request" => {
                debug!(method = %method, "codex: auto-accepting MCP elicitation request");
                serde_json::json!({ "action": "accept" })
            }
            other => {
                warn!(method = %other, "codex: replying to unknown server request with null result");
                Value::Null
            }
        };

        let resp = Response { id, result };
        let line = serde_json::to_string(&resp)?;
        let mut writer = stdin.lock().await;
        writer.write_all(line.as_bytes()).await?;
        writer.write_all(b"\n").await?;
        writer.flush().await?;
        debug!(line, "codex → sent server-request reply");
        Ok(())
    }

    fn thread_id_from_params(params: Option<&Value>) -> String {
        params
            .and_then(|p| p.get("threadId"))
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string()
    }

    fn parse_event(method: &str, params: Option<&Value>) -> Option<(String, CodexEvent)> {
        let thread_id = Self::thread_id_from_params(params);

        match method {
            "item/agentMessage/delta" => {
                let delta = params?.get("delta")?.as_str()?;
                if delta.is_empty() {
                    return None;
                }
                Some((thread_id, CodexEvent::TextDelta { text: delta.to_string() }))
            }
            "item/reasoning/textDelta" | "item/reasoning/summaryTextDelta" => {
                let delta = params?.get("delta")?.as_str()?;
                if delta.is_empty() {
                    return None;
                }
                Some((
                    thread_id,
                    CodexEvent::ReasoningDelta {
                        text: delta.to_string(),
                    },
                ))
            }
            "item/started" => {
                let item = params?.get("item")?;
                Self::parse_thread_item(item, true).map(|event| (thread_id, event))
            }
            "item/completed" => {
                let item = params?.get("item")?;
                let item_type = item.get("type")?.as_str()?;
                if item_type == "agentMessage" {
                    trace!(thread_id, "codex: ignoring agentMessage item/completed (streamed via delta)");
                    return None;
                }
                Self::parse_thread_item(item, false).map(|event| (thread_id, event))
            }
            "thread/tokenUsage/updated" => {
                let usage = params?.get("tokenUsage")?;
                let last = usage.get("last")?;
                let input = last.get("inputTokens")?.as_u64()?;
                let output = last.get("outputTokens")?.as_u64()?;
                let total = last.get("totalTokens")?.as_u64()?;
                let context_window = usage
                    .get("modelContextWindow")
                    .and_then(|v| v.as_u64());
                Some((
                    thread_id,
                    CodexEvent::Usage {
                        input,
                        output,
                        total,
                        context_window,
                    },
                ))
            }
            "turn/completed" => Some((thread_id, CodexEvent::TurnCompleted)),
            "error" => {
                let message = params
                    .and_then(|p| p.get("message"))
                    .and_then(|v| v.as_str())
                    .unwrap_or("unknown error")
                    .to_string();
                Some((thread_id, CodexEvent::Error { message }))
            }
            _ => None,
        }
    }

    fn parse_thread_item(item: &Value, started: bool) -> Option<CodexEvent> {
        let item_type = item.get("type")?.as_str()?;
        let id = item.get("id")?.as_str()?.to_string();

        match item_type {
            "commandExecution" if started => {
                let name = item
                    .get("command")
                    .and_then(|v| v.as_str())
                    .unwrap_or("command")
                    .to_string();
                let input = serde_json::json!({
                    "command": item.get("command").cloned().unwrap_or(Value::Null),
                    "cwd": item.get("cwd").cloned().unwrap_or(Value::Null),
                });
                Some(CodexEvent::ToolStarted { id, name, input })
            }
            "commandExecution" => {
                let output = item
                    .get("aggregatedOutput")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string();
                Some(CodexEvent::ToolCompleted { id, output })
            }
            "mcpToolCall" if started => {
                let name = item
                    .get("tool")
                    .and_then(|v| v.as_str())
                    .unwrap_or("mcp")
                    .to_string();
                let input = item
                    .get("arguments")
                    .cloned()
                    .unwrap_or(Value::Null);
                Some(CodexEvent::ToolStarted { id, name, input })
            }
            "mcpToolCall" => {
                let output = if let Some(result) = item.get("result") {
                    result.to_string()
                } else if let Some(err) = item.get("error").and_then(|e| e.get("message")) {
                    err.as_str().unwrap_or("").to_string()
                } else {
                    String::new()
                };
                Some(CodexEvent::ToolCompleted { id, output })
            }
            "fileChange" if started => {
                let input = item.get("changes").cloned().unwrap_or(Value::Null);
                Some(CodexEvent::ToolStarted {
                    id,
                    name: "fileChange".to_string(),
                    input,
                })
            }
            "fileChange" => {
                let status = item
                    .get("status")
                    .and_then(|v| v.as_str())
                    .unwrap_or("completed");
                Some(CodexEvent::ToolCompleted {
                    id,
                    output: status.to_string(),
                })
            }
            "dynamicToolCall" if started => {
                let name = item
                    .get("tool")
                    .and_then(|v| v.as_str())
                    .unwrap_or("dynamicTool")
                    .to_string();
                let input = item
                    .get("arguments")
                    .cloned()
                    .unwrap_or(Value::Null);
                Some(CodexEvent::ToolStarted { id, name, input })
            }
            "dynamicToolCall" => {
                let output = item
                    .get("contentItems")
                    .map(|v| v.to_string())
                    .unwrap_or_default();
                Some(CodexEvent::ToolCompleted { id, output })
            }
            _ => None,
        }
    }
}

// ── Trait implementations ─────────────────────────────────────────────────────

type DynError = Box<dyn std::error::Error + Send + Sync>;

#[async_trait(?Send)]
impl CodexProcessClient for CodexProcess {
    fn is_alive(&self) -> bool {
        self.alive.load(Ordering::Relaxed)
    }

    async fn thread_start(&self, cwd: &str) -> Result<String, DynError> {
        self.thread_start(cwd).await
    }

    async fn thread_resume(&self, thread_id: &str) -> Result<String, DynError> {
        self.thread_resume(thread_id).await
    }

    async fn thread_fork(&self, thread_id: &str) -> Result<String, DynError> {
        self.thread_fork(thread_id).await
    }

    async fn turn_start(
        &self,
        thread_id: &str,
        user_input: &str,
        model: Option<&str>,
        approval_policy: Option<&str>,
        permission_mode: Option<&str>,
    ) -> Result<broadcast::Receiver<CodexEvent>, DynError> {
        self.turn_start(
            thread_id,
            user_input,
            model,
            approval_policy,
            permission_mode,
        )
        .await
    }

    async fn turn_interrupt(&self, thread_id: &str) -> Result<(), DynError> {
        self.turn_interrupt(thread_id).await
    }
}

pub struct RealProcessSpawner;

#[async_trait(?Send)]
impl ProcessSpawner for RealProcessSpawner {
    type Process = CodexProcess;

    async fn spawn(&self) -> Result<CodexProcess, DynError> {
        CodexProcess::spawn().await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(method: &str, params: serde_json::Value) -> Option<(String, CodexEvent)> {
        CodexProcess::parse_event(method, Some(&params))
    }

    #[test]
    fn text_delta_from_agent_message_delta() {
        let params = serde_json::json!({
            "threadId": "t1",
            "turnId": "turn-1",
            "itemId": "item-1",
            "delta": "hello"
        });
        let (tid, event) = parse("item/agentMessage/delta", params).unwrap();
        assert_eq!(tid, "t1");
        assert!(matches!(event, CodexEvent::TextDelta { text } if text == "hello"));
    }

    #[test]
    fn reasoning_delta_from_text_delta() {
        let params = serde_json::json!({
            "threadId": "t2",
            "turnId": "turn-1",
            "itemId": "item-1",
            "contentIndex": 0,
            "delta": "thinking"
        });
        let (_, event) = parse("item/reasoning/textDelta", params).unwrap();
        assert!(matches!(event, CodexEvent::ReasoningDelta { text } if text == "thinking"));
    }

    #[test]
    fn empty_text_delta_returns_none() {
        let params = serde_json::json!({
            "threadId": "t3",
            "turnId": "turn-1",
            "itemId": "item-1",
            "delta": ""
        });
        assert!(parse("item/agentMessage/delta", params).is_none());
    }

    #[test]
    fn tool_started_from_item_started() {
        let params = serde_json::json!({
            "threadId": "t4",
            "turnId": "turn-1",
            "startedAtMs": 1,
            "item": {
                "type": "commandExecution",
                "id": "call-1",
                "command": "git status",
                "commandActions": [],
                "cwd": "/tmp",
                "status": "inProgress"
            }
        });
        let (tid, event) = parse("item/started", params).unwrap();
        assert_eq!(tid, "t4");
        match event {
            CodexEvent::ToolStarted { id, name, .. } => {
                assert_eq!(id, "call-1");
                assert_eq!(name, "git status");
            }
            _ => panic!("expected ToolStarted"),
        }
    }

    #[test]
    fn tool_completed_from_item_completed() {
        let params = serde_json::json!({
            "threadId": "t5",
            "turnId": "turn-1",
            "completedAtMs": 2,
            "item": {
                "type": "commandExecution",
                "id": "call-2",
                "command": "echo hi",
                "commandActions": [],
                "cwd": "/tmp",
                "status": "completed",
                "aggregatedOutput": "done"
            }
        });
        let (tid, event) = parse("item/completed", params).unwrap();
        assert_eq!(tid, "t5");
        assert!(
            matches!(event, CodexEvent::ToolCompleted { id, output } if id == "call-2" && output == "done")
        );
    }

    #[test]
    fn agent_message_completed_is_ignored() {
        let params = serde_json::json!({
            "threadId": "t6",
            "turnId": "turn-1",
            "completedAtMs": 2,
            "item": {
                "type": "agentMessage",
                "id": "msg-1",
                "text": "full text"
            }
        });
        assert!(parse("item/completed", params).is_none());
    }

    #[test]
    fn usage_from_token_usage_updated() {
        let params = serde_json::json!({
            "threadId": "t7",
            "turnId": "turn-1",
            "tokenUsage": {
                "last": {
                    "inputTokens": 10,
                    "outputTokens": 5,
                    "totalTokens": 15,
                    "cachedInputTokens": 0,
                    "reasoningOutputTokens": 0
                },
                "total": {
                    "inputTokens": 100,
                    "outputTokens": 50,
                    "totalTokens": 150,
                    "cachedInputTokens": 0,
                    "reasoningOutputTokens": 0
                },
                "modelContextWindow": 128000
            }
        });
        let (_, event) = parse("thread/tokenUsage/updated", params).unwrap();
        match event {
            CodexEvent::Usage {
                input,
                output,
                total,
                context_window,
            } => {
                assert_eq!(input, 10);
                assert_eq!(output, 5);
                assert_eq!(total, 15);
                assert_eq!(context_window, Some(128_000));
            }
            _ => panic!("expected Usage"),
        }
    }

    #[test]
    fn turn_completed() {
        let params = serde_json::json!({
            "threadId": "t8",
            "turn": { "id": "turn-1", "status": "completed", "items": [] }
        });
        let (tid, event) = parse("turn/completed", params).unwrap();
        assert_eq!(tid, "t8");
        assert!(matches!(event, CodexEvent::TurnCompleted));
    }

    #[test]
    fn error_with_thread_id() {
        let params = serde_json::json!({"threadId": "t9", "message": "oops"});
        let (tid, event) = parse("error", params).unwrap();
        assert_eq!(tid, "t9");
        assert!(matches!(event, CodexEvent::Error { message } if message == "oops"));
    }

    #[test]
    fn error_without_thread_id_broadcasts() {
        let params = serde_json::json!({"message": "fatal"});
        let (tid, event) = parse("error", params).unwrap();
        assert_eq!(tid, "");
        assert!(matches!(event, CodexEvent::Error { .. }));
    }

    #[test]
    fn unknown_method_returns_none() {
        let params = serde_json::json!({"threadId": "t10"});
        assert!(parse("item/updated", params).is_none());
    }

    #[test]
    fn mcp_tool_started_and_completed() {
        let started = serde_json::json!({
            "threadId": "t11",
            "turnId": "turn-1",
            "startedAtMs": 1,
            "item": {
                "type": "mcpToolCall",
                "id": "mcp-1",
                "server": "srv",
                "tool": "search",
                "arguments": {"q": "x"},
                "status": "inProgress"
            }
        });
        let (_, event) = parse("item/started", started).unwrap();
        match event {
            CodexEvent::ToolStarted { name, input, .. } => {
                assert_eq!(name, "search");
                assert_eq!(input, serde_json::json!({"q": "x"}));
            }
            _ => panic!("expected ToolStarted"),
        }

        let completed = serde_json::json!({
            "threadId": "t11",
            "turnId": "turn-1",
            "completedAtMs": 2,
            "item": {
                "type": "mcpToolCall",
                "id": "mcp-1",
                "server": "srv",
                "tool": "search",
                "arguments": {},
                "status": "completed",
                "result": {"content": [{"type": "text", "text": "ok"}]}
            }
        });
        let (_, event) = parse("item/completed", completed).unwrap();
        assert!(matches!(event, CodexEvent::ToolCompleted { .. }));
    }

    #[test]
    fn approval_decision_maps_modes() {
        assert_eq!(
            CodexProcess::approval_decision_for_mode(Some("acceptEdits")),
            "approved"
        );
        assert_eq!(
            CodexProcess::approval_decision_for_mode(Some("default")),
            "approved"
        );
    }

    #[tokio::test]
    async fn broadcast_channel_lag_allows_recovery() {
        use tokio::sync::broadcast;
        let (tx, mut rx) = broadcast::channel::<i32>(2);
        tx.send(1).unwrap();
        tx.send(2).unwrap();
        tx.send(3).unwrap();

        match rx.recv().await {
            Err(broadcast::error::RecvError::Lagged(_)) => {}
            other => panic!("expected Lagged, got {:?}", other),
        }
        let v = rx.recv().await.unwrap();
        assert_eq!(v, 2);
        let v = rx.recv().await.unwrap();
        assert_eq!(v, 3);
    }

    #[tokio::test]
    async fn broadcast_channel_closed_returns_closed_error() {
        use tokio::sync::broadcast;
        let (tx, mut rx) = broadcast::channel::<i32>(8);
        drop(tx);
        match rx.recv().await {
            Err(broadcast::error::RecvError::Closed) => {}
            other => panic!("expected Closed, got {:?}", other),
        }
    }

    #[test]
    fn turn_completed_with_no_params_returns_empty_thread_id() {
        let (tid, event) = CodexProcess::parse_event("turn/completed", None).unwrap();
        assert_eq!(tid, "");
        assert!(matches!(event, CodexEvent::TurnCompleted));
    }

    #[test]
    fn error_with_no_params_returns_default_message() {
        let (tid, event) = CodexProcess::parse_event("error", None).unwrap();
        assert_eq!(tid, "");
        assert!(matches!(event, CodexEvent::Error { message } if message == "unknown error"));
    }
}
