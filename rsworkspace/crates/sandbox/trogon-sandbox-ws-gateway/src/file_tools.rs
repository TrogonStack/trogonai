use serde_json::Value;
use trogon_sandbox_client::ClientError;
use trogon_sandbox_contract::{
    CreateSandboxGroupRequest, CreateSandboxUserRequest, FileOpRequest, FileOpResponse, ModifyGroupMembershipRequest,
    ReadFileRequest, WriteFilesRequest,
};

use crate::encode::encode;
use crate::from_payload::from_payload;
use crate::response_bodies::{OutcomeBody, ReadFileBody};
use crate::{DispatchError, ToolContext};

pub(crate) async fn call(ctx: &ToolContext, tool: &str, payload: &Value) -> Result<Option<Value>, DispatchError> {
    let value = match tool {
        "sandbox_write_files" => {
            let request: WriteFilesRequest = from_payload(payload)?;
            ctx.backend
                .write_files(ctx.api_key(), &request.lease_id, request.files, request.as_user)
                .await?;
            encode(OutcomeBody { outcome: "written" })?
        }
        "sandbox_read_file" => read_file(ctx, payload).await?,
        "sandbox_file_op" => {
            let request: FileOpRequest = from_payload(payload)?;
            encode(file_op_outcome(ctx.backend.file_op(ctx.api_key(), request).await)?)?
        }
        "sandbox_create_user" => {
            let request: CreateSandboxUserRequest = from_payload(payload)?;
            let response = ctx
                .backend
                .create_user(ctx.api_key(), &request.lease_id, request.username)
                .await?;
            encode(response)?
        }
        "sandbox_create_group" => {
            let request: CreateSandboxGroupRequest = from_payload(payload)?;
            let response = ctx
                .backend
                .create_group(ctx.api_key(), &request.lease_id, request.groupname)
                .await?;
            encode(response)?
        }
        "sandbox_modify_group_membership" => {
            let request: ModifyGroupMembershipRequest = from_payload(payload)?;
            ctx.backend
                .modify_group_membership(
                    ctx.api_key(),
                    &request.lease_id,
                    request.username,
                    request.groupname,
                    request.action,
                )
                .await?;
            encode(OutcomeBody { outcome: "applied" })?
        }
        _ => return Ok(None),
    };
    Ok(Some(value))
}

async fn read_file(ctx: &ToolContext, payload: &Value) -> Result<Value, DispatchError> {
    let request: ReadFileRequest = from_payload(payload)?;
    match ctx
        .backend
        .read_file(ctx.api_key(), &request.lease_id, request.path, request.as_user)
        .await
    {
        Ok(data) => encode(ReadFileBody::Content { data }),
        Err(ClientError::FileNotFound(_)) => encode(ReadFileBody::NotFound),
        Err(error) => Err(error.into()),
    }
}

/// A typed refusal (unsupported, not found, a failed operation) is a normal outcome the caller branches on, so it
/// travels as an `ok` frame with its `outcome` tag instead of becoming a transport error.
pub(crate) fn file_op_outcome(result: Result<FileOpResponse, ClientError>) -> Result<FileOpResponse, DispatchError> {
    match result {
        Err(ClientError::FileOpUnsupported(message)) => Ok(FileOpResponse::Unsupported { message }),
        Err(ClientError::FileNotFound(path)) => Ok(FileOpResponse::NotFound { path }),
        Err(ClientError::FileOpFailed { kind, message }) => Ok(FileOpResponse::Failed { kind, message }),
        other => other.map_err(DispatchError::from),
    }
}

#[cfg(test)]
mod tests;
