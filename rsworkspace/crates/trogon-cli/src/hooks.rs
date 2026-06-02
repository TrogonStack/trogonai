//! Lifecycle hooks (Claude Code-style) configured under `settings.json` `hooks`.
//!
//! ```json
//! "hooks": {
//!   "UserPromptSubmit": [ { "hooks": [ { "type": "command", "command": "./check.sh" } ] } ],
//!   "PreToolUse":  [ { "matcher": "Bash", "hooks": [ { "type": "command", "command": "..." } ] } ]
//! }
//! ```
//! Each command runs via `sh -c`, receives the event payload as JSON on stdin,
//! and signals its decision by exit code (2 = block) or a JSON stdout
//! `{"decision":"block","reason":"…"}`. For `UserPromptSubmit`, a hook's stdout
//! (when it doesn't block) is added to the prompt as extra context.
//!
//! This module is the execution engine + config schema. CLI-side events
//! (`UserPromptSubmit`, `Stop`, `Notification`) are wired in the REPL. The
//! runner-side tool events (`PreToolUse`, `PostToolUse`) reuse this engine.

use serde::Deserialize;
use serde_json::Value;
use std::time::Duration;

const DEFAULT_HOOK_TIMEOUT_SECS: u64 = 30;

fn default_hook_type() -> String {
    "command".to_string()
}

/// All configured hooks, keyed by event (Claude Code event names).
#[derive(Debug, Clone, Default, Deserialize, PartialEq, Eq)]
pub struct HooksConfig {
    #[serde(default, rename = "PreToolUse")]
    pub pre_tool_use: Vec<HookMatcher>,
    #[serde(default, rename = "PostToolUse")]
    pub post_tool_use: Vec<HookMatcher>,
    #[serde(default, rename = "Stop")]
    pub stop: Vec<HookMatcher>,
    #[serde(default, rename = "Notification")]
    pub notification: Vec<HookMatcher>,
    #[serde(default, rename = "UserPromptSubmit")]
    pub user_prompt_submit: Vec<HookMatcher>,
}

impl HooksConfig {
    /// Merge `other` (higher precedence) by appending its matchers per event.
    pub fn merge(&mut self, other: HooksConfig) {
        self.pre_tool_use.extend(other.pre_tool_use);
        self.post_tool_use.extend(other.post_tool_use);
        self.stop.extend(other.stop);
        self.notification.extend(other.notification);
        self.user_prompt_submit.extend(other.user_prompt_submit);
    }
}

#[derive(Debug, Clone, Default, Deserialize, PartialEq, Eq)]
pub struct HookMatcher {
    /// Tool-name pattern (tool events only). Empty or `*` = match all. Supports
    /// `A|B` alternatives. Ignored for non-tool events.
    #[serde(default)]
    pub matcher: String,
    #[serde(default)]
    pub hooks: Vec<HookCommand>,
}

#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
pub struct HookCommand {
    #[serde(rename = "type", default = "default_hook_type")]
    pub r#type: String,
    #[serde(default)]
    pub command: String,
    #[serde(default)]
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
        return true; // non-tool event: matcher ignored
    };
    let m = matcher.trim();
    if m.is_empty() || m == "*" {
        return true;
    }
    m.split('|').any(|alt| alt.trim() == tool)
}

/// Decide a single command's effect from its exit code + output. Exit 2, or a
/// JSON stdout `{"decision":"block"}`, blocks; exit 0 continues (stdout = context);
/// other non-zero is a non-blocking error.
fn decide(exit_code: Option<i32>, stdout: &str, stderr: &str) -> HookCmdResult {
    // JSON control object on stdout takes precedence.
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
    if let Some(mut stdin) = child.stdin.take() {
        let _ = stdin.write_all(stdin_json.as_bytes()).await;
        // Drop closes stdin so the hook's read terminates.
    }
    let timeout = Duration::from_secs(cmd.timeout.unwrap_or(DEFAULT_HOOK_TIMEOUT_SECS));
    let output = match tokio::time::timeout(timeout, child.wait_with_output()).await {
        Ok(Ok(o)) => o,
        Ok(Err(e)) => return HookCmdResult::Error(format!("hook io error: {e}")),
        Err(_) => return HookCmdResult::Error(format!("hook timed out after {}s", timeout.as_secs())),
    };
    decide(
        output.status.code(),
        &String::from_utf8_lossy(&output.stdout),
        &String::from_utf8_lossy(&output.stderr),
    )
}

/// Run all matching hooks for an event. `tool_name` is `Some` for tool events.
/// Returns the first `Block` encountered, else `Continue` with concatenated
/// non-blocking stdout as context.
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
        // Non-tool events ignore the matcher.
        assert!(matcher_matches("Bash", None));
    }

    #[test]
    fn decide_from_exit_and_json() {
        assert!(matches!(decide(Some(0), "", ""), HookCmdResult::Continue(_)));
        assert!(matches!(decide(Some(2), "", "nope"), HookCmdResult::Block(r) if r == "nope"));
        assert!(matches!(decide(Some(1), "", "boom"), HookCmdResult::Error(_)));
        // JSON block decision wins regardless of exit 0.
        match decide(Some(0), r#"{"decision":"block","reason":"policy"}"#, "") {
            HookCmdResult::Block(r) => assert_eq!(r, "policy"),
            _ => panic!("expected block"),
        }
        // stdout on exit 0 becomes context.
        match decide(Some(0), "extra context", "") {
            HookCmdResult::Continue(c) => assert_eq!(c, "extra context"),
            _ => panic!("expected continue"),
        }
    }

    #[tokio::test]
    async fn run_event_hooks_blocks_on_exit_2() {
        let matchers = vec![HookMatcher {
            matcher: String::new(),
            hooks: vec![HookCommand {
                r#type: "command".into(),
                command: "echo denied >&2; exit 2".into(),
                timeout: None,
            }],
        }];
        let out = run_event_hooks(&matchers, None, &json!({"hook_event_name": "Stop"})).await;
        assert_eq!(out, HookOutcome::Block { reason: "denied".into() });
    }

    #[tokio::test]
    async fn run_event_hooks_collects_context_and_passes_stdin() {
        // The hook echoes back a field from the JSON payload on stdin.
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
    async fn non_matching_tool_hook_is_skipped() {
        let matchers = vec![HookMatcher {
            matcher: "Bash".into(),
            hooks: vec![HookCommand {
                r#type: "command".into(),
                command: "exit 2".into(),
                timeout: None,
            }],
        }];
        // Tool event for Read → Bash matcher doesn't apply → not blocked.
        let out = run_event_hooks(&matchers, Some("Read"), &json!({})).await;
        assert_eq!(out, HookOutcome::Continue { context: None });
    }
}
