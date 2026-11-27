use serde_json::Value;

use crate::constants::{STREAM_LEASES, STREAM_SNAPSHOTS, STREAM_WORKERS};
use crate::encode::encode;
use crate::lease_payloads;
use crate::response_bodies::{DashboardSubscribedBody, SubscribedBody};
use crate::{DispatchError, ToolContext};

pub(crate) async fn call(ctx: &ToolContext, tool: &str, payload: &Value) -> Result<Option<Value>, DispatchError> {
    let value = match tool {
        "sandbox_list_capabilities" => encode(ctx.backend.list_capabilities().await)?,
        "sandbox_list_leases" => {
            let filter = lease_payloads::tag_filter(payload)?;
            ctx.backend.list_leases(&ctx.session.tenant, &filter).await?
        }
        "sandbox_reap_leases" => {
            let request = lease_payloads::reap_leases(payload, &ctx.session.tenant)?;
            encode(ctx.backend.reap_leases(ctx.api_key(), request).await?)?
        }
        "sandbox_list_snapshots" => ctx.backend.list_snapshots(&ctx.session.tenant).await?,
        "sandbox_list_tags" => ctx.backend.list_tags(&ctx.session.tenant).await?,
        "sandbox_subscribe_dashboard" => {
            ctx.backend
                .spawn_dashboard_watchers(ctx.tenant(), ctx.events.clone(), ctx.dashboard_cancel.child_token());
            encode(DashboardSubscribedBody {
                subscribed: true,
                streams: [STREAM_LEASES, STREAM_SNAPSHOTS, STREAM_WORKERS],
            })?
        }
        "sandbox_subscribe_evidence" => {
            let lease_id = lease_payloads::lease_id(payload)?;
            ctx.backend
                .subscribe_evidence(ctx.session.api_key.clone(), lease_id, ctx.events.clone())
                .await?;
            encode(SubscribedBody { subscribed: true })?
        }
        _ => return Ok(None),
    };
    Ok(Some(value))
}
