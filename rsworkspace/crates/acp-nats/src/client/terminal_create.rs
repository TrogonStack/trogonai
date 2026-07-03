use crate::client::rpc_reply;
use crate::nats::{FlushClient, PublishClient};
use crate::wire::{decode_request_params, response_id_from_request_headers};
use agent_client_protocol::{Client, CreateTerminalRequest, CreateTerminalResponse, ErrorCode};
use async_nats::header::HeaderMap;
use serde::de::Error as SerdeDeError;
use tracing::{instrument, warn};
use trogon_semconv::span::ACP_CLIENT_TERMINAL_CREATE;

#[derive(Debug, thiserror::Error)]
pub enum TerminalCreateError {
    #[error("invalid request: {0}")]
    InvalidRequest(#[source] serde_json::Error),
    #[error("client error: {0}")]
    ClientError(#[source] agent_client_protocol::Error),
}

pub fn error_code_and_message(e: &TerminalCreateError) -> (ErrorCode, String) {
    match e {
        TerminalCreateError::InvalidRequest(inner) => (
            ErrorCode::InvalidParams,
            format!("Invalid terminal/create request: {}", inner),
        ),
        TerminalCreateError::ClientError(inner) => (inner.code, inner.message.clone()),
    }
}

#[instrument(name = ACP_CLIENT_TERMINAL_CREATE, skip(headers, payload, client, nats))]
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
                "terminal/create requires reply subject; ignoring message"
            );
            return;
        }
    };

    let response_id = response_id_from_request_headers(headers);
    match forward_to_client(headers, payload, client, session_id).await {
        Ok(response) => {
            rpc_reply::publish_success_reply(nats, reply_to, response_id, &response, "terminal_create reply").await;
        }
        Err(e) => {
            let (code, message) = error_code_and_message(&e);
            warn!(
                error = %e,
                session_id = %session_id,
                "Failed to handle terminal/create"
            );
            rpc_reply::publish_error_reply(
                nats,
                reply_to,
                response_id,
                code,
                &message,
                "terminal_create error reply",
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
) -> Result<CreateTerminalResponse, TerminalCreateError> {
    let request: CreateTerminalRequest = decode_request_params("terminal/create", headers, payload).map_err(|e| {
        TerminalCreateError::InvalidRequest(serde_json::Error::custom(format!(
            "Invalid terminal/create request: {e}"
        )))
    })?;
    let params_session_id = request.session_id.to_string();
    if params_session_id != expected_session_id {
        return Err(TerminalCreateError::InvalidRequest(serde_json::Error::custom(format!(
            "params.sessionId ({}) does not match subject session id ({})",
            params_session_id, expected_session_id
        ))));
    }
    client
        .create_terminal(request)
        .await
        .map_err(TerminalCreateError::ClientError)
}

#[cfg(test)]
mod tests;
