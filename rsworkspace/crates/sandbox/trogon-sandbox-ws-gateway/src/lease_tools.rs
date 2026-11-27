use serde::Deserialize;
use serde_json::Value;
use trogon_sandbox_contract::{
    ExtendLeaseTimeoutRequest, LeaseId, LeaseRequest, NativeSnapshotRef, NetworkPolicyUpdateRequest,
};

use crate::constants::{DEFAULT_LEASE_TIMEOUT, DEFAULT_WAIT_TIMEOUT};
use crate::encode::encode;
use crate::from_payload::{from_payload, timeout_or};
use crate::lease_payloads;
use crate::response_bodies::{CancelledBody, ExtendedBody, OutcomeBody, ReleasedBody, UpdatedBody};
use crate::{DispatchError, ToolContext};

pub(crate) async fn call(ctx: &ToolContext, tool: &str, payload: &Value) -> Result<Option<Value>, DispatchError> {
    let value = match tool {
        "sandbox_lease" => lease(ctx, payload).await?,
        "sandbox_cancel" => cancel(ctx, payload).await?,
        "sandbox_update_tags" => update_tags(ctx, payload).await?,
        "sandbox_wait_released" => wait_released(ctx, payload).await?,
        "sandbox_stop_lease" => {
            let lease_id = lease_payloads::lease_id(payload)?;
            ctx.backend.stop_lease(ctx.api_key(), ctx.tenant(), &lease_id).await?;
            encode(OutcomeBody { outcome: "stopped" })?
        }
        "sandbox_delete_lease" => {
            let lease_id = lease_payloads::lease_id(payload)?;
            ctx.backend.delete_lease(ctx.api_key(), ctx.tenant(), &lease_id).await?;
            encode(OutcomeBody { outcome: "deleted" })?
        }
        "sandbox_pause_lease" => {
            let (lease_id, max_paused_seconds) = lease_payloads::pause_lease(payload)?;
            let response = ctx
                .backend
                .pause_lease(ctx.api_key(), ctx.tenant(), &lease_id, max_paused_seconds)
                .await?;
            encode(response)?
        }
        "sandbox_suspend_lease" => {
            let (lease_id, level, max_suspended_seconds) = lease_payloads::suspend_lease(payload)?;
            let response = ctx
                .backend
                .suspend_lease(ctx.api_key(), ctx.tenant(), &lease_id, level, max_suspended_seconds)
                .await?;
            encode(response)?
        }
        "sandbox_wake_lease" => {
            let lease_id = lease_payloads::lease_id(payload)?;
            encode(ctx.backend.wake_lease(ctx.api_key(), ctx.tenant(), &lease_id).await?)?
        }
        "sandbox_resume_lease" => {
            let lease_id = lease_payloads::lease_id(payload)?;
            encode(ctx.backend.resume_lease(ctx.api_key(), ctx.tenant(), &lease_id).await?)?
        }
        "sandbox_snapshot_lease" => {
            let (lease_id, accept_native) = lease_payloads::snapshot_lease(payload)?;
            let response = ctx
                .backend
                .snapshot_lease(ctx.api_key(), ctx.tenant(), &lease_id, accept_native)
                .await?;
            encode(response)?
        }
        "sandbox_native_snapshot_delete" => native_snapshot_delete(ctx, payload).await?,
        "sandbox_usage" => {
            let lease_id = lease_payloads::lease_id(payload)?;
            encode(ctx.backend.lease_usage(ctx.api_key(), ctx.tenant(), &lease_id).await?)?
        }
        "sandbox_tenant_usage" => encode(ctx.backend.tenant_usage(ctx.api_key(), ctx.tenant()).await?)?,
        "sandbox_account_limits" => encode(ctx.backend.account_limits(ctx.api_key(), ctx.tenant()).await?)?,
        "sandbox_extend_lease_timeout" => {
            let request: ExtendLeaseTimeoutRequest = from_payload(payload)?;
            let new_deadline = ctx
                .backend
                .extend_lease_timeout(ctx.api_key(), &request.lease_id, request.additional_seconds)
                .await?;
            encode(ExtendedBody {
                outcome: "extended",
                new_deadline,
            })?
        }
        "sandbox_fork" => {
            let request = lease_payloads::fork_lease(payload, &ctx.tenant())?;
            let granted = ctx
                .backend
                .fork_lease(
                    ctx.api_key(),
                    request.tenant,
                    &request.source_lease_id,
                    request.ttl_seconds,
                    request.idempotency_key,
                )
                .await?;
            encode(granted)?
        }
        "sandbox_update_network_policy" => {
            let request: NetworkPolicyUpdateRequest = from_payload(payload)?;
            ctx.backend
                .update_network_policy(ctx.api_key(), &request.lease_id, request.network_policy)
                .await?;
            encode(OutcomeBody { outcome: "applied" })?
        }
        "sandbox_native_handle" => {
            let lease_id = lease_payloads::lease_id(payload)?;
            encode(ctx.backend.native_handle(ctx.api_key(), &lease_id).await?)?
        }
        "sandbox_ssh_access" => {
            let (lease_id, ttl_seconds) = lease_payloads::ssh_access(payload)?;
            encode(ctx.backend.ssh_access(ctx.api_key(), &lease_id, ttl_seconds).await?)?
        }
        _ => return Ok(None),
    };
    Ok(Some(value))
}

async fn lease(ctx: &ToolContext, payload: &Value) -> Result<Value, DispatchError> {
    let request: LeaseRequest = from_payload(payload)?;
    if request.tenant != ctx.session.tenant {
        return Err(DispatchError::TenantMismatch);
    }
    let timeout = timeout_or(payload, DEFAULT_LEASE_TIMEOUT);
    encode(ctx.backend.lease(ctx.api_key(), &request, timeout).await?)
}

async fn cancel(ctx: &ToolContext, payload: &Value) -> Result<Value, DispatchError> {
    let lease_id: LeaseId = lease_payloads::lease_id(payload)?;
    ctx.backend.cancel(ctx.api_key(), &lease_id).await?;
    encode(CancelledBody { cancelled: true })
}

async fn update_tags(ctx: &ToolContext, payload: &Value) -> Result<Value, DispatchError> {
    let (lease_id, tags) = lease_payloads::update_tags(payload)?;
    ctx.backend.update_tags(ctx.api_key(), &lease_id, tags).await?;
    encode(UpdatedBody { updated: true })
}

async fn wait_released(ctx: &ToolContext, payload: &Value) -> Result<Value, DispatchError> {
    let lease_id = lease_payloads::lease_id(payload)?;
    let timeout = timeout_or(payload, DEFAULT_WAIT_TIMEOUT);
    let reason = ctx.backend.wait_released(ctx.api_key(), &lease_id, timeout).await?;
    encode(ReleasedBody { reason })
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct NativeDeletePayload {
    reference: NativeSnapshotRef,
}

async fn native_snapshot_delete(ctx: &ToolContext, payload: &Value) -> Result<Value, DispatchError> {
    let request: NativeDeletePayload = from_payload(payload)?;
    encode(
        ctx.backend
            .native_snapshot_delete(ctx.api_key(), ctx.tenant(), request.reference)
            .await?,
    )
}
