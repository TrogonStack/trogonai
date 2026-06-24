use crate::client::rpc_reply;
use crate::nats::{FlushClient, PublishClient};
use crate::wire::{decode_request_params, response_id_from_request_headers};
use agent_client_protocol::{Client, ErrorCode, ReleaseTerminalRequest, ReleaseTerminalResponse};
use async_nats::header::HeaderMap;
use tracing::{instrument, warn};

#[derive(Debug, thiserror::Error)]
pub enum TerminalReleaseError {
    #[error("invalid request: {0}")]
    InvalidRequest(String),
    #[error("client error: {0}")]
    ClientError(#[source] agent_client_protocol::Error),
}

pub fn error_code_and_message(e: &TerminalReleaseError) -> (ErrorCode, String) {
    match e {
        TerminalReleaseError::InvalidRequest(message) => (ErrorCode::InvalidParams, message.clone()),
        TerminalReleaseError::ClientError(inner) => (inner.code, inner.message.clone()),
    }
}

#[instrument(name = "acp.client.terminal.release", skip(headers, payload, client, nats))]
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
                "terminal/release requires reply subject; ignoring message"
            );
            return;
        }
    };

    let response_id = response_id_from_request_headers(headers);
    match forward_to_client(headers, payload, client, session_id).await {
        Ok(response) => {
            rpc_reply::publish_success_reply(nats, reply_to, response_id, &response, "terminal_release reply").await;
        }
        Err(e) => {
            let (code, message) = error_code_and_message(&e);
            warn!(
                error = %e,
                session_id = %session_id,
                "Failed to handle terminal/release"
            );
            rpc_reply::publish_error_reply(nats, reply_to, response_id, code, &message, "terminal_release error reply")
                .await;
        }
    }
}

async fn forward_to_client<C: Client>(
    headers: &HeaderMap,
    payload: &[u8],
    client: &C,
    expected_session_id: &str,
) -> Result<ReleaseTerminalResponse, TerminalReleaseError> {
    let request: ReleaseTerminalRequest = decode_request_params("terminal/release", headers, payload).map_err(|e| {
        TerminalReleaseError::InvalidRequest(format!("Invalid terminal/release request: {e}"))
    })?;
    let params_session_id = request.session_id.to_string();
    if params_session_id != expected_session_id {
        return Err(TerminalReleaseError::InvalidRequest(format!(
            "params.sessionId ({}) does not match subject session id ({})",
            params_session_id, expected_session_id
        )));
    }
    client
        .release_terminal(request)
        .await
        .map_err(TerminalReleaseError::ClientError)
}

#[cfg(test)]
mod tests;
