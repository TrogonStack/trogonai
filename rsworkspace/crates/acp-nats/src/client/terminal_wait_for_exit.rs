use crate::client::rpc_reply;
use crate::nats::{FlushClient, PublishClient};
use crate::wire::{decode_request_params, response_id_from_request_headers};
use agent_client_protocol::{Client, ErrorCode, WaitForTerminalExitRequest, WaitForTerminalExitResponse};
use async_nats::header::HeaderMap;
use std::time::Duration;
use tokio::time::timeout;
use tracing::{instrument, warn};

#[derive(Debug, thiserror::Error)]
pub enum TerminalWaitForExitError {
    #[error("invalid params: {0}")]
    InvalidParams(#[source] agent_client_protocol::Error),
    #[error("Timed out waiting for terminal exit")]
    TimedOut,
    #[error("client error: {0}")]
    ClientError(#[source] agent_client_protocol::Error),
}

fn invalid_params_error(message: impl Into<String>) -> TerminalWaitForExitError {
    TerminalWaitForExitError::InvalidParams(agent_client_protocol::Error::new(
        i32::from(ErrorCode::InvalidParams),
        message.into(),
    ))
}

pub fn error_code_and_message(e: &TerminalWaitForExitError) -> (ErrorCode, String) {
    match e {
        TerminalWaitForExitError::InvalidParams(inner) => (inner.code, inner.message.clone()),
        TerminalWaitForExitError::TimedOut => (
            ErrorCode::InternalError,
            "Timed out waiting for terminal exit".to_string(),
        ),
        TerminalWaitForExitError::ClientError(inner) => (inner.code, inner.message.clone()),
    }
}

#[instrument(
    name = "acp.client.terminal.wait_for_exit",
    skip(headers, payload, client, nats),
    fields(session_id = %expected_session_id)
)]
pub async fn handle<N: PublishClient + FlushClient, C: Client>(
    headers: &HeaderMap,
    payload: &[u8],
    client: &C,
    reply: Option<&str>,
    nats: &N,
    expected_session_id: &str,
    operation_timeout: Duration,
) {
    let reply_to = match reply {
        Some(r) => r,
        None => {
            warn!(
                session_id = %expected_session_id,
                "terminal/wait_for_exit requires reply subject; ignoring message"
            );
            return;
        }
    };

    let response_id = response_id_from_request_headers(headers);
    match forward_to_client(headers, payload, client, expected_session_id, operation_timeout).await {
        Ok(response) => {
            rpc_reply::publish_success_reply(nats, reply_to, response_id, &response, "terminal/wait_for_exit reply")
                .await;
        }
        Err(e) => {
            let (code, message) = error_code_and_message(&e);
            warn!(
                error = %e,
                session_id = %expected_session_id,
                "Failed to handle terminal/wait_for_exit"
            );
            rpc_reply::publish_error_reply(
                nats,
                reply_to,
                response_id,
                code,
                &message,
                "terminal/wait_for_exit error reply",
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
    operation_timeout: Duration,
) -> Result<WaitForTerminalExitResponse, TerminalWaitForExitError> {
    let request = parse_request(headers, payload, expected_session_id)?;
    timeout(operation_timeout, client.wait_for_terminal_exit(request))
        .await
        .map_err(|_| TerminalWaitForExitError::TimedOut)?
        .map_err(TerminalWaitForExitError::ClientError)
}

fn parse_request(
    headers: &HeaderMap,
    payload: &[u8],
    expected_session_id: &str,
) -> Result<WaitForTerminalExitRequest, TerminalWaitForExitError> {
    let request: WaitForTerminalExitRequest = decode_request_params("terminal/wait_for_exit", headers, payload)
        .map_err(|e| invalid_params_error(format!("Invalid terminal/wait_for_exit request: {e}")))?;

    let params_session_id = request.session_id.to_string();
    if params_session_id != expected_session_id {
        return Err(invalid_params_error(format!(
            "params.sessionId ({}) does not match subject session id ({})",
            params_session_id, expected_session_id
        )));
    }

    Ok(request)
}

#[cfg(test)]
mod tests;
