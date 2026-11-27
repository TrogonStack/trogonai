use rmcp::handler::server::wrapper::{Json, Parameters};
use rmcp::{tool, tool_router};
use trogon_sandbox_contract::{Digest, NetworkPolicy};

use super::SandboxMcpServer;
use super::lifecycle_output::{pause_output, resume_output, suspend_output, wake_output};
use crate::ack::Ack;
use crate::inputs::lease_inputs::LeaseIdInput;
use crate::inputs::lifecycle_inputs::{
    NetworkPolicyUpdateInput, PauseLeaseInput, PauseLeaseOutput, ResumeLeaseOutput, SnapshotDeleteInput,
    SuspendLeaseInput, SuspendLeaseOutput, WakeLeaseOutput,
};
use crate::parse;
use crate::reply::{JsonReply, json_reply};
use crate::tool_error::ToolError;

#[tool_router(router = lifecycle_router, vis = "pub(super)")]
impl SandboxMcpServer {
    #[tool(
        description = "Permanently delete a lease, purging resumable snapshot state. Mirrors trogon_sandbox_contract::DeleteLeaseRequest."
    )]
    async fn sandbox_delete_lease(&self, params: Parameters<LeaseIdInput>) -> Result<Json<Ack>, ToolError> {
        self.state.require_auth()?;
        let lease_id = parse::lease_id(params.0.lease_id);
        self.client()?
            .delete_lease(self.state.tenant().clone(), &lease_id)
            .await?;
        Ok(Ack::reply())
    }

    #[tool(
        description = "Permanently stop a lease's current session while keeping it resumable later. Mirrors trogon_sandbox_contract::StopLeaseRequest."
    )]
    async fn sandbox_stop_lease(&self, params: Parameters<LeaseIdInput>) -> Result<Json<Ack>, ToolError> {
        self.state.require_auth()?;
        let lease_id = parse::lease_id(params.0.lease_id);
        self.client()?
            .stop_lease(self.state.tenant().clone(), &lease_id)
            .await?;
        Ok(Ack::reply())
    }

    #[tool(
        description = "Pause a running lease in place, keeping its id, files and processes while stopping CPU use where the provider can. The lease is not reaped while paused, up to max_paused_seconds. Idempotent. Returns outcome \"unsupported\" when the provider cannot pause. Mirrors trogon_sandbox_contract::PauseLeaseRequest."
    )]
    async fn sandbox_pause_lease(
        &self,
        params: Parameters<PauseLeaseInput>,
    ) -> Result<Json<PauseLeaseOutput>, ToolError> {
        self.state.require_auth()?;
        let lease_id = parse::lease_id(params.0.lease_id);
        let response = self
            .client()?
            .pause_lease(self.state.tenant().clone(), &lease_id, params.0.max_paused_seconds)
            .await?;
        Ok(Json(pause_output(response)))
    }

    #[tool(
        description = "Snapshot a running lease without stopping it. Returns outcome \"created\" with a receipt whose reference seeds a new lease as a snapshot image, or \"unsupported\" when the provider cannot checkpoint a running lease. Mirrors trogon_sandbox_contract::SnapshotLeaseRequest."
    )]
    async fn sandbox_snapshot_lease(&self, params: Parameters<LeaseIdInput>) -> JsonReply {
        self.state.require_auth()?;
        let lease_id = parse::lease_id(params.0.lease_id);
        json_reply(
            &self
                .client()?
                .snapshot_lease(self.state.tenant().clone(), &lease_id)
                .await?,
        )
    }

    #[tool(
        description = "Delete a snapshot by digest. Returns outcome \"deleted\", \"not_found\" (also for a repeat) or \"unsupported\". Mirrors trogon_sandbox_contract::SnapshotDeleteRequest."
    )]
    async fn sandbox_snapshot_delete(&self, params: Parameters<SnapshotDeleteInput>) -> JsonReply {
        self.state.require_auth()?;
        let digest = Digest::new(params.0.digest).map_err(|e| ToolError::invalid("digest", e))?;
        json_reply(
            &self
                .client()?
                .snapshot_delete(self.state.tenant().clone(), digest)
                .await?,
        )
    }

    #[tool(
        description = "Resume a paused lease with its original remaining time. Idempotent. Returns outcome \"unsupported\" when the provider cannot pause. Mirrors trogon_sandbox_contract::ResumeLeaseRequest."
    )]
    async fn sandbox_resume_lease(
        &self,
        params: Parameters<LeaseIdInput>,
    ) -> Result<Json<ResumeLeaseOutput>, ToolError> {
        self.state.require_auth()?;
        let lease_id = parse::lease_id(params.0.lease_id);
        let response = self
            .client()?
            .resume_lease(self.state.tenant().clone(), &lease_id)
            .await?;
        Ok(Json(resume_output(response)))
    }

    #[tool(
        description = "Suspend a lease: release its host resources while keeping its id and state, which costs less than pause. level is auto (default), memory (processes continue after wake) or disk (only the disk survives); memory and disk must match what the provider offers. Idempotent. Fails with suspend_unsupported when the provider cannot suspend to that level. Mirrors trogon_sandbox_contract::SuspendLeaseRequest."
    )]
    async fn sandbox_suspend_lease(
        &self,
        params: Parameters<SuspendLeaseInput>,
    ) -> Result<Json<SuspendLeaseOutput>, ToolError> {
        self.state.require_auth()?;
        let lease_id = parse::lease_id(params.0.lease_id);
        let level = parse::suspend_level(params.0.level)?;
        let response = self
            .client()?
            .suspend_lease(
                self.state.tenant().clone(),
                &lease_id,
                level,
                params.0.max_suspended_seconds,
            )
            .await?;
        suspend_output(response).map(Json)
    }

    #[tool(
        description = "Wake a suspended lease with the run time it had left. Idempotent: a running lease is returned unchanged. Fails with lease_waking, which carries a retry delay, while another wake is in flight. Mirrors trogon_sandbox_contract::WakeLeaseRequest."
    )]
    async fn sandbox_wake_lease(&self, params: Parameters<LeaseIdInput>) -> Result<Json<WakeLeaseOutput>, ToolError> {
        self.state.require_auth()?;
        let lease_id = parse::lease_id(params.0.lease_id);
        let response = self
            .client()?
            .wake_lease(self.state.tenant().clone(), &lease_id)
            .await?;
        wake_output(response).map(Json)
    }

    #[tool(
        description = "Replace the egress network_policy of an already-running lease in place, without restarting or re-provisioning the sandbox. Mirrors trogon_sandbox_contract::NetworkPolicyUpdateRequest."
    )]
    async fn sandbox_update_network_policy(
        &self,
        params: Parameters<NetworkPolicyUpdateInput>,
    ) -> Result<Json<Ack>, ToolError> {
        self.state.require_auth()?;
        let lease_id = parse::lease_id(params.0.lease_id);
        let network_policy: NetworkPolicy =
            serde_json::from_value(params.0.network_policy).map_err(|e| ToolError::invalid("network_policy", e))?;
        self.client()?.update_network_policy(&lease_id, network_policy).await?;
        Ok(Ack::reply())
    }
}
