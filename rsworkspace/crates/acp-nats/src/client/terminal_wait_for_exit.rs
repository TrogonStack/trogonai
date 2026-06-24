use crate::client::rpc_reply;
use crate::jsonrpc::extract_request_id;
use crate::nats::{FlushClient, PublishClient};
use agent_client_protocol::{
    Client, ErrorCode, Request, Response, WaitForTerminalExitRequest, WaitForTerminalExitResponse,
};
use bytes::Bytes;
use std::time::Duration;
use tokio::time::timeout;
use tracing::{instrument, warn};
use trogon_std::JsonSerialize;

#[derive(Debug, thiserror::Error)]
pub enum TerminalWaitForExitError {
    #[error("malformed JSON: {0}")]
    MalformedJson(#[source] serde_json::Error),
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
        TerminalWaitForExitError::MalformedJson(inner) => (
            ErrorCode::ParseError,
            format!("Malformed terminal/wait_for_exit request JSON: {}", inner),
        ),
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
    skip(payload, client, nats, serializer),
    fields(session_id = %expected_session_id)
)]
pub async fn handle<N: PublishClient + FlushClient, C: Client, S: JsonSerialize>(
    payload: &[u8],
    client: &C,
    reply: Option<&str>,
    nats: &N,
    expected_session_id: &str,
    operation_timeout: Duration,
    serializer: &S,
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

    let request_id = extract_request_id(payload);
    match forward_to_client(payload, client, expected_session_id, operation_timeout).await {
        Ok(response) => {
            let (response_bytes, content_type) = serializer
                .to_vec(&Response::Result {
                    id: request_id.clone(),
                    result: response,
                })
                .map(|v| (Bytes::from(v), rpc_reply::CONTENT_TYPE_JSON))
                .unwrap_or_else(|e| {
                    warn!(
                        error = %e,
                        "JSON serialization of response failed, sending error reply"
                    );
                    rpc_reply::error_response_bytes(
                        serializer,
                        request_id,
                        ErrorCode::InternalError,
                        &format!("Failed to serialize response: {}", e),
                    )
                });
            rpc_reply::publish_reply(
                nats,
                reply_to,
                response_bytes,
                content_type,
                "terminal/wait_for_exit reply",
            )
            .await;
        }
        Err(e) => {
            let (code, message) = error_code_and_message(&e);
            warn!(
                error = %e,
                session_id = %expected_session_id,
                "Failed to handle terminal/wait_for_exit"
            );
            let (bytes, content_type) = rpc_reply::error_response_bytes(serializer, request_id, code, &message);
            rpc_reply::publish_reply(
                nats,
                reply_to,
                bytes,
                content_type,
                "terminal/wait_for_exit error reply",
            )
            .await;
        }
    }
}

async fn forward_to_client<C: Client>(
    payload: &[u8],
    client: &C,
    expected_session_id: &str,
    operation_timeout: Duration,
) -> Result<WaitForTerminalExitResponse, TerminalWaitForExitError> {
    let request = parse_request(payload, expected_session_id)?;
    timeout(operation_timeout, client.wait_for_terminal_exit(request))
        .await
        .map_err(|_| TerminalWaitForExitError::TimedOut)?
        .map_err(TerminalWaitForExitError::ClientError)
}

fn parse_request(
    payload: &[u8],
    expected_session_id: &str,
) -> Result<WaitForTerminalExitRequest, TerminalWaitForExitError> {
    let payload_value: serde_json::Value =
        serde_json::from_slice(payload).map_err(TerminalWaitForExitError::MalformedJson)?;
    let envelope: Request<WaitForTerminalExitRequest> = serde_json::from_value(payload_value)
        .map_err(|e| invalid_params_error(format!("Invalid terminal/wait_for_exit request: {}", e)))?;
    let request = envelope
        .params
        .ok_or_else(|| invalid_params_error("params is null or missing"))?;

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
