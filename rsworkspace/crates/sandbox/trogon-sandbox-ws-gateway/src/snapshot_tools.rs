use serde_json::Value;

use crate::encode::encode;
use crate::resource_payloads;
use crate::response_bodies::OutcomeBody;
use crate::{DispatchError, ToolContext};

pub(crate) async fn call(ctx: &ToolContext, tool: &str, payload: &Value) -> Result<Option<Value>, DispatchError> {
    let value = match tool {
        "sandbox_snapshot_delete" => {
            let digest = resource_payloads::digest(payload)?;
            encode(ctx.backend.snapshot_delete(ctx.api_key(), ctx.tenant(), digest).await?)?
        }
        "sandbox_snapshot_get" => {
            let digest = resource_payloads::digest(payload)?;
            encode(ctx.backend.snapshot_get(ctx.api_key(), ctx.tenant(), digest).await?)?
        }
        "sandbox_snapshot_tree" => {
            let digest = resource_payloads::digest(payload)?;
            encode(ctx.backend.snapshot_tree(ctx.api_key(), ctx.tenant(), digest).await?)?
        }
        "sandbox_set_snapshot_tag" => {
            let (name, digest) = resource_payloads::snapshot_tag(payload)?;
            ctx.backend
                .set_snapshot_tag(ctx.api_key(), ctx.tenant(), name, digest)
                .await?;
            encode(OutcomeBody { outcome: "applied" })?
        }
        "sandbox_delete_snapshot_tag" => {
            let name = resource_payloads::name(payload)?;
            let response = ctx
                .backend
                .delete_snapshot_tag(ctx.api_key(), ctx.tenant(), name)
                .await?;
            encode(response)?
        }
        "sandbox_snapshot_tag_history" => {
            let name = resource_payloads::name(payload)?;
            let response = ctx
                .backend
                .snapshot_tag_history(ctx.api_key(), ctx.tenant(), name)
                .await?;
            encode(response)?
        }
        _ => return Ok(None),
    };
    Ok(Some(value))
}
