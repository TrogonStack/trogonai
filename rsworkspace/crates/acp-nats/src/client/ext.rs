use crate::client::rpc_reply;
use crate::nats::{FlushClient, PublishClient};
use crate::wire::{decode_notification_params, decode_request_params, response_id_from_request_headers};
use agent_client_protocol::{Client, ErrorCode, ExtNotification, ExtRequest, ExtResponse};
use async_nats::header::HeaderMap;
use serde_json::value::RawValue;
use std::sync::Arc;
use tracing::{instrument, warn};

#[derive(Debug, thiserror::Error)]
pub enum ExtError {
    #[error("malformed JSON: {0}")]
    MalformedJson(String),
    #[error("client error: {0}")]
    ClientError(#[source] agent_client_protocol::Error),
}

pub fn error_code_and_message(e: &ExtError) -> (ErrorCode, String) {
    match e {
        ExtError::MalformedJson(inner) => (ErrorCode::ParseError, format!("Malformed ext request JSON: {}", inner)),
        ExtError::ClientError(inner) => (inner.code, inner.message.clone()),
    }
}

#[instrument(
    name = "acp.client.ext",
    skip(headers, payload, client, nats),
    fields(ext_method = %ext_method_name)
)]
pub async fn handle<N: PublishClient + FlushClient, C: Client>(
    headers: &HeaderMap,
    payload: &[u8],
    client: &C,
    reply: Option<&str>,
    nats: &N,
    ext_method_name: &str,
) {
    match reply {
        Some(reply_to) => {
            handle_request(headers, payload, client, reply_to, nats, ext_method_name).await;
        }
        None => {
            handle_notification(headers, payload, client, ext_method_name).await;
        }
    }
}

async fn handle_request<N: PublishClient + FlushClient, C: Client>(
    headers: &HeaderMap,
    payload: &[u8],
    client: &C,
    reply_to: &str,
    nats: &N,
    ext_method_name: &str,
) {
    let wire_method = format!("ext/{ext_method_name}");
    let response_id = response_id_from_request_headers(headers);
    match forward_request(headers, payload, client, ext_method_name, &wire_method).await {
        Ok(response) => {
            rpc_reply::publish_success_reply(nats, reply_to, response_id, &response, "ext_method reply").await;
        }
        Err(e) => {
            let (code, message) = error_code_and_message(&e);
            warn!(error = %e, "Failed to handle ext method");
            rpc_reply::publish_error_reply(nats, reply_to, response_id, code, &message, "ext_method error reply").await;
        }
    }
}

async fn handle_notification<C: Client>(headers: &HeaderMap, payload: &[u8], client: &C, ext_method_name: &str) {
    let wire_method = format!("ext/{ext_method_name}");
    let params: Arc<RawValue> = match decode_notification_params(&wire_method, headers, payload) {
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
    headers: &HeaderMap,
    payload: &[u8],
    client: &C,
    ext_method_name: &str,
    wire_method: &str,
) -> Result<ExtResponse, ExtError> {
    let params: Arc<RawValue> =
        decode_request_params(wire_method, headers, payload).map_err(|e| ExtError::MalformedJson(e.to_string()))?;
    let request = ExtRequest::new(ext_method_name, params);
    client.ext_method(request).await.map_err(ExtError::ClientError)
}

#[cfg(test)]
mod tests;
