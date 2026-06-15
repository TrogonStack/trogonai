//! Lifecycle hooks (Claude Code-style), configured under `settings.json` `hooks`.
//!
//! Each command runs via `sh -c`, receives the event payload as JSON on stdin,
//! and signals its decision by exit code (2 = block) or a JSON stdout
//! `{"decision":"block","reason":"…"}`. For `UserPromptSubmit`, a hook's stdout
//! (when it doesn't block) is added as extra context.
//!
//! Lives in `trogon-runner-tools` so both the CLI (UserPromptSubmit/Stop/
//! Notification) and the runner (PreToolUse/PostToolUse) share one engine. The
//! config structs derive `Serialize` so the runner can persist the tool-event
//! matchers in `SessionState` (sent from the CLI via session meta).

use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::time::Duration;

const DEFAULT_HOOK_TIMEOUT_SECS: u64 = 30;

fn default_hook_type() -> String {
    "command".to_string()
}

/// All configured hooks, keyed by event (Claude Code event names).
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct HooksConfig {
    #[serde(default, rename = "PreToolUse", skip_serializing_if = "Vec::is_empty")]
    pub pre_tool_use: Vec<HookMatcher>,
    #[serde(default, rename = "PostToolUse", skip_serializing_if = "Vec::is_empty")]
    pub post_tool_use: Vec<HookMatcher>,
    /// Fires once after all tool calls in a single assistant turn complete.
    #[serde(default, rename = "PostToolBatch", skip_serializing_if = "Vec::is_empty")]
    pub post_tool_batch: Vec<HookMatcher>,
    #[serde(default, rename = "Stop", skip_serializing_if = "Vec::is_empty")]
    pub stop: Vec<HookMatcher>,
    #[serde(default, rename = "Notification", skip_serializing_if = "Vec::is_empty")]
    pub notification: Vec<HookMatcher>,
    #[serde(default, rename = "UserPromptSubmit", skip_serializing_if = "Vec::is_empty")]
    pub user_prompt_submit: Vec<HookMatcher>,
    /// Fires when a new session starts (CLI startup / runner session create).
    #[serde(default, rename = "SessionStart", skip_serializing_if = "Vec::is_empty")]
    pub session_start: Vec<HookMatcher>,
    /// Fires before the context compactor runs.
    #[serde(default, rename = "PreCompact", skip_serializing_if = "Vec::is_empty")]
    pub pre_compact: Vec<HookMatcher>,
    /// Fires when a spawned sub-agent finishes its session.
    #[serde(default, rename = "SubagentStop", skip_serializing_if = "Vec::is_empty")]
    pub subagent_stop: Vec<HookMatcher>,
}

impl HooksConfig {
    /// Merge `other` (higher precedence) by appending its matchers per event.
    pub fn merge(&mut self, other: HooksConfig) {
        self.pre_tool_use.extend(other.pre_tool_use);
        self.post_tool_use.extend(other.post_tool_use);
        self.post_tool_batch.extend(other.post_tool_batch);
        self.stop.extend(other.stop);
        self.notification.extend(other.notification);
        self.user_prompt_submit.extend(other.user_prompt_submit);
        self.session_start.extend(other.session_start);
        self.pre_compact.extend(other.pre_compact);
        self.subagent_stop.extend(other.subagent_stop);
    }

