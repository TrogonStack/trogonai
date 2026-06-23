use crate::client::rpc_reply;
use crate::jsonrpc::extract_request_id;
use crate::nats::{FlushClient, PublishClient};
use agent_client_protocol::{Client, ErrorCode, Request, Response, TerminalOutputRequest};
use bytes::Bytes;
use tracing::{instrument, warn};
use trogon_std::JsonSerialize;

#[instrument(name = "acp.client.terminal.output", skip(payload, client, nats, serializer))]
pub async fn handle<N: PublishClient + FlushClient, C: Client, S: JsonSerialize>(
    payload: &[u8],
    client: &C,
    reply: Option<&str>,
    nats: &N,
    session_id: &str,
    serializer: &S,
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

    let request_id = extract_request_id(payload);

    let request = match parse_request(payload, session_id) {
        Ok(req) => req,
        Err((code, message)) => {
            warn!(
                error = %message,
                session_id = %session_id,
                "Failed to handle terminal/output"
            );
            let (bytes, content_type) = rpc_reply::error_response_bytes(serializer, request_id, code, &message);
            rpc_reply::publish_reply(nats, reply_to, bytes, content_type, "terminal/output error reply").await;
            return;
        }
    };

    match client.terminal_output(request).await {
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
            rpc_reply::publish_reply(nats, reply_to, response_bytes, content_type, "terminal/output reply").await;
        }
        Err(e) => {
            warn!(
                error = %e,
                session_id = %session_id,
                "Failed to handle terminal/output"
            );
            let (bytes, content_type) = rpc_reply::error_response_bytes(serializer, request_id, e.code, &e.message);
            rpc_reply::publish_reply(nats, reply_to, bytes, content_type, "terminal/output error reply").await;
        }
    }
}

fn parse_request(payload: &[u8], expected_session_id: &str) -> Result<TerminalOutputRequest, (ErrorCode, String)> {
    let value: serde_json::Value =
        serde_json::from_slice(payload).map_err(|e| (ErrorCode::ParseError, format!("Malformed JSON: {}", e)))?;

    let envelope: Request<TerminalOutputRequest> =
        serde_json::from_value(value).map_err(|e| (ErrorCode::InvalidParams, format!("Invalid request: {}", e)))?;

    let request = envelope
        .params
        .ok_or_else(|| (ErrorCode::InvalidParams, "params is null or missing".to_string()))?;

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
