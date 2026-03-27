use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex as StdMutex;

use serde::{Deserialize, Serialize};
use serde_json::Value;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::{Child, ChildStdin, ChildStdout, Command};
use tokio::sync::{Mutex, broadcast, oneshot};
use tracing::{debug, trace, warn};

// ── JSON-RPC wire types ────────────────────────────────────────────────────────

#[derive(Serialize)]
struct Request<'a> {
    id: u64,
    method: &'a str,
    #[serde(skip_serializing_if = "Option::is_none")]
    params: Option<Value>,
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
    /// A text message chunk from the model.
    TextDelta { text: String },
    /// A tool/command execution started.
    ToolStarted { id: String, name: String, input: Value },
    /// A tool/command completed.
    ToolCompleted { id: String, output: String },
    /// The turn finished.
    TurnCompleted,
    /// An error occurred during the turn.
    Error { message: String },
}

// ── CodexProcess ──────────────────────────────────────────────────────────────

type PendingMap = Arc<Mutex<HashMap<u64, oneshot::Sender<Result<Value, String>>>>>;

/// Per-thread event channels. Keyed by Codex thread_id.
/// Each turn_start creates a new sender for that thread; the read_loop routes
/// events to the correct channel so concurrent sessions don't cross-contaminate.
type TurnSenders = Arc<Mutex<HashMap<String, broadcast::Sender<CodexEvent>>>>;

/// Manages a single `codex app-server` subprocess.
/// Speaks the Codex JSON-RPC protocol over stdin/stdout.
pub struct CodexProcess {
    stdin: Arc<Mutex<ChildStdin>>,
    next_id: Arc<Mutex<u64>>,
    pending: PendingMap,
    turn_senders: TurnSenders,
    alive: Arc<AtomicBool>,
    _child: StdMutex<Child>,
}

impl Drop for CodexProcess {
    fn drop(&mut self) {
        // Best-effort kill: start_kill sends SIGKILL without waiting. If the
        // child has already exited this is a harmless no-op.
        if let Ok(mut child) = self._child.lock() {
            let _ = child.start_kill();
        }
    }
}

impl CodexProcess {
    /// Returns `true` while the subprocess stdout is open.
    pub fn is_alive(&self) -> bool {
        self.alive.load(Ordering::Relaxed)
    }

    /// Spawn `codex app-server` and perform the initialization handshake.
    pub async fn spawn() -> Result<Self, Box<dyn std::error::Error + Send + Sync>> {
        let mut child = Command::new("codex")
            .arg("app-server")
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::inherit())
            .spawn()?;

        let stdin = child.stdin.take().expect("stdin piped");
        let stdout = child.stdout.take().expect("stdout piped");

        let pending: PendingMap = Arc::new(Mutex::new(HashMap::new()));
        let turn_senders: TurnSenders = Arc::new(Mutex::new(HashMap::new()));
        let alive = Arc::new(AtomicBool::new(true));

        let process = Self {
            stdin: Arc::new(Mutex::new(stdin)),
            next_id: Arc::new(Mutex::new(1)),
            pending: pending.clone(),
            turn_senders: turn_senders.clone(),
            alive: alive.clone(),
            _child: StdMutex::new(child),
        };

        // Background task: read stdout and route messages.
        tokio::task::spawn_local(Self::read_loop(stdout, pending, turn_senders, alive));

        // Handshake: initialize.
        process.initialize().await?;

