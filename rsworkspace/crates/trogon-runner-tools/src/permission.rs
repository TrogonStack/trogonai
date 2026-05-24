//! In-process permission gate: Runner → LocalSet handler → ACP client.
//!
//! When the agent loop is about to execute a tool, it calls `PermissionChecker::check`.
//! `ChannelPermissionChecker` sends the request over an mpsc channel to the ACP
//! `LocalSet` task (the only context that can call `conn.request_permission`).
//! The caller awaits a oneshot reply with the user's allow/deny decision.

use std::sync::{Arc, Mutex};

use serde_json::Value;
use tokio::sync::{mpsc, oneshot};
use trogon_tools::PermissionChecker;

use crate::permission_rules::{
    extract_path_from_input, normalize_tool_name, PermissionRules, RuleDecision,
};
use crate::session_store::{AuditEntry, AuditOutcome, PolicyAction, ToolPolicy};

/// A single permission check request sent from the Runner to the ACP connection handler.
pub struct PermissionReq {
    pub session_id: String,
    pub tool_call_id: String,
    pub tool_name: String,
    pub tool_input: Value,
    /// Send `true` to allow, `false` to deny.
    pub response_tx: oneshot::Sender<bool>,
}

/// Sender half — given to the Runner so it can forward permission requests.
pub type PermissionTx = mpsc::Sender<PermissionReq>;

/// Shared buffer for audit entries accumulated during a prompt.
pub type AuditBuf = Arc<Mutex<Vec<AuditEntry>>>;

fn extract_input_summary(tool_name: &str, tool_input: &Value) -> String {
    if let Some(path) = extract_path_from_input(tool_input) {
        return path.to_string();
    }
    if normalize_tool_name(tool_name) == "bash"
        && let Some(cmd) = tool_input.get("command").and_then(|v| v.as_str())
    {
        let prefix: String = cmd.chars().take(60).collect();
        return prefix;
    }
    tool_name.to_string()
}

fn push_audit(buf: &AuditBuf, tool: &str, input: &Value, outcome: AuditOutcome) {
    let entry = AuditEntry {
        timestamp: crate::session_store::now_iso8601(),
        tool: tool.to_string(),
        input_summary: extract_input_summary(tool, input),
        outcome,
    };
    if let Ok(mut guard) = buf.lock() {
        guard.push(entry);
    }
}

/// `PermissionChecker` implementation that routes requests through an mpsc channel
/// to be handled by the ACP `LocalSet` task (which holds `AgentSideConnection`).
pub struct ChannelPermissionChecker {
    pub session_id: String,
    pub tx: PermissionTx,
    /// Tools for which the user previously chose "Always Allow" — auto-approved.
    pub allowed_tools: Vec<String>,
    pub audit_buf: AuditBuf,
}

impl PermissionChecker for ChannelPermissionChecker {
    #[cfg_attr(coverage, coverage(off))]
    fn check<'a>(
        &'a self,
        tool_call_id: &'a str,
        tool_name: &'a str,
        tool_input: &'a Value,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = bool> + Send + 'a>> {
        // Auto-allow tools the user has previously allowed for this session.
        if self.allowed_tools.iter().any(|t| t == tool_name) {
            push_audit(&self.audit_buf, tool_name, tool_input, AuditOutcome::Allowed);
            return Box::pin(async move { true });
        }

        let session_id = self.session_id.clone();
        let tool_call_id = tool_call_id.to_string();
        let tool_name = tool_name.to_string();
        let tool_input = tool_input.clone();
        let tx = self.tx.clone();
        let audit_buf = self.audit_buf.clone();

        Box::pin(async move {
            push_audit(&audit_buf, &tool_name, &tool_input, AuditOutcome::RequiredApproval);
            let (resp_tx, resp_rx) = oneshot::channel();
            let req = PermissionReq {
                session_id,
                tool_call_id,
                tool_name: tool_name.clone(),
                tool_input: tool_input.clone(),
                response_tx: resp_tx,
            };
            if tx.send(req).await.is_err() {
                push_audit(&audit_buf, &tool_name, &tool_input, AuditOutcome::DeniedByUser);
                // Channel closed — default deny
                return false;
            }
            // Wait up to 60 seconds for the user to respond; deny on timeout or error
            match tokio::time::timeout(std::time::Duration::from_secs(60), resp_rx).await {
                Ok(Ok(true)) => {
                    push_audit(&audit_buf, &tool_name, &tool_input, AuditOutcome::ApprovedByUser);
                    true
                }
                _ => {
                    push_audit(&audit_buf, &tool_name, &tool_input, AuditOutcome::DeniedByUser);
                    false
                }
            }
        })
    }
}

