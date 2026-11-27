use std::sync::Arc;
use std::time::Duration;

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD;
use futures::StreamExt;
use rmcp::handler::server::wrapper::{Json, Parameters};
use rmcp::{Peer, RoleServer, tool, tool_router};
use trogon_sandbox_contract::{LeaseReleased, LeaseRequest, ListLeasesRequest, ReapLeasesRequest, ReapMode};
use trogon_sandbox_protocol::subjects;

use super::SandboxMcpServer;
use super::evidence::notify_evidence;
use crate::ack::Ack;
use crate::constants::LEASE_REQUEST_TIMEOUT;
use crate::inputs::lease_inputs::{
    EvidenceNotification, ExtendTimeoutInput, ExtendTimeoutOutput, LeaseIdInput, LimitsInput, ListCapabilitiesInput,
    ListLeasesInput, ReapLeasesInput, SubscribeEvidenceOutput, WaitReleasedInput,
};
use crate::parse;
use crate::reply::{JsonReply, json_reply};
use crate::tool_error::ToolError;

#[tool_router(router = lease_router, vis = "pub(super)")]
impl SandboxMcpServer {
    #[tool(
        description = "Request a sandbox lease from the NATS broker. Input mirrors trogon_sandbox_contract::LeaseRequest; tenant is taken from the server --tenant flag."
    )]
    async fn sandbox_lease(&self, params: Parameters<serde_json::Map<String, serde_json::Value>>) -> JsonReply {
        self.state.require_auth()?;
        let mut request: LeaseRequest =
            serde_json::from_value(params.0.into()).map_err(|e| ToolError::invalid("lease request", e))?;
        request.tenant = self.state.tenant().clone();
        let granted = self.client()?.lease(&request, LEASE_REQUEST_TIMEOUT).await?;
        json_reply(&granted)
    }

    #[tool(description = "Cancel an active sandbox lease")]
    async fn sandbox_cancel(&self, params: Parameters<LeaseIdInput>) -> Result<Json<Ack>, ToolError> {
        self.state.require_auth()?;
        self.client()?.cancel(&parse::lease_id(params.0.lease_id)).await?;
        Ok(Ack::reply())
    }

    #[tool(description = "Wait until a lease is released or the timeout elapses")]
    async fn sandbox_wait_released(&self, params: Parameters<WaitReleasedInput>) -> JsonReply {
        self.state.require_auth()?;
        let lease_id = parse::lease_id(params.0.lease_id);
        let timeout = Duration::from_secs(u64::from(params.0.timeout_seconds.max(1)));
        let reason = self.client()?.wait_released_with_reason(&lease_id, timeout).await?;
        json_reply(&LeaseReleased {
            lease_id,
            tenant: self.state.tenant().clone(),
            reason,
            snapshot_out: None,
            worker_id: None,
            correlation_id: None,
            restored_from: None,
            released_at: None,
        })
    }

    #[tool(description = "List sandbox leases owned by the caller's tenant, paginated by an opaque cursor")]
    async fn sandbox_list_leases(&self, params: Parameters<ListLeasesInput>) -> JsonReply {
        self.state.require_auth()?;
        let request = ListLeasesRequest {
            tenant: self.state.tenant().clone(),
            status_filter: parse::status_filter(params.0.status_filter)?,
            cursor: params.0.cursor,
            limit: params.0.limit,
            tags: parse::tags(params.0.tags)?,
        };
        json_reply(&self.client()?.list_leases(request).await?)
    }

    #[tool(
        description = "Release the caller's tenant leases that match every given filter (tags, older_than_seconds, idle_for_seconds). At least one filter is required. Use dry_run to preview. Safe to repeat."
    )]
    async fn sandbox_reap_leases(&self, params: Parameters<ReapLeasesInput>) -> JsonReply {
        self.state.require_auth()?;
        let input = params.0;
        let request = ReapLeasesRequest::new(
            self.state.tenant().clone(),
            parse::tags(input.tags)?,
            input.older_than_seconds,
            input.idle_for_seconds,
            ReapMode::from(input.dry_run),
        )
        .map_err(|e| ToolError::invalid("filters", e))?;
        json_reply(&self.client()?.reap_leases(request).await?)
    }

    #[tool(
        description = "List provider capability profiles from the broker registry. Set `provider` (e.g. \"mock\", \"hyperlight\", \"k8s\") to filter to a single provider, or omit/null to list every provider currently reachable via a live worker heartbeat."
    )]
    async fn sandbox_list_capabilities(&self, params: Parameters<ListCapabilitiesInput>) -> JsonReply {
        self.state.require_auth()?;
        let provider = params
            .0
            .provider
            .map(|name| subjects::parse_provider_kind(&name).map_err(|e| ToolError::invalid("provider", e)))
            .transpose()?;
        json_reply(&self.client()?.list_capabilities(provider).await?)
    }

    #[tool(
        description = "Subscribe to evidence events for a lease. Evidence frames are delivered as MCP logging notifications (logger: sandbox.evidence)."
    )]
    async fn sandbox_subscribe_evidence(
        &self,
        peer: Peer<RoleServer>,
        params: Parameters<LeaseIdInput>,
    ) -> Result<Json<SubscribeEvidenceOutput>, ToolError> {
        self.state.require_auth()?;
        let lease_id = parse::lease_id(params.0.lease_id);
        let mut stream = self.client()?.evidence_stream(&lease_id).await?;
        let lease_text = Arc::<str>::from(lease_id.as_str());
        let announced = lease_text.to_string();
        tokio::spawn(async move {
            while let Some(Ok(frame)) = stream.next().await {
                let notification = EvidenceNotification {
                    lease_id: lease_text.to_string(),
                    subject_suffix: frame.subject_suffix,
                    payload_base64: STANDARD.encode(&frame.payload),
                };
                let data = serde_json::to_value(notification).unwrap_or_default();
                if notify_evidence(&peer, data).await.is_err() {
                    break;
                }
            }
        });
        Ok(Json(SubscribeEvidenceOutput {
            subscribed: true,
            lease_id: announced,
            evidence_subject: subjects::evidence_subject(&lease_id),
        }))
    }

    #[tool(
        description = "Extend the wall-clock deadline of a running lease. Mirrors trogon_sandbox_contract::ExtendLeaseTimeoutRequest."
    )]
    async fn sandbox_extend_timeout(
        &self,
        params: Parameters<ExtendTimeoutInput>,
    ) -> Result<Json<ExtendTimeoutOutput>, ToolError> {
        self.state.require_auth()?;
        let lease_id = parse::lease_id(params.0.lease_id);
        let new_deadline = self
            .client()?
            .extend_timeout(&lease_id, params.0.additional_seconds)
            .await?;
        Ok(Json(ExtendTimeoutOutput { new_deadline }))
    }

    #[tool(
        description = "Read-only usage counters for a lease: commands run, command output bytes, file bytes in and out, running and paused seconds, and cpu and memory gauges when the provider reports them. A released lease returns its final figures. Mirrors trogon_sandbox_contract::GetLeaseUsageRequest."
    )]
    async fn sandbox_usage(&self, params: Parameters<LeaseIdInput>) -> JsonReply {
        self.state.require_auth()?;
        let lease_id = parse::lease_id(params.0.lease_id);
        json_reply(&self.client()?.usage(self.state.tenant().clone(), &lease_id).await?)
    }

    #[tool(
        description = "Read-only account limits for the caller's tenant next to current consumption: live leases against the lease cap, monthly cpu and wall budgets, per-request cpu and memory caps, and any per-tag lease cap with its live counts. Use it to check capacity before creating a lease. Mirrors trogon_sandbox_contract::GetAccountLimitsRequest."
    )]
    async fn sandbox_limits(&self, _params: Parameters<LimitsInput>) -> JsonReply {
        self.state.require_auth()?;
        json_reply(&self.client()?.limits(self.state.tenant().clone()).await?)
    }
}
