use std::sync::Arc;

use async_nats::Client;
use trogon_sandbox_contract::{ReadFileRequest, ReadFileResponse, WriteFilesRequest, WriteFilesResponse};
use trogon_sandbox_protocol::nats_transfer;

use super::encode_within::encode_read_within;
use super::handler_error::HandlerError;
use super::{accept, backend};
use crate::adapter::SubstrateProvider;

pub(super) async fn handle_write_files_msg(
    mut msg: async_nats::Message,
    client: Client,
    provider: Arc<SubstrateProvider>,
) -> Result<(), HandlerError> {
    msg.payload = nats_transfer::receive(&client, &msg).await?;
    let Some((reply, request)) = accept::<WriteFilesRequest>(&msg, "write_files") else {
        return Ok(());
    };

    let Some(agent) = provider.agent_for(&request.lease_id).await else {
        return Ok(());
    };

    let response = agent
        .write_files(&request)
        .await
        .unwrap_or_else(|e| WriteFilesResponse::Denied(backend(&e)));

    nats_transfer::respond(&client, reply.as_str(), serde_json::to_vec(&response)?.into()).await?;
    Ok(())
}

pub(super) async fn handle_read_file_msg(
    mut msg: async_nats::Message,
    client: Client,
    provider: Arc<SubstrateProvider>,
) -> Result<(), HandlerError> {
    msg.payload = nats_transfer::receive(&client, &msg).await?;
    let Some((reply, request)) = accept::<ReadFileRequest>(&msg, "read_file") else {
        return Ok(());
    };

    let Some(agent) = provider.agent_for(&request.lease_id).await else {
        return Ok(());
    };

    let response = agent
        .read_file(&request)
        .await
        .unwrap_or_else(|e| ReadFileResponse::Denied(backend(&e)));

    let body = encode_read_within(response, &request.path, nats_transfer::MAX_TRANSFER_BYTES)?;
    nats_transfer::respond(&client, reply.as_str(), body.into()).await?;
    Ok(())
}