/// Evaluate `tool_policies` for the given tool name and path.
///
/// Evaluation order: Deny beats Allow beats RequireApproval. Returns `None`
/// when no policy matches (caller should fall through to interactive ask).
fn eval_tool_policies(
    policies: &[ToolPolicy],
    tool_name: &str,
    tool_input: &Value,
) -> Option<PolicyAction> {
    let path = extract_path_from_input(tool_input).unwrap_or("");
    let normalized = normalize_tool_name(tool_name);

    let matching: Vec<&PolicyAction> = policies
        .iter()
        .filter(|p| p.tool == tool_name || p.tool == normalized)
        .filter(|p| {
            globset::Glob::new(&p.path_pattern)
                .ok()
                .map(|g| g.compile_matcher().is_match(path))
                .unwrap_or(false)
        })
        .map(|p| &p.action)
        .collect();

    if matching.is_empty() {
        return None;
    }
    if matching.iter().any(|a| matches!(a, PolicyAction::Deny)) {
        return Some(PolicyAction::Deny);
    }
    if matching.iter().any(|a| matches!(a, PolicyAction::Allow)) {
        return Some(PolicyAction::Allow);
    }
    Some(PolicyAction::RequireApproval)
}

/// `PermissionChecker` that first evaluates static [`PermissionRules`] before
/// forwarding to the interactive [`ChannelPermissionChecker`].
///
/// - `Deny` → rejects immediately (no UI prompt).
/// - `Allow` → approves immediately (no UI prompt).
/// - `Ask` → falls through to the channel checker for interactive approval.
pub struct RulesPermissionChecker {
    pub rules: Arc<PermissionRules>,
    pub tool_policies: Vec<ToolPolicy>,
    pub inner: ChannelPermissionChecker,
}

impl PermissionChecker for RulesPermissionChecker {
    fn check<'a>(
        &'a self,
        tool_call_id: &'a str,
        tool_name: &'a str,
        tool_input: &'a Value,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = bool> + Send + 'a>> {
        match self.rules.check(tool_name, tool_input) {
            RuleDecision::Deny => {
                push_audit(&self.inner.audit_buf, tool_name, tool_input, AuditOutcome::Denied);
                return Box::pin(async move { false });
            }
            RuleDecision::Allow => {
                push_audit(&self.inner.audit_buf, tool_name, tool_input, AuditOutcome::Allowed);
                return Box::pin(async move { true });
            }
            RuleDecision::Ask => {}
        }

        match eval_tool_policies(&self.tool_policies, tool_name, tool_input) {
            Some(PolicyAction::Deny) => {
                push_audit(&self.inner.audit_buf, tool_name, tool_input, AuditOutcome::Denied);
                Box::pin(async move { false })
            }
            Some(PolicyAction::Allow) => {
                push_audit(&self.inner.audit_buf, tool_name, tool_input, AuditOutcome::Allowed);
                Box::pin(async move { true })
            }
            Some(PolicyAction::RequireApproval) | None => {
                self.inner.check(tool_call_id, tool_name, tool_input)
            }
        }
    }
}

/// Read-only tools auto-allowed in `default` mode (Claude Code parity — no prompt for reads).
const READ_ONLY_TOOLS: &[&str] = &[
    "read_file",
    "glob",
    "list_dir",
    "grep",
    "todo_read",
    "git_status",
    "git_diff",
    "git_log",
];

/// File-edit tools auto-allowed in `acceptEdits` mode; bash and MCP still prompt.
const ACCEPT_EDITS_TOOLS: &[&str] = &["write_file", "str_replace", "notebook_edit"];

/// Tools denied in `plan` mode until the ExitPlanMode permission flow completes.
const PLAN_DENIED_TOOLS: &[&str] = &[
    "write_file",
    "str_replace",
    "notebook_edit",
    "bash",
    "todo_write",
    "git_commit",
    "fetch_url",
];

fn is_read_only_tool(tool_name: &str) -> bool {
    READ_ONLY_TOOLS.contains(&normalize_tool_name(tool_name))
}

/// Bash commands that only read or inspect the filesystem (no writes, no exec).
fn is_read_only_bash_command(tool_input: &Value) -> bool {
    let Some(cmd) = tool_input.get("command").and_then(|v| v.as_str()) else {
        return false;
    };
    let cmd = cmd.trim();
    if cmd.is_empty() {
        return false;
    }
    let lowered = cmd.to_ascii_lowercase();
    if lowered.contains("rm -rf")
        || lowered.contains("-exec")
        || lowered.contains("-delete")
        || lowered.contains("sudo ")
        || lowered.contains("chmod ")
        || lowered.contains("chown ")
    {
        return false;
    }
    let base = cmd
        .split_whitespace()
        .next()
        .unwrap_or("")
        .trim_start_matches('(');
    matches!(
        base,
        "pwd" | "ls" | "ll" | "dir" | "find" | "echo" | "cat" | "head" | "tail" | "wc"
            | "which" | "file" | "stat" | "du" | "df" | "test" | "rg" | "grep" | "tree"
    )
}

fn is_edit_tool(tool_name: &str) -> bool {
    ACCEPT_EDITS_TOOLS.contains(&normalize_tool_name(tool_name))
}

