use agent_client_protocol::{
    ContentBlock, NewSessionRequest, PromptRequest, SessionNotification, SessionUpdate, TextContent,
};
use async_nats::Client;
use bytes::Bytes;
use futures::StreamExt;
use serde_json::Value;
use std::path::PathBuf;
use std::time::Duration;
use tokio::sync::mpsc;

const SESSION_NEW_TIMEOUT: Duration = Duration::from_secs(15);

pub struct TrogonSession {
    nats: Client,
    pub session_id: String,
    prefix: String,
}

impl TrogonSession {
    /// Create a new ACP session via NATS. `cwd` is the working directory for the session.
    pub async fn new(nats: Client, prefix: &str, cwd: PathBuf) -> anyhow::Result<Self> {
        let subject = format!("{prefix}.agent.session.new");
        let req = NewSessionRequest::new(cwd);
        let payload = serde_json::to_vec(&req)?;

        let reply = tokio::time::timeout(
            SESSION_NEW_TIMEOUT,
            nats.request(subject, payload.into()),
        )
        .await
        .map_err(|_| anyhow::anyhow!("timed out waiting for session creation (is trogon-acp-runner running?)"))?
        .map_err(|e| anyhow::anyhow!("NATS error creating session: {e}"))?;

        let resp: Value = serde_json::from_slice(&reply.payload)
            .map_err(|e| anyhow::anyhow!("invalid session response: {e}"))?;

        let session_id = resp["sessionId"]
            .as_str()
            .ok_or_else(|| anyhow::anyhow!("session response missing sessionId: {resp}"))?
            .to_string();

        Ok(Self {
            nats,
            session_id,
            prefix: prefix.to_string(),
        })
    }

    /// Send a prompt and stream events via the returned channel.
    /// The channel closes when the turn is done.
    pub async fn prompt(&self, text: &str) -> anyhow::Result<mpsc::Receiver<StreamEvent>> {
        let notif_subject = format!(
            "{}.session.{}.client.session.update",
            self.prefix, self.session_id
        );
        let prompt_subject = format!(
            "{}.session.{}.agent.prompt",
            self.prefix, self.session_id
        );

        let mut notif_sub = self
            .nats
            .subscribe(notif_subject)
            .await
            .map_err(|e| anyhow::anyhow!("subscribe notifications: {e}"))?;

        let inbox = self.nats.new_inbox();
        let mut resp_sub = self
            .nats
            .subscribe(inbox.clone())
            .await
            .map_err(|e| anyhow::anyhow!("subscribe inbox: {e}"))?;

        let req = PromptRequest::new(
            self.session_id.clone(),
            vec![ContentBlock::Text(TextContent::new(text))],
        );
        let payload = serde_json::to_vec(&req)?;

        self.nats
            .publish_with_reply(prompt_subject, inbox, payload.into())
            .await
            .map_err(|e| anyhow::anyhow!("publish prompt: {e}"))?;

        let (tx, rx) = mpsc::channel(64);

        tokio::spawn(async move {
            loop {
                tokio::select! {
                    biased;
                    msg = resp_sub.next() => {
                        let Some(msg) = msg else { break };
                        if let Ok(v) = serde_json::from_slice::<Value>(&msg.payload) {
                            let stop = v.get("stopReason")
                                .and_then(|s| s.as_str())
                                .unwrap_or("end_turn")
                                .to_string();
                            let _ = tx.send(StreamEvent::Done(stop)).await;
                        } else {
                            let _ = tx.send(StreamEvent::Done("end_turn".into())).await;
                        }
                        break;
                    }
                    msg = notif_sub.next() => {
                        let Some(msg) = msg else { break };
                        if let Ok(notif) = serde_json::from_slice::<SessionNotification>(&msg.payload) {
                            match notif.update {
                                SessionUpdate::AgentMessageChunk(chunk) => {
                                    if let ContentBlock::Text(t) = chunk.content {
                                        let _ = tx.send(StreamEvent::Text(t.text)).await;
                                    }
                                }
                                SessionUpdate::AgentThoughtChunk(_chunk) => {
                                    let _ = tx.send(StreamEvent::Thinking).await;
                                }
                                SessionUpdate::ToolCall(tc) => {
                                    if let Some(diff) = render_diff(&tc.title, tc.raw_input.as_ref()) {
                                        let _ = tx.send(StreamEvent::ToolCall(tc.title)).await;
                                        let _ = tx.send(StreamEvent::Diff(diff)).await;
                                    } else {
                                        let _ = tx.send(StreamEvent::ToolCall(tc.title)).await;
                                    }
                                }
                                SessionUpdate::UsageUpdate(u) => {
                                    let _ = tx.send(StreamEvent::Usage {
                                        used_tokens: u.used,
                                        context_size: u.size,
                                    }).await;
                                }
                                _ => {}
                            }
                        }
                    }
                }
            }
        });

        Ok(rx)
    }