    /// True when no hooks are configured for any event.
    pub fn is_empty(&self) -> bool {
        self.pre_tool_use.is_empty()
            && self.post_tool_use.is_empty()
            && self.post_tool_batch.is_empty()
            && self.stop.is_empty()
            && self.notification.is_empty()
            && self.user_prompt_submit.is_empty()
            && self.session_start.is_empty()
            && self.pre_compact.is_empty()
            && self.subagent_stop.is_empty()
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct HookMatcher {
    /// Tool-name pattern (tool events only). Empty or `*` = match all. Supports
    /// `A|B` alternatives. Ignored for non-tool events.
    #[serde(default)]
    pub matcher: String,
    #[serde(default)]
    pub hooks: Vec<HookCommand>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct HookCommand {
    #[serde(rename = "type", default = "default_hook_type")]
    pub r#type: String,
    #[serde(default)]
    pub command: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub timeout: Option<u64>,
}

/// Result of running all hooks for an event.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HookOutcome {
    /// Proceed; `context` (concatenated non-blocking stdout) may augment the action.
    Continue { context: Option<String> },
    /// Block the action with a reason.
    Block { reason: String },
}

/// Does `matcher` apply to `tool_name`? Non-tool events (`tool_name == None`)
/// always match. `""`/`"*"` match all; `A|B` matches any listed tool.
pub fn matcher_matches(matcher: &str, tool_name: Option<&str>) -> bool {
    let Some(tool) = tool_name else {
        return true;
    };
    let m = matcher.trim();
    if m.is_empty() || m == "*" {
        return true;
    }
    m.split('|').any(|alt| alt.trim() == tool)
}

fn decide(exit_code: Option<i32>, stdout: &str, stderr: &str) -> HookCmdResult {
    if let Ok(Value::Object(obj)) = serde_json::from_str::<Value>(stdout.trim()) {
        let blocked = obj.get("decision").and_then(|v| v.as_str()) == Some("block")
            || obj.get("continue").and_then(|v| v.as_bool()) == Some(false);
        if blocked {
            let reason = obj
                .get("reason")
                .or_else(|| obj.get("stopReason"))
                .and_then(|v| v.as_str())
                .unwrap_or("blocked by hook")
                .to_string();
            return HookCmdResult::Block(reason);
        }
    }
    match exit_code {
        Some(0) => HookCmdResult::Continue(stdout.to_string()),
        Some(2) => HookCmdResult::Block(if stderr.trim().is_empty() {
            "blocked by hook".to_string()
        } else {
            stderr.trim().to_string()
        }),
        _ => HookCmdResult::Error(if stderr.trim().is_empty() {
            format!("hook exited with {exit_code:?}")
        } else {
            stderr.trim().to_string()
        }),
    }
}

enum HookCmdResult {
    Continue(String),
    Block(String),
    Error(String),
}

async fn run_one(cmd: &HookCommand, stdin_json: &str) -> HookCmdResult {
    use tokio::io::AsyncWriteExt;
    use tokio::process::Command;

    let mut child = match Command::new("sh")
        .arg("-c")
        .arg(&cmd.command)
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
    {
        Ok(c) => c,
        Err(e) => return HookCmdResult::Error(format!("could not spawn hook: {e}")),
    };
    let timeout = Duration::from_secs(cmd.timeout.unwrap_or(DEFAULT_HOOK_TIMEOUT_SECS));
    let stdin_json = stdin_json.to_owned();
    let output = if let Some(mut stdin) = child.stdin.take() {
        match tokio::time::timeout(timeout, async {
            let write = async {
                stdin
                    .write_all(stdin_json.as_bytes())
                    .await
                    .map_err(|e| format!("hook stdin write error: {e}"))?;
                drop(stdin);
                Ok(())
            };
            let (write_res, output_res) = tokio::join!(write, child.wait_with_output());
            write_res?;
            output_res.map_err(|e| format!("hook io error: {e}"))
        })
        .await
        {
            Ok(Ok(output)) => output,
            Ok(Err(msg)) => return HookCmdResult::Error(msg),
            Err(_) => {
                return HookCmdResult::Error(format!(
                    "hook timed out after {}s",
                    timeout.as_secs()
                ));
            }
        }
    } else {
        match tokio::time::timeout(timeout, child.wait_with_output()).await {
            Ok(Ok(o)) => o,
            Ok(Err(e)) => return HookCmdResult::Error(format!("hook io error: {e}")),
            Err(_) => {
                return HookCmdResult::Error(format!(
                    "hook timed out after {}s",
                    timeout.as_secs()
                ));
            }
        }
    };
    decide(
        output.status.code(),
        &String::from_utf8_lossy(&output.stdout),
        &String::from_utf8_lossy(&output.stderr),
    )
}

/// Run all matching hooks for an event. `tool_name` is `Some` for tool events.
/// Returns the first `Block`, else `Continue` with concatenated non-blocking
/// stdout as context.
pub async fn run_event_hooks(
    matchers: &[HookMatcher],
    tool_name: Option<&str>,
    payload: &Value,
) -> HookOutcome {
    let stdin_json = serde_json::to_string(payload).unwrap_or_default();
    let mut context = String::new();
    for m in matchers {
        if !matcher_matches(&m.matcher, tool_name) {
            continue;
        }
        for cmd in &m.hooks {
            if cmd.r#type != "command" || cmd.command.trim().is_empty() {
                continue;
            }
            match run_one(cmd, &stdin_json).await {
                HookCmdResult::Block(reason) => return HookOutcome::Block { reason },
                HookCmdResult::Continue(out) => {
                    let out = out.trim();
                    if !out.is_empty() {
                        if !context.is_empty() {
                            context.push('\n');
                        }
                        context.push_str(out);
                    }
                }
                HookCmdResult::Error(e) => eprintln!("warning: hook error: {e}"),
            }
        }
    }
    if context.is_empty() {
        HookOutcome::Continue { context: None }
    } else {
        HookOutcome::Continue { context: Some(context) }
    }
}

/// A [`trogon_tools::PostToolObserver`] backed by `PostToolUse` hook matchers.
///
/// After each tool runs, the matching hooks are invoked with the standard
/// `PostToolUse` payload. A blocking hook's reason is returned so the agent loop
/// can append it to the tool result the model sees (the tool already ran).
pub struct HookPostToolObserver {
    matchers: Vec<HookMatcher>,
}

impl HookPostToolObserver {
    pub fn new(matchers: Vec<HookMatcher>) -> Self {
        Self { matchers }
    }
}

impl trogon_tools::PostToolObserver for HookPostToolObserver {
    fn observe<'a>(
        &'a self,
        _tool_call_id: &'a str,
        tool_name: &'a str,
        tool_output: &'a str,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Option<String>> + Send + 'a>> {
        Box::pin(async move {
            let payload = serde_json::json!({
                "hook_event_name": "PostToolUse",
                "tool_name": tool_name,
                "tool_response": tool_output,
            });
            match run_event_hooks(&self.matchers, Some(tool_name), &payload).await {
                HookOutcome::Block { reason } => Some(reason),
                HookOutcome::Continue { .. } => None,
            }
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn matcher_matches_rules() {
        assert!(matcher_matches("", Some("Bash")));
        assert!(matcher_matches("*", Some("Bash")));
        assert!(matcher_matches("Bash", Some("Bash")));
        assert!(!matcher_matches("Bash", Some("Read")));
        assert!(matcher_matches("Bash|Read", Some("Read")));
        assert!(matcher_matches("Bash", None));
    }

    #[test]
    fn decide_from_exit_and_json() {
        assert!(matches!(decide(Some(0), "", ""), HookCmdResult::Continue(_)));
        assert!(matches!(decide(Some(2), "", "nope"), HookCmdResult::Block(r) if r == "nope"));
        assert!(matches!(decide(Some(1), "", "boom"), HookCmdResult::Error(_)));
        match decide(Some(0), r#"{"decision":"block","reason":"policy"}"#, "") {
            HookCmdResult::Block(r) => assert_eq!(r, "policy"),
            _ => panic!("expected block"),
        }
        match decide(Some(0), "extra context", "") {
            HookCmdResult::Continue(c) => assert_eq!(c, "extra context"),
            _ => panic!("expected continue"),
        }
    }

    #[test]
    fn config_serde_round_trips_claude_format() {
        let raw = r#"{"PreToolUse":[{"matcher":"Bash","hooks":[{"type":"command","command":"./guard.sh"}]}]}"#;
        let cfg: HooksConfig = serde_json::from_str(raw).unwrap();
        assert_eq!(cfg.pre_tool_use.len(), 1);
        assert_eq!(cfg.pre_tool_use[0].matcher, "Bash");
        // Round-trips (Serialize) for persistence in SessionState.
        let back: HooksConfig = serde_json::from_str(&serde_json::to_string(&cfg).unwrap()).unwrap();
        assert_eq!(back, cfg);
    }

    #[test]
    fn config_serde_round_trips_new_events() {
        let raw = r#"{
            "SessionStart":[{"hooks":[{"type":"command","command":"./start.sh"}]}],
            "PreCompact":[{"hooks":[{"type":"command","command":"./pre.sh"}]}],
            "SubagentStop":[{"hooks":[{"type":"command","command":"./sub.sh"}]}],
            "PostToolBatch":[{"hooks":[{"type":"command","command":"./batch.sh"}]}]
        }"#;
        let cfg: HooksConfig = serde_json::from_str(raw).unwrap();
        assert_eq!(cfg.session_start.len(), 1);
        assert_eq!(cfg.pre_compact.len(), 1);
        assert_eq!(cfg.subagent_stop.len(), 1);
        assert_eq!(cfg.post_tool_batch.len(), 1);
        assert!(!cfg.is_empty());
        let back: HooksConfig = serde_json::from_str(&serde_json::to_string(&cfg).unwrap()).unwrap();
        assert_eq!(back, cfg);
    }

    #[test]
    fn merge_and_is_empty_cover_new_events() {
        let mut a = HooksConfig::default();
        assert!(a.is_empty());
        let mut b = HooksConfig::default();
        b.subagent_stop.push(HookMatcher {
            matcher: String::new(),
            hooks: vec![HookCommand {
                r#type: "command".into(),
                command: "echo hi".into(),
                timeout: None,
            }],
        });
        a.merge(b);
        assert!(!a.is_empty());
        assert_eq!(a.subagent_stop.len(), 1);
    }

    #[tokio::test]
    async fn run_event_hooks_blocks_on_exit_2() {
        let matchers = vec![HookMatcher {
            matcher: "Bash".into(),
            hooks: vec![HookCommand {
                r#type: "command".into(),
                command: "echo denied >&2; exit 2".into(),
                timeout: None,
            }],
        }];
        let out = run_event_hooks(&matchers, Some("Bash"), &json!({"tool_name": "Bash"})).await;
        assert_eq!(out, HookOutcome::Block { reason: "denied".into() });
    }

    #[tokio::test]
    async fn run_event_hooks_collects_context_and_passes_stdin() {
        let matchers = vec![HookMatcher {
            matcher: String::new(),
            hooks: vec![HookCommand {
                r#type: "command".into(),
                command: "cat | sed -n 's/.*\"prompt\":\"\\([^\"]*\\)\".*/got:\\1/p'".into(),
                timeout: None,
            }],
        }];
        let out = run_event_hooks(&matchers, None, &json!({"prompt": "hello"})).await;
        match out {
            HookOutcome::Continue { context: Some(c) } => assert!(c.contains("got:hello")),
            other => panic!("expected context, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn post_tool_observer_returns_block_reason() {
        use trogon_tools::PostToolObserver;
        let obs = HookPostToolObserver::new(vec![HookMatcher {
            matcher: "Bash".into(),
            hooks: vec![HookCommand {
                r#type: "command".into(),
                command: "echo nope >&2; exit 2".into(),
                timeout: None,
            }],
        }]);
        assert_eq!(obs.observe("id1", "Bash", "output").await, Some("nope".to_string()));
        // Non-matching tool → no objection.
        assert_eq!(obs.observe("id2", "Read", "output").await, None);
    }

    #[tokio::test]
    async fn non_matching_tool_hook_is_skipped() {
        let matchers = vec![HookMatcher {
            matcher: "Bash".into(),
            hooks: vec![HookCommand {
                r#type: "command".into(),
                command: "exit 2".into(),
                timeout: None,
            }],
        }];
        let out = run_event_hooks(&matchers, Some("Read"), &json!({})).await;
        assert_eq!(out, HookOutcome::Continue { context: None });
    }
}
