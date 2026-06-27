use crate::client::rpc_reply;
use crate::nats::{FlushClient, PublishClient};
use crate::wire::{decode_request_params, response_id_from_request_headers};
use agent_client_protocol::{Client, ErrorCode, ReadTextFileRequest, ReadTextFileResponse};
use async_nats::header::HeaderMap;
use tracing::{instrument, warn};
use trogon_semconv::span::ACP_CLIENT_FS_READ_TEXT_FILE;

#[derive(Debug, thiserror::Error)]
pub enum FsReadTextFileError {
    #[error("invalid request: {0}")]
    InvalidRequest(String),
    #[error("client error: {0}")]
    ClientError(#[source] agent_client_protocol::Error),
}

pub fn error_code_and_message(e: &FsReadTextFileError) -> (ErrorCode, String) {
    match e {
        FsReadTextFileError::InvalidRequest(message) => (ErrorCode::InvalidParams, message.clone()),
        FsReadTextFileError::ClientError(inner) => (inner.code, inner.message.clone()),
    }
}

/// Handles read_text_file: decodes wire request params, calls client, and publishes a wire-encoded reply.
#[instrument(name = ACP_CLIENT_FS_READ_TEXT_FILE, skip(headers, payload, client, nats))]
pub async fn handle<N: PublishClient + FlushClient, C: Client>(
    headers: &HeaderMap,
    payload: &[u8],
    client: &C,
    reply: Option<&str>,
    nats: &N,
    session_id: &str,
) {
    let reply_to = match reply {
        Some(r) => r,
        None => {
            warn!(
                session_id = %session_id,
                "read_text_file requires reply subject; ignoring message"
            );
            return;
        }
    };

    let response_id = response_id_from_request_headers(headers);
    match forward_to_client(headers, payload, client).await {
        Ok(response) => {
            rpc_reply::publish_success_reply(nats, reply_to, response_id, &response, "fs_read_text_file reply").await;
        }
        Err(e) => {
            let (code, message) = error_code_and_message(&e);
            warn!(
                error = %e,
                session_id = %session_id,
                "Failed to handle fs_read_text_file"
            );
            rpc_reply::publish_error_reply(
                nats,
                reply_to,
                response_id,
                code,
                &message,
                "fs_read_text_file error reply",
            )
            .await;
        }
    }
}

async fn forward_to_client<C: Client>(
    headers: &HeaderMap,
    payload: &[u8],
    client: &C,
) -> Result<ReadTextFileResponse, FsReadTextFileError> {
    let request: ReadTextFileRequest = decode_request_params("fs/read_text_file", headers, payload)
        .map_err(|e| FsReadTextFileError::InvalidRequest(format!("Invalid read_text_file request: {e}")))?;
    client
        .read_text_file(request)
        .await
        .map_err(FsReadTextFileError::ClientError)
}

#[cfg(test)]
mod tests;
