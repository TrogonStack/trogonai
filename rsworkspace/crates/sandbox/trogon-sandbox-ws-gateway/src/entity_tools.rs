use serde_json::Value;

use crate::encode::encode;
use crate::resource_payloads;
use crate::{DispatchError, ToolContext};

pub(crate) async fn call(ctx: &ToolContext, tool: &str, payload: &Value) -> Result<Option<Value>, DispatchError> {
    let value = match tool {
        "sandbox_get_entity" => {
            let entity = resource_payloads::entity_ref(payload)?;
            encode(ctx.backend.get_entity(ctx.api_key(), ctx.tenant(), entity).await?)?
        }
        "sandbox_list_entities" => {
            let request = resource_payloads::list_entities(payload, &ctx.tenant())?;
            encode(ctx.backend.list_entities(ctx.api_key(), request).await?)?
        }
        "sandbox_delete_entity" => {
            let entity = resource_payloads::entity_ref(payload)?;
            encode(ctx.backend.delete_entity(ctx.api_key(), ctx.tenant(), entity).await?)?
        }
        "sandbox_get_lease_by_name" => {
            let name = resource_payloads::name(payload)?;
            encode(ctx.backend.get_lease_by_name(ctx.api_key(), ctx.tenant(), name).await?)?
        }
        "sandbox_delete_named_sandbox" => {
            let name = resource_payloads::name(payload)?;
            let response = ctx
                .backend
                .delete_named_sandbox(ctx.api_key(), ctx.tenant(), name)
                .await?;
            encode(response)?
        }
        _ => return Ok(None),
    };
    Ok(Some(value))
}
