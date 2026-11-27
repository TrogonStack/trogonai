//! SSH sessions on subprocess leases.
//!
//! The gateway's stream bridge needs two things from a worker: whether it holds
//! a lease, and a way to run terminal requests. The subprocess worker already
//! has both in [`SubprocessTerminals`], so an SSH shell or exec is one of its
//! sandboxed terminals.

use std::sync::Arc;

use async_trait::async_trait;
use trogon_sandbox_contract::{LeaseId, TerminalError, TerminalRequest, TerminalResponse};
use trogon_sandbox_ssh_gateway::SshTerminalHost;

use crate::terminal::SubprocessTerminals;

pub struct SubprocessSshHost(pub Arc<SubprocessTerminals>);

#[async_trait]
impl SshTerminalHost for SubprocessSshHost {
    async fn holds(&self, lease_id: &LeaseId) -> bool {
        self.0.holds(lease_id)
    }

    async fn terminal(&self, req: &TerminalRequest) -> TerminalResponse {
        let body = match serde_json::to_vec(req) {
            Ok(body) => body,
            Err(e) => {
                return TerminalResponse::Failed(TerminalError::InvalidRequest { reason: e.to_string() });
            }
        };
        self.0
            .handle(&body)
            .await
            .unwrap_or(TerminalResponse::Failed(TerminalError::NotFound))
    }
}
