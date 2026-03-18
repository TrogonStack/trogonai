//! In-process permission gate: Runner → LocalSet handler → ACP client.
//!
//! When the agent loop is about to execute a tool, it calls `PermissionChecker::check`.
//! `ChannelPermissionChecker` sends the request over an mpsc channel to the ACP
//! `LocalSet` task (the only context that can call `conn.request_permission`).
//! The caller awaits a oneshot reply with the user's allow/deny decision.

use serde_json::Value;
use tokio::sync::{mpsc, oneshot};
use trogon_agent::agent_loop::PermissionChecker;

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
        _session_id: &'a str,
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