    /// Fire-and-forget cancel for the active prompt turn.
    pub async fn cancel(&self) {
        let subject = format!(
            "{}.session.{}.agent.cancel",
            self.prefix, self.session_id
        );
        let _ = self.nats.publish(subject, Bytes::new()).await;
    }
}

/// Events produced during a prompt turn.
#[derive(Debug)]
pub enum StreamEvent {
    Text(String),
    Thinking,
    ToolCall(String),
    /// Pre-rendered colored diff for Edit/MultiEdit/Write tool calls.
    Diff(String),
    /// Token usage update at the end of a turn.
    Usage { used_tokens: u64, context_size: u64 },
    Done(String),
}

// ── Diff rendering ────────────────────────────────────────────────────────────

const RED: &str = "\x1b[31m";
const GREEN: &str = "\x1b[32m";
const BOLD: &str = "\x1b[1m";
const RESET: &str = "\x1b[0m";
const DIM: &str = "\x1b[2m";

/// Render a colored diff for Edit/MultiEdit/Write tool calls.
/// Returns `None` for tools that don't produce a diff.
fn render_diff(tool_name: &str, input: Option<&serde_json::Value>) -> Option<String> {
    let input = input?;
    match tool_name {
        "Edit" => {
            let path = input.get("file_path")?.as_str()?;
            let old = input.get("old_string").and_then(|v| v.as_str()).unwrap_or("");
            let new = input.get("new_string").and_then(|v| v.as_str())?;
            Some(format_edit_diff(path, old, new))
        }
        "MultiEdit" => {
            let path = input.get("file_path")?.as_str()?;
            let edits = input.get("edits")?.as_array()?;
            let mut out = format!("{DIM}--- {path}{RESET}\n{DIM}+++ {path}{RESET}");
            for edit in edits {
                let old = edit.get("old_string").and_then(|v| v.as_str()).unwrap_or("");
                let new = edit.get("new_string").and_then(|v| v.as_str()).unwrap_or("");
                out.push('\n');
                out.push_str(&diff_lines(old, new));
            }
            Some(out)
        }
        "Write" => {
            let path = input.get("file_path")?.as_str()?;
            let content = input.get("content").and_then(|v| v.as_str()).unwrap_or("");
            let lines = content.lines().count();
            Some(format!("{BOLD}[write: {path}]{RESET} {DIM}({lines} lines){RESET}"))
        }
        _ => None,
    }
}

fn format_edit_diff(path: &str, old: &str, new: &str) -> String {
    let mut out = format!("{DIM}--- {path}{RESET}\n{DIM}+++ {path}{RESET}\n");
    out.push_str(&diff_lines(old, new));
    out
}

fn diff_lines(old: &str, new: &str) -> String {
    let mut out = String::new();
    for line in old.lines() {
        out.push_str(&format!("{RED}-{line}{RESET}\n"));
    }
    for line in new.lines() {
        out.push_str(&format!("{GREEN}+{line}{RESET}\n"));
    }
    out
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn render_diff_edit_returns_colored_diff() {
        let input = json!({"file_path": "src/main.rs", "old_string": "hello", "new_string": "world"});
        let diff = render_diff("Edit", Some(&input)).unwrap();
        assert!(diff.contains("src/main.rs"), "path in header");
        assert!(diff.contains("-hello"), "old line with minus");
        assert!(diff.contains("+world"), "new line with plus");
    }

    #[test]
    fn render_diff_edit_empty_old_string() {
        let input = json!({"file_path": "new.rs", "old_string": "", "new_string": "fn main() {}"});
        let diff = render_diff("Edit", Some(&input)).unwrap();
        assert!(diff.contains("+fn main() {}"));
        // No removal lines (lines starting with RED + "-"), only the "---" header.
        assert!(!diff.contains(&format!("{RED}-")), "no removal lines for empty old_string");
    }

    #[test]
    fn render_diff_multiedit_all_edits_shown() {
        let input = json!({
            "file_path": "lib.rs",
            "edits": [
                {"old_string": "foo", "new_string": "bar"},
                {"old_string": "baz", "new_string": "qux"},
            ]
        });
        let diff = render_diff("MultiEdit", Some(&input)).unwrap();
        assert!(diff.contains("-foo") && diff.contains("+bar"));
        assert!(diff.contains("-baz") && diff.contains("+qux"));
    }

    #[test]
    fn render_diff_write_shows_path_and_line_count() {
        let content = "line1\nline2\nline3";
        let input = json!({"file_path": "out.txt", "content": content});
        let diff = render_diff("Write", Some(&input)).unwrap();
        assert!(diff.contains("out.txt"));
        assert!(diff.contains("3 lines"));
    }

    #[test]
    fn render_diff_unknown_tool_returns_none() {
        let input = json!({"file_path": "x.rs"});
        assert!(render_diff("Bash", Some(&input)).is_none());
        assert!(render_diff("Read", Some(&input)).is_none());
    }

    #[test]
    fn render_diff_none_input_returns_none() {
        assert!(render_diff("Edit", None).is_none());
    }
}
