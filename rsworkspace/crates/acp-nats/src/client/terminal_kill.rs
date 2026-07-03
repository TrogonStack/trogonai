use crate::client::rpc_reply;
use crate::nats::{FlushClient, PublishClient};
use crate::wire::{decode_request_params, response_id_from_request_headers};
use agent_client_protocol::{Client, ErrorCode, KillTerminalRequest, KillTerminalResponse};
use async_nats::header::HeaderMap;
use tracing::{instrument, warn};
use trogon_semconv::span::ACP_CLIENT_TERMINAL_KILL;

#[derive(Debug, thiserror::Error)]
pub enum TerminalKillError {
    #[error("invalid params: {0}")]
    InvalidParams(#[source] agent_client_protocol::Error),
    #[error("client error: {0}")]
    ClientError(#[source] agent_client_protocol::Error),
}

fn invalid_params_error(message: impl Into<String>) -> TerminalKillError {
    TerminalKillError::InvalidParams(agent_client_protocol::Error::new(
        i32::from(ErrorCode::InvalidParams),
        message.into(),
    ))
}

pub fn error_code_and_message(e: &TerminalKillError) -> (ErrorCode, String) {
    match e {
        TerminalKillError::InvalidParams(inner) => (inner.code, inner.message.clone()),
        TerminalKillError::ClientError(inner) => (inner.code, inner.message.clone()),
    }
}

#[instrument(name = ACP_CLIENT_TERMINAL_KILL, skip(headers, payload, client, nats))]
pub async fn handle<N: PublishClient + FlushClient, C: Client>(
    headers: &HeaderMap,
    payload: &[u8],
    client: &C,
    reply: Option<&str>,
    nats: &N,
    expected_session_id: &str,
) {
    let reply_to = match reply {
        Some(r) => r,
        None => {
            warn!(
                session_id = %expected_session_id,
                "terminal/kill requires reply subject; ignoring message"
            );
            return;
        }
    };

    let response_id = response_id_from_request_headers(headers);
    match forward_to_client(headers, payload, client, expected_session_id).await {
        Ok(response) => {
            rpc_reply::publish_success_reply(nats, reply_to, response_id, &response, "terminal_kill reply").await;
        }
        Err(e) => {
            let (code, message) = error_code_and_message(&e);
            warn!(
                error = %e,
                session_id = %expected_session_id,
                "Failed to handle terminal/kill"
            );
            rpc_reply::publish_error_reply(nats, reply_to, response_id, code, &message, "terminal_kill error reply")
                .await;
        }
    }
}

async fn forward_to_client<C: Client>(
    headers: &HeaderMap,
    payload: &[u8],
    client: &C,
    expected_session_id: &str,
) -> Result<KillTerminalResponse, TerminalKillError> {
    let request: KillTerminalRequest = decode_request_params("terminal/kill", headers, payload)
        .map_err(|e| invalid_params_error(format!("Invalid terminal/kill request: {e}")))?;

    let params_session_id = request.session_id.to_string();
    if params_session_id != expected_session_id {
        return Err(invalid_params_error(format!(
            "params.sessionId ({}) does not match subject session id ({})",
            params_session_id, expected_session_id
        )));
    }

    client
        .kill_terminal(request)
        .await
        .map_err(TerminalKillError::ClientError)
}

#[cfg(test)]
mod tests;
