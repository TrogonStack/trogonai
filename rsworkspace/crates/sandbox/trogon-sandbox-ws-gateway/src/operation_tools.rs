use serde::Deserialize;
use serde_json::Value;
use trogon_sandbox_contract::{CancelOperationRequest, CreatedLeaseCompensation, GetOperationResponse, OperationId};

use crate::encode::encode;
use crate::from_payload::from_payload;
use crate::{DispatchError, PayloadError, ToolContext};

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct OperationPayload {
    operation_id: OperationId,
    #[serde(default)]
    compensate_created: CreatedLeaseCompensation,
}

pub(crate) async fn call(ctx: &ToolContext, tool: &str, payload: &Value) -> Result<Option<Value>, DispatchError> {
    let value = match tool {
        "sandbox_operation_capabilities" => operation_capabilities(ctx, payload).await?,
        "sandbox_cancel_operation" => cancel_operation(ctx, payload).await?,
        "sandbox_get_operation" => get_operation(ctx, payload).await?,
        _ => return Ok(None),
    };
    Ok(Some(value))
}

async fn operation_capabilities(ctx: &ToolContext, payload: &Value) -> Result<Value, DispatchError> {
    if !payload.as_object().is_some_and(serde_json::Map::is_empty) {
        return Err(PayloadError::NonEmptyCapabilitiesPayload.into());
    }
    encode(ctx.backend.operation_capabilities(ctx.api_key(), ctx.tenant()).await?)
}

async fn cancel_operation(ctx: &ToolContext, payload: &Value) -> Result<Value, DispatchError> {
    let operation: OperationPayload = from_payload(payload)?;
    let request = CancelOperationRequest {
        tenant: ctx.tenant(),
        operation_id: operation.operation_id,
        compensate_created: operation.compensate_created,
    };
    encode(ctx.backend.cancel_operation(ctx.api_key(), request).await?)
}

async fn get_operation(ctx: &ToolContext, payload: &Value) -> Result<Value, DispatchError> {
    if payload.get("compensate_created").is_some() {
        return Err(PayloadError::CompensateOnLookup.into());
    }
    let operation: OperationPayload = from_payload(payload)?;
    let found = ctx
        .backend
        .get_operation(ctx.api_key(), ctx.tenant(), &operation.operation_id)
        .await?;
    encode(match found {
        Some(operation) => GetOperationResponse::Found {
            operation: Box::new(operation),
        },
        None => GetOperationResponse::NotFound,
    })
}
