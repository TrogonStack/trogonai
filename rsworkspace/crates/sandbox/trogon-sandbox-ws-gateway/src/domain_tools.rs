use serde_json::Value;

use crate::encode::encode;
use crate::resource_payloads;
use crate::wait_until_answering::wait_until_answering;
use crate::{DispatchError, ToolContext};

pub(crate) async fn call(ctx: &ToolContext, tool: &str, payload: &Value) -> Result<Option<Value>, DispatchError> {
    if tool != "sandbox_get_domain" {
        return Ok(None);
    }
    let (lease_id, port, wait_ms) = resource_payloads::domain(payload)?;
    let first = ctx.backend.get_sandbox_domain(ctx.api_key(), &lease_id, port).await?;
    let resolved = match wait_ms.filter(|wait| *wait > 0) {
        Some(wait) => {
            wait_until_answering(first, wait, || {
                ctx.backend.get_sandbox_domain(ctx.api_key(), &lease_id, port)
            })
            .await?
        }
        None => first,
    };
    let response = match &ctx.preview {
        Some(preview) => {
            preview
                .decorate(&ctx.session, ctx.sessions.as_ref(), &lease_id, port, resolved)
                .await?
        }
        None => resolved,
    };
    Ok(Some(encode(response)?))
}
