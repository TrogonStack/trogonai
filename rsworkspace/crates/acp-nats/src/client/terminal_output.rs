use crate::client::rpc_reply;
use crate::nats::{FlushClient, PublishClient};
use crate::wire::{decode_request_params, response_id_from_request_headers};
use agent_client_protocol::{Client, ErrorCode, TerminalOutputRequest};
use async_nats::header::HeaderMap;
use tracing::{instrument, warn};
use trogon_semconv::span::ACP_CLIENT_TERMINAL_OUTPUT;

#[instrument(name = ACP_CLIENT_TERMINAL_OUTPUT, skip(headers, payload, client, nats))]
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
                "terminal/output requires reply subject; ignoring message"
            );
            return;
        }
    };

    let response_id = response_id_from_request_headers(headers);

    let request = match parse_request(headers, payload, session_id) {
        Ok(req) => req,
        Err((code, message)) => {
            warn!(
                error = %message,
                session_id = %session_id,
                "Failed to handle terminal/output"
            );
            rpc_reply::publish_error_reply(
                nats,
                reply_to,
                response_id,
                code,
                &message,
                "terminal/output error reply",
            )
            .await;
            return;
        }
    };

    match client.terminal_output(request).await {
        Ok(response) => {
            rpc_reply::publish_success_reply(nats, reply_to, response_id, &response, "terminal/output reply").await;
        }
        Err(e) => {
            warn!(
                error = %e,
                session_id = %session_id,
                "Failed to handle terminal/output"
            );
            rpc_reply::publish_error_reply(
                nats,
                reply_to,
                response_id,
                e.code,
                &e.message,
                "terminal/output error reply",
            )
            .await;
        }
    }
}

fn parse_request(
    headers: &HeaderMap,
    payload: &[u8],
    expected_session_id: &str,
) -> Result<TerminalOutputRequest, (ErrorCode, String)> {
    let request: TerminalOutputRequest = decode_request_params("terminal/output", headers, payload)
        .map_err(|e| (ErrorCode::InvalidParams, format!("Invalid request: {e}")))?;

    let params_session_id = request.session_id.to_string();
    if params_session_id != expected_session_id {
        return Err((
            ErrorCode::InvalidParams,
            format!(
                "params.sessionId ({}) does not match subject session id ({})",
                params_session_id, expected_session_id
            ),
        ));
    }

    Ok(request)
}

#[cfg(test)]
mod tests;
