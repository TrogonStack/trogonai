//! In-process permission gate: Runner → LocalSet handler → ACP client.
//!
//! When the agent loop is about to execute a tool, it calls `PermissionChecker::check`.
//! `ChannelPermissionChecker` sends the request over an mpsc channel to the ACP
//! `LocalSet` task (the only context that can call `conn.request_permission`).
//! The caller awaits a oneshot reply with the user's allow/deny decision.

use std::sync::{Arc, Mutex};

use serde_json::Value;
use tokio::sync::{mpsc, oneshot};
use trogon_agent_core::agent_loop::PermissionChecker;

use crate::permission_rules::{PermissionRules, RuleDecision};
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
    if let Some(path) = tool_input.get("path").and_then(|v| v.as_str()) {
        return path.to_string();
    }
    if tool_name == "bash" || tool_name == "Bash" {
        if let Some(cmd) = tool_input.get("command").and_then(|v| v.as_str()) {
            let prefix: String = cmd.chars().take(60).collect();
            return prefix;
        }
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
    let path = tool_input["path"].as_str().unwrap_or("");

    let matching: Vec<&PolicyAction> = policies
        .iter()
        .filter(|p| p.tool == tool_name)
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

#[cfg(test)]
mod tests {
    use super::*;
    use trogon_agent_core::agent_loop::PermissionChecker;

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
}
