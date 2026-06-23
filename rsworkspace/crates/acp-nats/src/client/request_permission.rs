use crate::client::rpc_reply;
use crate::jsonrpc::extract_request_id;
use crate::nats::{FlushClient, PublishClient};
use agent_client_protocol::{
    Client, ErrorCode, Request, RequestPermissionRequest, RequestPermissionResponse, Response,
};
use bytes::Bytes;
use serde::de::Error as SerdeDeError;
use tracing::{instrument, warn};
use trogon_std::JsonSerialize;

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

/// Handles session/request_permission: parses JSON-RPC request, calls client, wraps response in
/// JSON-RPC envelope, and publishes to reply subject. Reply is required (request-reply pattern).
#[instrument(
    name = "acp.client.session.request_permission",
    skip(payload, client, nats, serializer)
)]
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
                "request_permission requires reply subject; ignoring message"
            );
            return;
        }
    };

    let request_id = extract_request_id(payload);
    match forward_to_client(payload, client, session_id).await {
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
            rpc_reply::publish_reply(nats, reply_to, response_bytes, content_type, "request_permission reply").await;
        }
        Err(e) => {
            let (code, message) = error_code_and_message(&e);
            warn!(
                error = %e,
                session_id = %session_id,
                "Failed to handle request_permission"
            );
            let (bytes, content_type) = rpc_reply::error_response_bytes(serializer, request_id, code, &message);
            rpc_reply::publish_reply(nats, reply_to, bytes, content_type, "request_permission error reply").await;
        }
    }
}

async fn forward_to_client<C: Client>(
    payload: &[u8],
    client: &C,
    expected_session_id: &str,
) -> Result<RequestPermissionResponse, RequestPermissionError> {
    let envelope: Request<RequestPermissionRequest> =
        serde_json::from_slice(payload).map_err(RequestPermissionError::InvalidRequest)?;
    let request = envelope.params.ok_or_else(|| {
        RequestPermissionError::InvalidRequest(serde_json::Error::custom("params is null or missing"))
    })?;
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
