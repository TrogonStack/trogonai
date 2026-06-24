use crate::client::rpc_reply;
use crate::jsonrpc::extract_request_id;
use crate::nats::{FlushClient, PublishClient};
use agent_client_protocol::{Client, ErrorCode, KillTerminalRequest, KillTerminalResponse, Request, Response};
use bytes::Bytes;
use tracing::{instrument, warn};
use trogon_std::JsonSerialize;

#[derive(Debug, thiserror::Error)]
pub enum TerminalKillError {
    #[error("malformed JSON: {0}")]
    MalformedJson(#[source] serde_json::Error),
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
        TerminalKillError::MalformedJson(inner) => (
            ErrorCode::ParseError,
            format!("Malformed terminal/kill request JSON: {}", inner),
        ),
        TerminalKillError::InvalidParams(inner) => (inner.code, inner.message.clone()),
        TerminalKillError::ClientError(inner) => (inner.code, inner.message.clone()),
    }
}

#[instrument(name = "acp.client.terminal.kill", skip(payload, client, nats, serializer))]
pub async fn handle<N: PublishClient + FlushClient, C: Client, S: JsonSerialize>(
    payload: &[u8],
    client: &C,
    reply: Option<&str>,
    nats: &N,
    expected_session_id: &str,
    serializer: &S,
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

    let request_id = extract_request_id(payload);
    match forward_to_client(payload, client, expected_session_id).await {
        Ok(response) => {
            let (response_bytes, content_type) = serializer
                .to_vec(&Response::Result {
                    id: request_id.clone(),
                    result: response,
                })
                .map(|v| (Bytes::from(v), rpc_reply::CONTENT_TYPE_JSON))
                .unwrap_or_else(|e| {
                    warn!(error = %e, "JSON serialization of response failed, sending error reply");
                    rpc_reply::error_response_bytes(
                        serializer,
                        request_id,
                        ErrorCode::InternalError,
                        &format!("Failed to serialize response: {}", e),
                    )
                });
            rpc_reply::publish_reply(nats, reply_to, response_bytes, content_type, "terminal_kill reply").await;
        }
        Err(e) => {
            let (code, message) = error_code_and_message(&e);
            warn!(
                error = %e,
                session_id = %expected_session_id,
                "Failed to handle terminal/kill"
            );
            let (bytes, content_type) = rpc_reply::error_response_bytes(serializer, request_id, code, &message);
            rpc_reply::publish_reply(nats, reply_to, bytes, content_type, "terminal_kill error reply").await;
        }
    }
}

async fn forward_to_client<C: Client>(
    payload: &[u8],
    client: &C,
    expected_session_id: &str,
) -> Result<KillTerminalResponse, TerminalKillError> {
    let payload_value: serde_json::Value = serde_json::from_slice(payload).map_err(TerminalKillError::MalformedJson)?;
    let envelope: Request<KillTerminalRequest> = serde_json::from_value(payload_value)
        .map_err(|e| invalid_params_error(format!("Invalid terminal/kill request: {}", e)))?;
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

    client
        .kill_terminal(request)
        .await
        .map_err(TerminalKillError::ClientError)
}

#[cfg(test)]
mod tests;