        Ok(process)
    }

    /// Send `initialize` and wait for the response.
    async fn initialize(&self) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        let params = serde_json::json!({
            "clientInfo": {
                "name": "trogon-codex-runner",
                "version": env!("CARGO_PKG_VERSION")
            }
        });
        self.request("initialize", Some(params)).await?;
        // Send the `initialized` notification.
        self.notify("initialized", None).await?;
        Ok(())
    }

    /// Start a new thread (ACP session). Returns the Codex `threadId`.
    pub async fn thread_start(
        &self,
        working_directory: &str,
    ) -> Result<String, Box<dyn std::error::Error + Send + Sync>> {
        let params = serde_json::json!({ "workingDirectory": working_directory });
        let result = self.request("thread/start", Some(params)).await?;
        let thread_id = result["threadId"]
            .as_str()
            .ok_or("thread/start: missing threadId in response")?
            .to_string();
        Ok(thread_id)
    }

    /// Resume an existing thread. Returns the same `thread_id`.
    pub async fn thread_resume(
        &self,
        thread_id: &str,
    ) -> Result<String, Box<dyn std::error::Error + Send + Sync>> {
        let params = serde_json::json!({ "threadId": thread_id });
        self.request("thread/resume", Some(params)).await?;
        Ok(thread_id.to_string())
    }

    /// Fork a thread. Returns the new `threadId`.
    pub async fn thread_fork(
        &self,
        thread_id: &str,
    ) -> Result<String, Box<dyn std::error::Error + Send + Sync>> {
        let params = serde_json::json!({ "threadId": thread_id });
        let result = self.request("thread/fork", Some(params)).await?;
        let new_id = result["threadId"]
            .as_str()
            .ok_or("thread/fork: missing threadId in response")?
            .to_string();
        Ok(new_id)
    }

    /// Start a turn and return a receiver for streaming `CodexEvent`s.
    /// Events are scoped to this thread — other sessions' events are not visible.
    pub async fn turn_start(
        &self,
        thread_id: &str,
        user_input: &str,
        model: Option<&str>,
    ) -> Result<broadcast::Receiver<CodexEvent>, Box<dyn std::error::Error + Send + Sync>> {
        let (tx, rx) = broadcast::channel(256);
        self.turn_senders
            .lock()
            .await
            .insert(thread_id.to_string(), tx);

        let mut params = serde_json::json!({
            "threadId": thread_id,
            "userInput": user_input,
        });
        if let Some(m) = model {
            params["model"] = Value::String(m.to_string());
        }
        if let Err(e) = self.request("turn/start", Some(params)).await {
            // Request failed — remove the sender we just inserted so it doesn't
            // linger until the next turn_start call for this thread.
            self.turn_senders.lock().await.remove(thread_id);
            return Err(e);
        }
        Ok(rx)
    }

    /// Interrupt the current turn for a thread.
    pub async fn turn_interrupt(
        &self,
        thread_id: &str,
    ) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        let params = serde_json::json!({ "threadId": thread_id });
        self.request("turn/interrupt", Some(params)).await?;
        Ok(())
    }

    // ── internal ───────────────────────────────────────────────────────────────

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

    async fn send_line(
        &self,
        line: &str,
    ) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        let mut stdin = self.stdin.lock().await;
        stdin.write_all(line.as_bytes()).await?;
        stdin.write_all(b"\n").await?;
        stdin.flush().await?;
        debug!(line, "codex → sent");
        Ok(())
    }

    /// Background task: reads newline-delimited JSON from stdout and routes:
    /// - Responses (have `id`) → resolves the pending oneshot.
    /// - Notifications (have `method`, no `id`) → routed to the thread's event channel.
    ///
    /// When the subprocess exits, `alive` is set to false and all pending requests
    /// are resolved with an error so callers don't hang.
    async fn read_loop(
        stdout: ChildStdout,
        pending: PendingMap,
        turn_senders: TurnSenders,
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
                // It's a response to one of our requests.
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
                }
            } else if let Some(method) = &msg.method {
                // It's a server notification — route to the right thread's channel.
                if let Some((thread_id, event)) =
                    Self::parse_event(method, msg.params.as_ref())
                {
                    let is_terminal =
                        matches!(event, CodexEvent::TurnCompleted | CodexEvent::Error { .. });
                    let mut senders = turn_senders.lock().await;
                    if thread_id.is_empty() {
                        // No thread_id in params — broadcast to all active turns.
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
        }

        // Process stdout closed — mark dead, unblock pending requests, and signal
        // all active turns so prompt loops don't hang until their timeout fires.
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
    }

    fn parse_event(method: &str, params: Option<&Value>) -> Option<(String, CodexEvent)> {
        // thread_id may be absent for broadcast-style error notifications.
        let thread_id = params
            .and_then(|p| p.get("threadId"))
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();

        match method {
            "item/updated" | "item/completed" => {
                let item = params?.get("item")?;
                let item_type = item.get("type")?.as_str()?;
                let event = match item_type {
                    "message" => {
                        let content = item.get("content")?.as_array()?;
                        let mut text = String::new();
                        for block in content {
                            if block.get("type").and_then(|v| v.as_str()) == Some("output_text")
                                && let Some(t) = block.get("text").and_then(|v| v.as_str())
                            {
                                text.push_str(t);
                            }
                        }
                        if text.is_empty() {
                            trace!(thread_id, "codex: message item had no output_text blocks; skipping");
                            return None;
                        }
                        CodexEvent::TextDelta { text }
                    }
                    "tool_call" | "function_call" | "command_execution" => {
                        let id = item.get("id")?.as_str()?.to_string();
                        if method == "item/updated" {
                            let name = item
                                .get("name")
                                .or_else(|| item.get("command"))
                                .and_then(|v| v.as_str())
                                .unwrap_or("unknown")
                                .to_string();
                            let input = item
                                .get("arguments")
                                .or_else(|| item.get("input"))
                                .cloned()
                                .unwrap_or(Value::Null);
                            CodexEvent::ToolStarted { id, name, input }
                        } else {
                            let output = item
                                .get("output")
                                .and_then(|v| v.as_str())
                                .unwrap_or("")
                                .to_string();
                            CodexEvent::ToolCompleted { id, output }
                        }
                    }
                    _ => return None,
                };
                Some((thread_id, event))
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
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(method: &str, params: serde_json::Value) -> Option<(String, CodexEvent)> {
        CodexProcess::parse_event(method, Some(&params))
    }

    #[test]
    fn text_delta_from_item_updated() {
        let params = serde_json::json!({
            "threadId": "t1",
            "item": {
                "type": "message",
                "content": [{"type": "output_text", "text": "hello"}]
            }
        });
        let (tid, event) = parse("item/updated", params).unwrap();
        assert_eq!(tid, "t1");
        assert!(matches!(event, CodexEvent::TextDelta { text } if text == "hello"));
    }

    #[test]
    fn text_delta_concatenates_blocks() {
        let params = serde_json::json!({
            "threadId": "t2",
            "item": {
                "type": "message",
                "content": [
                    {"type": "output_text", "text": "foo"},
                    {"type": "output_text", "text": "bar"}
                ]
            }
        });
        let (_, event) = parse("item/updated", params).unwrap();
        assert!(matches!(event, CodexEvent::TextDelta { text } if text == "foobar"));
    }

    #[test]
    fn empty_text_returns_none() {
        let params = serde_json::json!({
            "threadId": "t3",
            "item": {
                "type": "message",
                "content": [{"type": "other_type", "text": "ignored"}]
            }
        });
        assert!(parse("item/updated", params).is_none());
    }

    #[test]
    fn tool_started_from_item_updated() {
        let params = serde_json::json!({
            "threadId": "t4",
            "item": {
                "type": "tool_call",
                "id": "call-1",
                "name": "bash",
                "arguments": {"cmd": "ls"}
            }
        });
        let (tid, event) = parse("item/updated", params).unwrap();
        assert_eq!(tid, "t4");
        match event {
            CodexEvent::ToolStarted { id, name, input } => {
                assert_eq!(id, "call-1");
                assert_eq!(name, "bash");
                assert_eq!(input, serde_json::json!({"cmd": "ls"}));
            }
            _ => panic!("expected ToolStarted"),
        }
    }

    #[test]
    fn tool_completed_from_item_completed() {
        let params = serde_json::json!({
            "threadId": "t5",
            "item": {
                "type": "tool_call",
                "id": "call-2",
                "output": "done"
            }
        });
        let (tid, event) = parse("item/completed", params).unwrap();
        assert_eq!(tid, "t5");
        assert!(matches!(event, CodexEvent::ToolCompleted { id, output } if id == "call-2" && output == "done"));
    }

    #[test]
    fn turn_completed() {
        let params = serde_json::json!({"threadId": "t6"});
        let (tid, event) = parse("turn/completed", params).unwrap();
        assert_eq!(tid, "t6");
        assert!(matches!(event, CodexEvent::TurnCompleted));
    }

    #[test]
    fn error_with_thread_id() {
        let params = serde_json::json!({"threadId": "t7", "message": "oops"});
        let (tid, event) = parse("error", params).unwrap();
        assert_eq!(tid, "t7");
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
        let params = serde_json::json!({"threadId": "t8"});
        assert!(parse("unknown/method", params).is_none());
    }

    #[test]
    fn missing_thread_id_defaults_to_empty() {
        let params = serde_json::json!({"threadId": null});
        let (tid, _) = parse("turn/completed", params).unwrap();
        assert_eq!(tid, "");
    }

    #[test]
    fn command_execution_tool_started() {
        let params = serde_json::json!({
            "threadId": "t9",
            "item": {
                "type": "command_execution",
                "id": "exec-1",
                "command": "git status",
                "input": {"cwd": "/tmp"}
            }
        });
        let (_, event) = parse("item/updated", params).unwrap();
        assert!(matches!(event, CodexEvent::ToolStarted { name, .. } if name == "git status"));
    }

    #[test]
    fn error_without_message_uses_default() {
        let params = serde_json::json!({"threadId": "t10"});
        let (_, event) = parse("error", params).unwrap();
        assert!(matches!(event, CodexEvent::Error { message } if message == "unknown error"));
    }
}
