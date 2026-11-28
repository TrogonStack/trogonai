use crate::client::rpc_reply;
use crate::client_handler::ClientHandler;
use crate::ext_method_name::ExtMethodName;
use crate::nats::{FlushClient, PublishClient};
use crate::wire::{decode_notification_params, decode_request_params, response_id_from_request_headers};
use agent_client_protocol::ErrorCode;
use agent_client_protocol::v1::schema::{ExtNotification, ExtRequest, ExtResponse};
use async_nats::header::HeaderMap;
use serde_json::value::RawValue;
use std::sync::Arc;
use tracing::{instrument, warn};
use trogon_semconv::span::ACP_CLIENT_EXT;

#[derive(Debug, thiserror::Error)]
pub enum ExtError {
    #[error("malformed JSON: {0}")]
    MalformedJson(String),
    #[error("client error: {0}")]
    ClientError(#[source] agent_client_protocol::Error),
}

impl From<&ExtError> for agent_client_protocol::Error {
    fn from(error: &ExtError) -> Self {
        match error {
            ExtError::MalformedJson(inner) => {
                Self::new(ErrorCode::PARSE_ERROR, format!("Malformed ext request JSON: {}", inner))
            }
            ExtError::ClientError(inner) => inner.clone(),
        }
    }
}

#[instrument(
    name = ACP_CLIENT_EXT,
    skip(headers, payload, client, nats),
    fields(ext_method = %ext_method_name)
)]
pub async fn handle<N: PublishClient + FlushClient, C: ClientHandler + Sync>(
    headers: &HeaderMap,
    payload: &[u8],
    client: &C,
    reply: Option<&str>,
    nats: &N,
    ext_method_name: &ExtMethodName,
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

async fn handle_request<N: PublishClient + FlushClient, C: ClientHandler + Sync>(
    headers: &HeaderMap,
    payload: &[u8],
    client: &C,
    reply_to: &str,
    nats: &N,
    ext_method_name: &ExtMethodName,
) {
    let wire_method = ext_method_name.protocol_method();
    let response_id = response_id_from_request_headers(headers);
    match forward_request(headers, payload, client, ext_method_name, &wire_method).await {
        Ok(response) => {
            rpc_reply::publish_success_reply(nats, reply_to, response_id, &response, "ext_method reply").await;
        }
        Err(e) => {
            let error = agent_client_protocol::Error::from(&e);
            warn!(error = %e, "Failed to handle ext method");
            rpc_reply::publish_agent_error_reply(nats, reply_to, response_id, &error, "ext_method error reply").await;
        }
    }
}

async fn handle_notification<C: ClientHandler + Sync>(
    headers: &HeaderMap,
    payload: &[u8],
    client: &C,
    ext_method_name: &ExtMethodName,
) {
    let wire_method = ext_method_name.protocol_method();
    let params: Arc<RawValue> = match decode_notification_params(&wire_method, headers, payload) {
        Ok(v) => v,
        Err(e) => {
            warn!(error = %e, "Failed to parse ext notification payload");
            return;
        }
    };

    let notification = ExtNotification::new(ext_method_name.as_str(), params);
    if let Err(e) = client.ext_notification(notification).await {
        warn!(error = %e, "Failed to send ext notification to client");
    }
}

async fn forward_request<C: ClientHandler + Sync>(
    headers: &HeaderMap,
    payload: &[u8],
    client: &C,
    ext_method_name: &ExtMethodName,
    wire_method: &str,
) -> Result<ExtResponse, ExtError> {
    let params: Arc<RawValue> =
        decode_request_params(wire_method, headers, payload).map_err(|e| ExtError::MalformedJson(e.to_string()))?;
    let request = ExtRequest::new(ext_method_name.as_str(), params);
    client.ext_method(request).await.map_err(ExtError::ClientError)
}

#[cfg(test)]
mod tests;
