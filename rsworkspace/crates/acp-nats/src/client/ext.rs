use crate::client::rpc_reply;
use crate::jsonrpc::extract_request_id;
use crate::nats::{FlushClient, PublishClient};
use agent_client_protocol::{Client, ErrorCode, ExtNotification, ExtRequest, ExtResponse, Request, Response};
use bytes::Bytes;
use serde_json::value::RawValue;
use std::sync::Arc;
use tracing::{instrument, warn};
use trogon_std::JsonSerialize;

#[derive(Debug, thiserror::Error)]
pub enum ExtError {
    #[error("malformed JSON: {0}")]
    MalformedJson(#[source] serde_json::Error),
    #[error("params is null or missing")]
    MissingParams,
    #[error("client error: {0}")]
    ClientError(#[source] agent_client_protocol::Error),
}

pub fn error_code_and_message(e: &ExtError) -> (ErrorCode, String) {
    match e {
        ExtError::MalformedJson(inner) => (ErrorCode::ParseError, format!("Malformed ext request JSON: {}", inner)),
        ExtError::MissingParams => (ErrorCode::InvalidParams, "params is null or missing".to_string()),
        ExtError::ClientError(inner) => (inner.code, inner.message.clone()),
    }
}

#[instrument(
    name = "acp.client.ext",
    skip(payload, client, nats, serializer),
    fields(ext_method = %ext_method_name)
)]
pub async fn handle<N: PublishClient + FlushClient, C: Client, S: JsonSerialize>(
    payload: &[u8],
    client: &C,
    reply: Option<&str>,
    nats: &N,
    ext_method_name: &str,
    serializer: &S,
) {
    match reply {
        Some(reply_to) => {
            handle_request(payload, client, reply_to, nats, ext_method_name, serializer).await;
        }
        None => {
            handle_notification(payload, client, ext_method_name).await;
        }
    }
}

async fn handle_request<N: PublishClient + FlushClient, C: Client, S: JsonSerialize>(
    payload: &[u8],
    client: &C,
    reply_to: &str,
    nats: &N,
    ext_method_name: &str,
    serializer: &S,
) {
    let request_id = extract_request_id(payload);
    match forward_request(payload, client, ext_method_name).await {
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
            rpc_reply::publish_reply(nats, reply_to, response_bytes, content_type, "ext_method reply").await;
        }
        Err(e) => {
            let (code, message) = error_code_and_message(&e);
            warn!(error = %e, "Failed to handle ext method");
            let (bytes, content_type) = rpc_reply::error_response_bytes(serializer, request_id, code, &message);
            rpc_reply::publish_reply(nats, reply_to, bytes, content_type, "ext_method error reply").await;
        }
    }
}

async fn handle_notification<C: Client>(payload: &[u8], client: &C, ext_method_name: &str) {
    let params: Arc<RawValue> = match serde_json::from_slice(payload) {
        Ok(v) => v,
        Err(e) => {
            warn!(error = %e, "Failed to parse ext notification payload");
            return;
        }
    };

    let notification = ExtNotification::new(ext_method_name, params);
    if let Err(e) = client.ext_notification(notification).await {
        warn!(error = %e, "Failed to send ext notification to client");
    }
}

async fn forward_request<C: Client>(
    payload: &[u8],
    client: &C,
    ext_method_name: &str,
) -> Result<ExtResponse, ExtError> {
    let envelope: Request<Arc<RawValue>> = serde_json::from_slice(payload).map_err(ExtError::MalformedJson)?;
    let params = envelope.params.ok_or(ExtError::MissingParams)?;
    let request = ExtRequest::new(ext_method_name, params);
    client.ext_method(request).await.map_err(ExtError::ClientError)
}

#[cfg(test)]
mod tests;
