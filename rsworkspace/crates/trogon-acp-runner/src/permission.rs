//! In-process permission gate: Runner → LocalSet handler → ACP client.
//!
//! When the agent loop is about to execute a tool, it calls `PermissionChecker::check`.
//! `ChannelPermissionChecker` sends the request over an mpsc channel to the ACP
//! `LocalSet` task (the only context that can call `conn.request_permission`).
//! The caller awaits a oneshot reply with the user's allow/deny decision.

use serde_json::Value;
use tokio::sync::{mpsc, oneshot};
use trogon_agent_core::agent_loop::PermissionChecker;

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

/// `PermissionChecker` implementation that routes requests through an mpsc channel
/// to be handled by the ACP `LocalSet` task (which holds `AgentSideConnection`).
pub struct ChannelPermissionChecker {
    pub session_id: String,
    pub tx: PermissionTx,
    /// Tools for which the user previously chose "Always Allow" — auto-approved.
    pub allowed_tools: Vec<String>,
}

impl PermissionChecker for ChannelPermissionChecker {
    fn check<'a>(
        &'a self,
        tool_call_id: &'a str,
        tool_name: &'a str,
        tool_input: &'a Value,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = bool> + Send + 'a>> {
        // Auto-allow tools the user has previously allowed for this session.
        if self.allowed_tools.iter().any(|t| t == tool_name) {
            return Box::pin(async move { true });
        }

        let session_id = self.session_id.clone();
        let tool_call_id = tool_call_id.to_string();
        let tool_name = tool_name.to_string();
        let tool_input = tool_input.clone();
        let tx = self.tx.clone();

        Box::pin(async move {
            let (resp_tx, resp_rx) = oneshot::channel();
            let req = PermissionReq {
                session_id,
                tool_call_id,
                tool_name,
                tool_input,
                response_tx: resp_tx,
            };
            if tx.send(req).await.is_err() {
                // Channel closed — default deny
                return false;
            }
            // Wait up to 60 seconds for the user to respond; deny on timeout or error
            match tokio::time::timeout(std::time::Duration::from_secs(60), resp_rx).await {
                Ok(Ok(allowed)) => allowed,
                _ => false,
            }
        })
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
}