fn is_plan_denied_tool(tool_name: &str) -> bool {
    let n = normalize_tool_name(tool_name);
    PLAN_DENIED_TOOLS.contains(&n)
        || matches!(tool_name, "Edit" | "Write" | "MultiEdit" | "NotebookEdit" | "Bash")
}

/// Applies session permission mode policy before delegating to [`RulesPermissionChecker`].
///
/// | Mode | Behavior |
/// |------|----------|
/// | `default` | Auto-allow read-only tools; TROGON.md rules + prompt for bash, edits, MCP |
/// | `acceptEdits` | Auto-allow file edits; prompt bash, MCP, and other tools |
/// | `dontAsk` | Auto-allow all (audit only) |
/// | `plan` | Deny write/bash tools |
/// | `bypassPermissions` | Not installed — caller skips checker entirely |
pub struct ModePermissionChecker {
    pub mode: String,
    pub inner: RulesPermissionChecker,
}

impl PermissionChecker for ModePermissionChecker {
    fn check<'a>(
        &'a self,
        tool_call_id: &'a str,
        tool_name: &'a str,
        tool_input: &'a Value,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = bool> + Send + 'a>> {
        let audit_buf = self.inner.inner.audit_buf.clone();
        match self.mode.as_str() {
            "dontAsk" => {
                push_audit(&audit_buf, tool_name, tool_input, AuditOutcome::Allowed);
                Box::pin(async move { true })
            }
            "default" if is_read_only_tool(tool_name) => {
                push_audit(&audit_buf, tool_name, tool_input, AuditOutcome::Allowed);
                Box::pin(async move { true })
            }
            "default"
                if normalize_tool_name(tool_name) == "bash" && is_read_only_bash_command(tool_input) =>
            {
                push_audit(&audit_buf, tool_name, tool_input, AuditOutcome::Allowed);
                Box::pin(async move { true })
            }
            "acceptEdits" if is_edit_tool(tool_name) => {
                push_audit(&audit_buf, tool_name, tool_input, AuditOutcome::Allowed);
                Box::pin(async move { true })
            }
            "plan" if is_plan_denied_tool(tool_name) => {
                push_audit(&audit_buf, tool_name, tool_input, AuditOutcome::Denied);
                Box::pin(async move { false })
            }
            _ => self.inner.check(tool_call_id, tool_name, tool_input),
        }
    }
}

/// Build a mode-aware permission checker, or `None` when mode is `bypassPermissions`.
pub fn build_mode_permission_checker(
    mode: &str,
    session_id: &str,
    perm_tx: &PermissionTx,
    allowed_tools: Vec<String>,
    rules: Arc<PermissionRules>,
    tool_policies: Vec<ToolPolicy>,
) -> Option<Arc<dyn PermissionChecker>> {
    if mode == "bypassPermissions" {
        return None;
    }
    let audit_buf: AuditBuf = Arc::new(Mutex::new(Vec::new()));
    let inner = ChannelPermissionChecker {
        session_id: session_id.to_string(),
        tx: perm_tx.clone(),
        allowed_tools,
        audit_buf,
    };
    let rules_checker = RulesPermissionChecker {
        rules,
        tool_policies,
        inner,
    };
    Some(Arc::new(ModePermissionChecker {
        mode: mode.to_string(),
        inner: rules_checker,
    }))
}

/// Gate a single tool invocation using session mode, rules, and optional NATS permission channel.
/// Returns `true` when the tool may run; `false` when denied.
#[allow(clippy::too_many_arguments)]
pub async fn check_tool_permission(
    mode: &str,
    session_id: &str,
    perm_tx: Option<&PermissionTx>,
    allowed_tools: &[String],
    rules: PermissionRules,
    tool_policies: &[ToolPolicy],
    tool_call_id: &str,
    tool_name: &str,
    tool_input: &Value,
) -> bool {
    if mode == "bypassPermissions" {
        return true;
    }
    let Some(tx) = perm_tx else {
        return true;
    };
    let Some(checker) = build_mode_permission_checker(
        mode,
        session_id,
        tx,
        allowed_tools.to_vec(),
        Arc::new(rules),
        tool_policies.to_vec(),
    ) else {
        return true;
    };
    checker
        .check(tool_call_id, tool_name, tool_input)
        .await
}

#[cfg(test)]
mod tests {
    use super::*;
    use trogon_tools::PermissionChecker;

    fn make_checker(tx: PermissionTx, allowed_tools: Vec<String>) -> ChannelPermissionChecker {
        ChannelPermissionChecker {
            session_id: "sess-1".to_string(),
            tx,
            allowed_tools,
            audit_buf: Arc::new(Mutex::new(vec![])),
        }
    }

    fn make_checker_with_buf(
        tx: PermissionTx,
        allowed_tools: Vec<String>,
        buf: AuditBuf,
    ) -> ChannelPermissionChecker {
        ChannelPermissionChecker {
            session_id: "sess-1".to_string(),
            tx,
            allowed_tools,
            audit_buf: buf,
        }
    }

