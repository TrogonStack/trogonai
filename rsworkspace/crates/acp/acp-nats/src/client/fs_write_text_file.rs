use crate::client::rpc_reply;
use crate::client_handler::ClientHandler;
use crate::nats::{FlushClient, PublishClient};
use crate::wire::{decode_request_params, response_id_from_request_headers};
use agent_client_protocol::ErrorCode;
use agent_client_protocol::schema::v1::{WriteTextFileRequest, WriteTextFileResponse};
use async_nats::header::HeaderMap;
use serde::de::Error as SerdeDeError;
use tracing::{instrument, warn};
use trogon_semconv::span::ACP_CLIENT_FS_WRITE_TEXT_FILE;

#[derive(Debug, thiserror::Error)]
pub enum FsWriteTextFileError {
    #[error("invalid request: {0}")]
    InvalidRequest(#[source] serde_json::Error),
    #[error("client error: {0}")]
    ClientError(#[source] agent_client_protocol::Error),
}

pub fn error_code_and_message(e: &FsWriteTextFileError) -> (ErrorCode, String) {
    match e {
        FsWriteTextFileError::InvalidRequest(inner) => (
            ErrorCode::InvalidParams,
            format!("Invalid write_text_file request: {}", inner),
        ),
        FsWriteTextFileError::ClientError(inner) => (inner.code, inner.message.clone()),
    }
}

/// Handles write_text_file: decodes wire request params, calls client, and publishes a wire-encoded reply.
#[instrument(name = ACP_CLIENT_FS_WRITE_TEXT_FILE, skip(headers, payload, client, nats))]
pub async fn handle<N: PublishClient + FlushClient, C: ClientHandler + Sync>(
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
                "write_text_file requires reply subject; ignoring message"
            );
            return;
        }
    };

    let response_id = response_id_from_request_headers(headers);
    match forward_to_client(headers, payload, client, session_id).await {
        Ok(response) => {
            rpc_reply::publish_success_reply(nats, reply_to, response_id, &response, "fs_write_text_file reply").await;
        }
        Err(e) => {
            let (code, message) = error_code_and_message(&e);
            warn!(
                error = %e,
                session_id = %session_id,
                "Failed to handle fs_write_text_file"
            );
            rpc_reply::publish_error_reply(
                nats,
                reply_to,
                response_id,
                code,
                &message,
                "fs_write_text_file error reply",
            )
            .await;
        }
    }
}

async fn forward_to_client<C: ClientHandler + Sync>(
    headers: &HeaderMap,
    payload: &[u8],
    client: &C,
    expected_session_id: &str,
) -> Result<WriteTextFileResponse, FsWriteTextFileError> {
    let request: WriteTextFileRequest = decode_request_params("fs/write_text_file", headers, payload).map_err(|e| {
        FsWriteTextFileError::InvalidRequest(serde_json::Error::custom(format!(
            "Invalid write_text_file request: {e}"
        )))
    })?;
    let params_session_id = request.session_id.to_string();
    if params_session_id != expected_session_id {
        return Err(FsWriteTextFileError::InvalidRequest(serde_json::Error::custom(
            format!(
                "params.sessionId ({}) does not match subject session id ({})",
                params_session_id, expected_session_id
            ),
        )));
    }
    client
        .write_text_file(request)
        .await
        .map_err(FsWriteTextFileError::ClientError)
}

#[cfg(test)]
mod tests;
