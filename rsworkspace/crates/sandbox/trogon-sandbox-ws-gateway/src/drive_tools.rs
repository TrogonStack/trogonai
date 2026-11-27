use serde_json::Value;

use crate::encode::encode;
use crate::resource_payloads;
use crate::response_bodies::{DeletedBody, DrivesBody};
use crate::{DispatchError, ToolContext};

pub(crate) async fn call(ctx: &ToolContext, tool: &str, payload: &Value) -> Result<Option<Value>, DispatchError> {
    let value = match tool {
        "sandbox_create_drive" => {
            let (drive_id, size_gib, storage) = resource_payloads::create_drive(payload, &ctx.tenant())?;
            let record = ctx
                .backend
                .create_drive(ctx.api_key(), ctx.tenant(), drive_id, size_gib, storage)
                .await?;
            encode(record)?
        }
        "sandbox_list_drives" => {
            let drives = ctx.backend.list_drives(ctx.api_key(), ctx.tenant()).await?;
            encode(DrivesBody { drives })?
        }
        "sandbox_delete_drive" => {
            let drive_id = resource_payloads::drive_id(payload, &ctx.tenant())?;
            ctx.backend.delete_drive(ctx.api_key(), ctx.tenant(), drive_id).await?;
            encode(DeletedBody { deleted: true })?
        }
        _ => return Ok(None),
    };
    Ok(Some(value))
}