    #[tokio::test]
    async fn auto_allows_tool_in_allowed_list() {
        let (tx, _rx) = mpsc::channel(1);
        let checker = make_checker(tx, vec!["Bash".to_string()]);
        let result = checker
            .check("tc-1", "Bash", &serde_json::Value::Null)
            .await;
        assert!(result, "Bash should be auto-allowed");
    }

    #[tokio::test]
    async fn auto_allows_is_case_sensitive() {
        let (tx, _rx) = mpsc::channel(1);
        let checker = make_checker(tx, vec!["Bash".to_string()]);
        // Lowercase "bash" is NOT the same as "Bash" — channel will be used
        let (tx2, mut rx2) = mpsc::channel(1);
        let checker2 = make_checker(tx2, vec!["Bash".to_string()]);
        // Respond with false from a separate task so we don't deadlock
        tokio::spawn(async move {
            if let Some(req) = rx2.recv().await {
                let _ = req.response_tx.send(false);
            }
        });
        let result = checker2
            .check("tc-1", "bash", &serde_json::Value::Null)
            .await;
        assert!(
            !result,
            "lowercase bash must not match Bash in allowed list"
        );
        drop(checker);
    }

    #[tokio::test]
    async fn channel_allow_returns_true() {
        let (tx, mut rx) = mpsc::channel(1);
        let checker = make_checker(tx, vec![]);
        tokio::spawn(async move {
            if let Some(req) = rx.recv().await {
                let _ = req.response_tx.send(true);
            }
        });
        let result = checker
            .check("tc-2", "Edit", &serde_json::Value::Null)
            .await;
        assert!(result, "channel returned allow");
    }

    #[tokio::test]
    async fn channel_deny_returns_false() {
        let (tx, mut rx) = mpsc::channel(1);
        let checker = make_checker(tx, vec![]);
        tokio::spawn(async move {
            if let Some(req) = rx.recv().await {
                let _ = req.response_tx.send(false);
            }
        });
        let result = checker
            .check("tc-3", "Write", &serde_json::Value::Null)
            .await;
        assert!(!result, "channel returned deny");
    }

    #[tokio::test]
    async fn closed_channel_returns_false() {
        let (tx, rx) = mpsc::channel(1);
        drop(rx); // close the receiver
        let checker = make_checker(tx, vec![]);
        let result = checker
            .check("tc-4", "Read", &serde_json::Value::Null)
            .await;
        assert!(!result, "closed channel should default to deny");
    }

    #[cfg_attr(coverage, coverage(off))]
    #[tokio::test]
    async fn permission_req_carries_correct_fields() {
        let (tx, mut rx) = mpsc::channel(1);
        let checker = make_checker(tx, vec![]);
        let input = serde_json::json!({"path": "/tmp/x"});
        tokio::spawn(async move {
            if let Some(req) = rx.recv().await {
                assert_eq!(req.session_id, "sess-1");
                assert_eq!(req.tool_call_id, "tc-99");
                assert_eq!(req.tool_name, "Read");
                assert_eq!(req.tool_input, serde_json::json!({"path": "/tmp/x"}));
                let _ = req.response_tx.send(true);
            }
        });
        let _ = checker.check("tc-99", "Read", &input).await;
    }

    /// Covers line 68: `_ => false` when response_tx is dropped without sending,
    /// causing resp_rx to return an error immediately.
    #[tokio::test]
    async fn dropped_response_tx_returns_false() {
        let (tx, mut rx) = mpsc::channel(1);
        let checker = make_checker(tx, vec![]);
        tokio::spawn(async move {
            if let Some(req) = rx.recv().await {
                drop(req.response_tx); // drop without sending — triggers Err on resp_rx
            }
        });
        let result = checker
            .check("tc-x", "Read", &serde_json::Value::Null)
            .await;
        assert!(!result, "dropped response_tx should return false");
    }

    // ── ModePermissionChecker ───────────────────────────────────────────────────

    #[tokio::test]
    async fn mode_checker_dont_ask_auto_allows_without_channel() {
        let (tx, rx) = mpsc::channel(1);
        drop(rx);
        let checker = ModePermissionChecker {
            mode: "dontAsk".to_string(),
            inner: make_rules_checker("", tx),
        };
        assert!(
            checker
                .check("tc-da", "bash", &serde_json::json!({"command": "rm -rf /"}))
                .await
        );
    }

    #[tokio::test]
    async fn mode_checker_default_auto_allows_read_file_without_channel() {
        let (tx, rx) = mpsc::channel(1);
        drop(rx);
        let checker = ModePermissionChecker {
            mode: "default".to_string(),
            inner: make_rules_checker("", tx),
        };
        assert!(
            checker
                .check(
                    "tc-dr",
                    "Read",
                    &serde_json::json!({"file_path": "src/main.rs"}),
                )
                .await
        );
    }

