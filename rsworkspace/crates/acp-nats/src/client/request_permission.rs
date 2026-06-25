use crate::client::rpc_reply;
use crate::nats::{FlushClient, PublishClient};
use crate::wire::{decode_request_params, response_id_from_request_headers};
use agent_client_protocol::{Client, ErrorCode, RequestPermissionRequest, RequestPermissionResponse};
use async_nats::header::HeaderMap;
use serde::de::Error as SerdeDeError;
use tracing::{instrument, warn};

#[derive(Debug, thiserror::Error)]
pub enum RequestPermissionError {
    #[error("invalid request: {0}")]
    InvalidRequest(#[source] serde_json::Error),
    #[error("client error: {0}")]
    ClientError(#[source] agent_client_protocol::Error),
}

pub fn error_code_and_message(e: &RequestPermissionError) -> (ErrorCode, String) {
    match e {
        RequestPermissionError::InvalidRequest(inner) => (
            ErrorCode::InvalidParams,
            format!("Invalid request_permission request: {}", inner),
        ),
        RequestPermissionError::ClientError(inner) => (inner.code, inner.message.clone()),
    }
}

/// Handles session/request_permission: decodes wire request params, calls client, and publishes
/// a wire-encoded reply. Reply is required (request-reply pattern).
#[instrument(name = "acp.client.session.request_permission", skip(headers, payload, client, nats))]
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
                "request_permission requires reply subject; ignoring message"
            );
            return;
        }
    };

    let response_id = response_id_from_request_headers(headers);
    match forward_to_client(headers, payload, client, session_id).await {
        Ok(response) => {
            rpc_reply::publish_success_reply(nats, reply_to, response_id, &response, "request_permission reply").await;
        }
        Err(e) => {
            let (code, message) = error_code_and_message(&e);
            warn!(
                error = %e,
                session_id = %session_id,
                "Failed to handle request_permission"
            );
            rpc_reply::publish_error_reply(
                nats,
                reply_to,
                response_id,
                code,
                &message,
                "request_permission error reply",
            )
            .await;
        }
    }
}

async fn forward_to_client<C: Client>(
    headers: &HeaderMap,
    payload: &[u8],
    client: &C,
    expected_session_id: &str,
) -> Result<RequestPermissionResponse, RequestPermissionError> {
    let request: RequestPermissionRequest = decode_request_params("session/request_permission", headers, payload)
        .map_err(|e| RequestPermissionError::InvalidRequest(serde_json::Error::custom(format!("{e}"))))?;
    let params_session_id = request.session_id.to_string();
    if params_session_id != expected_session_id {
        return Err(RequestPermissionError::InvalidRequest(serde_json::Error::custom(
            format!(
                "params.sessionId ({}) does not match subject session id ({})",
                params_session_id, expected_session_id
            ),
        )));
    }
    client
        .request_permission(request)
        .await
        .map_err(RequestPermissionError::ClientError)
}

#[cfg(test)]
mod tests;