    #[tokio::test]
    async fn mode_checker_default_prompts_bash_via_channel() {
        let (tx, mut rx) = mpsc::channel(1);
        let checker = ModePermissionChecker {
            mode: "default".to_string(),
            inner: make_rules_checker("", tx),
        };
        tokio::spawn(async move {
            if let Some(req) = rx.recv().await {
                let _ = req.response_tx.send(true);
            }
        });
        assert!(
            checker
                .check("tc-db", "Bash", &serde_json::json!({"command": "pwd"}))
                .await
        );
    }

    #[tokio::test]
    async fn mode_checker_accept_edits_auto_allows_write_file() {
        let (tx, rx) = mpsc::channel(1);
        drop(rx);
        let checker = ModePermissionChecker {
            mode: "acceptEdits".to_string(),
            inner: make_rules_checker("", tx),
        };
        assert!(
            checker
                .check("tc-ae", "write_file", &serde_json::json!({"path": "/tmp/x"}))
                .await
        );
    }

    #[tokio::test]
    async fn mode_checker_accept_edits_prompts_bash_via_channel() {
        let (tx, mut rx) = mpsc::channel(1);
        let checker = ModePermissionChecker {
            mode: "acceptEdits".to_string(),
            inner: make_rules_checker("", tx),
        };
        tokio::spawn(async move {
            if let Some(req) = rx.recv().await {
                let _ = req.response_tx.send(true);
            }
        });
        assert!(
            checker
                .check("tc-ab", "bash", &serde_json::json!({"command": "pwd"}))
                .await
        );
    }

    #[tokio::test]
    async fn mode_checker_accept_edits_prompts_read_without_rules() {
        let (tx, mut rx) = mpsc::channel(1);
        let checker = ModePermissionChecker {
            mode: "acceptEdits".to_string(),
            inner: make_rules_checker("", tx),
        };
        tokio::spawn(async move {
            if let Some(req) = rx.recv().await {
                let _ = req.response_tx.send(true);
            }
        });
        assert!(
            checker
                .check(
                    "tc-ar",
                    "read_file",
                    &serde_json::json!({"path": "src/main.rs"}),
                )
                .await
        );
    }

    #[tokio::test]
    async fn mode_checker_plan_denies_write_file() {
        let (tx, _rx) = mpsc::channel(1);
        let checker = ModePermissionChecker {
            mode: "plan".to_string(),
            inner: make_rules_checker("", tx),
        };
        assert!(
            !checker
                .check("tc-pl", "write_file", &serde_json::json!({"path": "/tmp/x"}))
                .await
        );
    }

    #[test]
    fn build_mode_permission_checker_none_for_bypass() {
        let (tx, _rx) = mpsc::channel(1);
        let checker = build_mode_permission_checker(
            "bypassPermissions",
            "sess-1",
            &tx,
            vec![],
            Arc::new(PermissionRules::default()),
            vec![],
        );
        assert!(checker.is_none());
    }

    // ── eval_tool_policies ────────────────────────────────────────────────────

    use crate::session_store::{PolicyAction, ToolPolicy};

    fn make_policy(tool: &str, pattern: &str, action: PolicyAction) -> ToolPolicy {
        ToolPolicy {
            tool: tool.to_string(),
            path_pattern: pattern.to_string(),
            action,
        }
    }

    #[test]
    fn no_policies_returns_none() {
        let result = eval_tool_policies(&[], "write_file", &serde_json::json!({"path": "/workspace/foo.rs"}));
        assert_eq!(result, None);
    }

    #[test]
    fn no_matching_tool_returns_none() {
        let policies = vec![make_policy("read_file", "/workspace/**", PolicyAction::Allow)];
        let result = eval_tool_policies(&policies, "write_file", &serde_json::json!({"path": "/workspace/foo.rs"}));
        assert_eq!(result, None);
    }

    #[test]
    fn matching_allow_policy_returns_allow() {
        let policies = vec![make_policy("write_file", "/workspace/**", PolicyAction::Allow)];
        let result = eval_tool_policies(&policies, "write_file", &serde_json::json!({"path": "/workspace/src/main.rs"}));
        assert_eq!(result, Some(PolicyAction::Allow));
    }

    #[test]
    fn matching_deny_policy_returns_deny() {
        let policies = vec![make_policy("write_file", "/workspace/**", PolicyAction::Deny)];
        let result = eval_tool_policies(&policies, "write_file", &serde_json::json!({"path": "/workspace/src/main.rs"}));
        assert_eq!(result, Some(PolicyAction::Deny));
    }

    #[test]
    fn deny_beats_allow_when_both_match() {
        let policies = vec![
            make_policy("write_file", "/workspace/**", PolicyAction::Allow),
            make_policy("write_file", "/workspace/secrets/**", PolicyAction::Deny),
        ];
        let result = eval_tool_policies(&policies, "write_file", &serde_json::json!({"path": "/workspace/secrets/key.txt"}));
        assert_eq!(result, Some(PolicyAction::Deny));
    }

    #[tokio::test]
    async fn rules_checker_tool_policy_deny_returns_false() {
        let (tx, _rx) = mpsc::channel(1);
        let inner = make_checker(tx, vec![]);
        let checker = RulesPermissionChecker {
            rules: Arc::new(crate::permission_rules::PermissionRules::default()),
            tool_policies: vec![make_policy("write_file", "/etc/**", PolicyAction::Deny)],
            inner,
        };
        let result = checker
            .check("tc-p1", "write_file", &serde_json::json!({"path": "/etc/passwd"}))
            .await;
        assert!(!result, "tool policy Deny should reject");
    }

    #[tokio::test]
    async fn rules_checker_tool_policy_allow_returns_true() {
        let (tx, _rx) = mpsc::channel(1);
        let inner = make_checker(tx, vec![]);
        let checker = RulesPermissionChecker {
            rules: Arc::new(crate::permission_rules::PermissionRules::default()),
            tool_policies: vec![make_policy("read_file", "/workspace/**", PolicyAction::Allow)],
            inner,
        };
        let result = checker
            .check("tc-p2", "read_file", &serde_json::json!({"path": "/workspace/main.rs"}))
            .await;
        assert!(result, "tool policy Allow should approve");
    }

    #[tokio::test]
    async fn rules_checker_require_approval_falls_through_to_channel() {
        let (tx, mut rx) = mpsc::channel(1);
        let inner = make_checker(tx, vec![]);
        let checker = RulesPermissionChecker {
            rules: Arc::new(crate::permission_rules::PermissionRules::default()),
            tool_policies: vec![make_policy("write_file", "/tmp/**", PolicyAction::RequireApproval)],
            inner,
        };
        tokio::spawn(async move {
            if let Some(req) = rx.recv().await {
                let _ = req.response_tx.send(true);
            }
        });
        let result = checker
            .check("tc-p3", "write_file", &serde_json::json!({"path": "/tmp/foo.txt"}))
            .await;
        assert!(result, "RequireApproval should fall through to channel (which approved)");
    }

    // ── extract_input_summary ────────────────────────────────────────────────────

    #[test]
    fn input_summary_uses_file_path_field() {
        let input = serde_json::json!({"file_path": "/home/user/file.txt"});
        assert_eq!(extract_input_summary("Read", &input), "/home/user/file.txt");
    }

    #[test]
    fn input_summary_uses_path_field() {
        let input = serde_json::json!({"path": "/home/user/file.txt"});
        assert_eq!(extract_input_summary("Read", &input), "/home/user/file.txt");
    }

    #[test]
    fn input_summary_bash_uses_command_prefix() {
        let input = serde_json::json!({"command": "cargo test --workspace"});
        assert_eq!(
            extract_input_summary("bash", &input),
            "cargo test --workspace"
        );
    }

    #[test]
    fn input_summary_bash_truncates_at_60_chars() {
        let long_cmd = "a".repeat(100);
        let input = serde_json::json!({"command": long_cmd});
        let summary = extract_input_summary("Bash", &input);
        assert_eq!(summary.len(), 60);
    }

    #[test]
    fn input_summary_fallback_is_tool_name() {
        let input = serde_json::json!({"other": "value"});
        assert_eq!(extract_input_summary("SomeTool", &input), "SomeTool");
    }

    #[test]
    fn input_summary_path_takes_priority_over_command() {
        let input = serde_json::json!({"path": "/tmp/x", "command": "do something"});
        assert_eq!(extract_input_summary("bash", &input), "/tmp/x");
    }

    // ── audit recording ──────────────────────────────────────────────────────────

    #[tokio::test]
    async fn auto_allow_records_allowed_outcome() {
        let (tx, _rx) = mpsc::channel(1);
        let buf: AuditBuf = Arc::new(Mutex::new(vec![]));
        let checker =
            make_checker_with_buf(tx, vec!["Read".to_string()], buf.clone());
        let input = serde_json::json!({"path": "/etc/hosts"});
        checker.check("tc-1", "Read", &input).await;
        let entries = buf.lock().unwrap();
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].tool, "Read");
        assert_eq!(entries[0].outcome, AuditOutcome::Allowed);
        assert_eq!(entries[0].input_summary, "/etc/hosts");
    }

    #[tokio::test]
    async fn channel_approve_records_required_then_approved() {
        let (tx, mut rx) = mpsc::channel(1);
        let buf: AuditBuf = Arc::new(Mutex::new(vec![]));
        let checker = make_checker_with_buf(tx, vec![], buf.clone());
        tokio::spawn(async move {
            if let Some(req) = rx.recv().await {
                let _ = req.response_tx.send(true);
            }
        });
        checker
            .check("tc-2", "Write", &serde_json::json!({"path": "/tmp/out"}))
            .await;
        let entries = buf.lock().unwrap();
        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0].outcome, AuditOutcome::RequiredApproval);
        assert_eq!(entries[1].outcome, AuditOutcome::ApprovedByUser);
        assert_eq!(entries[1].input_summary, "/tmp/out");
    }

    #[tokio::test]
    async fn channel_deny_records_required_then_denied() {
        let (tx, mut rx) = mpsc::channel(1);
        let buf: AuditBuf = Arc::new(Mutex::new(vec![]));
        let checker = make_checker_with_buf(tx, vec![], buf.clone());
        tokio::spawn(async move {
            if let Some(req) = rx.recv().await {
                let _ = req.response_tx.send(false);
            }
        });
        checker
            .check("tc-3", "Bash", &serde_json::json!({"command": "rm -rf /"}))
            .await;
        let entries = buf.lock().unwrap();
        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0].outcome, AuditOutcome::RequiredApproval);
        assert_eq!(entries[1].outcome, AuditOutcome::DeniedByUser);
    }

    #[tokio::test]
    async fn closed_channel_records_denied_by_user() {
        let (tx, rx) = mpsc::channel(1);
        drop(rx);
        let buf: AuditBuf = Arc::new(Mutex::new(vec![]));
        let checker = make_checker_with_buf(tx, vec![], buf.clone());
        checker
            .check("tc-4", "Edit", &serde_json::Value::Null)
            .await;
        let entries = buf.lock().unwrap();
        assert!(entries.iter().any(|e| e.outcome == AuditOutcome::DeniedByUser));
    }

    // ── RulesPermissionChecker: static rules audit ────────────────────────────

    #[tokio::test]
    async fn rules_checker_static_deny_returns_false_and_records_denied() {
        use crate::permission_rules::PermissionRules;
        let rules = PermissionRules::parse("## Permissions\ndeny_paths: /etc/**\n");
        let (tx, _rx) = mpsc::channel(1);
        let buf: AuditBuf = Arc::new(Mutex::new(vec![]));
        let inner = make_checker_with_buf(tx, vec![], buf.clone());
        let checker = RulesPermissionChecker {
            rules: Arc::new(rules),
            tool_policies: vec![],
            inner,
        };
        let result = checker
            .check("tc-sd", "write_file", &serde_json::json!({"path": "/etc/passwd"}))
            .await;
        assert!(!result, "static Deny must return false");
        let entries = buf.lock().unwrap();
        assert_eq!(entries.len(), 1, "must record one audit entry");
        assert_eq!(entries[0].outcome, AuditOutcome::Denied);
        assert_eq!(entries[0].input_summary, "/etc/passwd");
        assert_eq!(entries[0].tool, "write_file");
    }

    #[tokio::test]
    async fn rules_checker_static_allow_returns_true_and_records_allowed() {
        use crate::permission_rules::PermissionRules;
        let rules = PermissionRules::parse("## Permissions\nallow_paths: /workspace/**\n");
        let (tx, _rx) = mpsc::channel(1);
        let buf: AuditBuf = Arc::new(Mutex::new(vec![]));
        let inner = make_checker_with_buf(tx, vec![], buf.clone());
        let checker = RulesPermissionChecker {
            rules: Arc::new(rules),
            tool_policies: vec![],
            inner,
        };
        let result = checker
            .check("tc-sa", "read_file", &serde_json::json!({"path": "/workspace/main.rs"}))
            .await;
        assert!(result, "static Allow must return true");
        let entries = buf.lock().unwrap();
        assert_eq!(entries.len(), 1, "must record one audit entry");
        assert_eq!(entries[0].outcome, AuditOutcome::Allowed);
        assert_eq!(entries[0].input_summary, "/workspace/main.rs");
        assert_eq!(entries[0].tool, "read_file");
    }

    // ── eval_tool_policies: RequireApproval-only case ─────────────────────────

    #[test]
    fn eval_tool_policies_require_approval_only_returns_some() {
        let policies = vec![make_policy("write_file", "/tmp/**", PolicyAction::RequireApproval)];
        let result = eval_tool_policies(
            &policies,
            "write_file",
            &serde_json::json!({"path": "/tmp/foo.txt"}),
        );
        assert_eq!(result, Some(PolicyAction::RequireApproval));
    }

    // ── tool-policy audit gap (known) ─────────────────────────────────────────

    #[tokio::test]
    async fn rules_checker_tool_policy_deny_records_denied_audit() {
        let (tx, _rx) = mpsc::channel(1);
        let buf: AuditBuf = Arc::new(Mutex::new(vec![]));
        let inner = make_checker_with_buf(tx, vec![], buf.clone());
        let checker = RulesPermissionChecker {
            rules: Arc::new(crate::permission_rules::PermissionRules::default()),
            tool_policies: vec![make_policy("write_file", "/etc/**", PolicyAction::Deny)],
            inner,
        };
        checker
            .check("tc-tpd", "write_file", &serde_json::json!({"path": "/etc/passwd"}))
            .await;
        let entries = buf.lock().unwrap();
        assert_eq!(entries.len(), 1, "tool-policy Deny must record one audit entry");
        assert_eq!(entries[0].outcome, AuditOutcome::Denied);
        assert_eq!(entries[0].tool, "write_file");
        assert_eq!(entries[0].input_summary, "/etc/passwd");
    }

    #[tokio::test]
    async fn rules_checker_tool_policy_allow_records_allowed_audit() {
        let (tx, _rx) = mpsc::channel(1);
        let buf: AuditBuf = Arc::new(Mutex::new(vec![]));
        let inner = make_checker_with_buf(tx, vec![], buf.clone());
        let checker = RulesPermissionChecker {
            rules: Arc::new(crate::permission_rules::PermissionRules::default()),
            tool_policies: vec![make_policy("read_file", "/workspace/**", PolicyAction::Allow)],
            inner,
        };
        checker
            .check("tc-tpa", "read_file", &serde_json::json!({"path": "/workspace/main.rs"}))
            .await;
        let entries = buf.lock().unwrap();
        assert_eq!(entries.len(), 1, "tool-policy Allow must record one audit entry");
        assert_eq!(entries[0].outcome, AuditOutcome::Allowed);
        assert_eq!(entries[0].tool, "read_file");
        assert_eq!(entries[0].input_summary, "/workspace/main.rs");
    }

    // ── RulesPermissionChecker ────────────────────────────────────────────────

    fn make_rules_checker(rules_text: &str, tx: PermissionTx) -> RulesPermissionChecker {
        RulesPermissionChecker {
            rules: Arc::new(PermissionRules::parse(rules_text)),
            tool_policies: vec![],
            inner: make_checker(tx, vec![]),
        }
    }

    /// `Deny` rule → returns false immediately, without consulting the inner checker.
    #[tokio::test]
    async fn rules_deny_returns_false_without_inner() {
        // Deny all bash commands.
        let (tx, rx) = mpsc::channel(1);
        // Drop rx so any inner channel send would return an error — but it should
        // never be reached for a Deny decision.
        drop(rx);
        let checker = make_rules_checker(
            "## Permissions\ndeny_commands: bash\n",
            tx,
        );
        let result = checker
            .check("tc-d1", "bash", &serde_json::json!({"command": "rm -rf /"}))
            .await;
        assert!(!result, "Deny rule must return false");
    }

    /// `Allow` rule → returns true immediately, without consulting the inner checker.
    #[tokio::test]
    async fn rules_allow_returns_true_without_inner() {
        // Allow all read_file calls.
        let (tx, rx) = mpsc::channel(1);
        drop(rx); // inner channel closed — would deny if reached
        let checker = make_rules_checker(
            "## Permissions\nallow_paths: **\n",
            tx,
        );
        let result = checker
            .check(
                "tc-a1",
                "read_file",
                &serde_json::json!({"path": "/home/user/file.txt"}),
            )
            .await;
        assert!(result, "Allow rule must return true");
    }

    /// `Ask` rule → delegates to the inner `ChannelPermissionChecker`.
    #[tokio::test]
    async fn rules_ask_delegates_to_inner_checker_allow() {
        // Empty rules → all decisions are Ask.
        let (tx, mut rx) = mpsc::channel::<PermissionReq>(1);
        tokio::spawn(async move {
            if let Some(req) = rx.recv().await {
                let _ = req.response_tx.send(true);
            }
        });
        let checker = make_rules_checker("", tx);
        let result = checker
            .check("tc-q1", "write_file", &serde_json::json!({"path": "/tmp/x"}))
            .await;
        assert!(result, "Ask must forward to inner and return its allow decision");
    }

    /// `Ask` rule → inner returns deny.
    #[tokio::test]
    async fn rules_ask_delegates_to_inner_checker_deny() {
        let (tx, mut rx) = mpsc::channel::<PermissionReq>(1);
        tokio::spawn(async move {
            if let Some(req) = rx.recv().await {
                let _ = req.response_tx.send(false);
            }
        });
        let checker = make_rules_checker("", tx);
        let result = checker
            .check("tc-q2", "write_file", &serde_json::json!({"path": "/tmp/x"}))
            .await;
        assert!(!result, "Ask must forward to inner and return its deny decision");
    }

    /// Deny takes precedence even when an allow rule also matches.
    #[tokio::test]
    async fn rules_deny_beats_allow() {
        let (tx, rx) = mpsc::channel(1);
        drop(rx);
        let checker = make_rules_checker(
            "## Permissions\nallow_paths: **\ndeny_paths: /etc/**\n",
            tx,
        );
        let result = checker
            .check(
                "tc-db",
                "read_file",
                &serde_json::json!({"path": "/etc/passwd"}),
            )
            .await;
        assert!(!result, "Deny must beat Allow");
    }
}
