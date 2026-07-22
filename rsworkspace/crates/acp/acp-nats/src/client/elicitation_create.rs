use crate::client::rpc_reply;
use crate::client_handler::ClientHandler;
use crate::nats::{FlushClient, PublishClient};
use crate::wire::{decode_request_params, response_id_from_request_headers};
use agent_client_protocol::ErrorCode;
use agent_client_protocol::schema::v1::{CreateElicitationRequest, CreateElicitationResponse, ElicitationScope};
use async_nats::header::HeaderMap;
use tracing::{instrument, warn};
use trogon_semconv::span::ACP_CLIENT_ELICITATION_CREATE;

#[derive(Debug, thiserror::Error)]
pub enum ElicitationCreateError {
    #[error("invalid request: {0}")]
    InvalidRequest(#[source] crate::wire::WireError),
    #[error("scope.sessionId ({params}) does not match subject session id ({subject})")]
    SessionMismatch { params: String, subject: String },
    #[error("client error: {0}")]
    ClientError(#[source] agent_client_protocol::Error),
}

pub fn error_code_and_message(e: &ElicitationCreateError) -> (ErrorCode, String) {
    match e {
        ElicitationCreateError::InvalidRequest(inner) => (
            ErrorCode::InvalidParams,
            format!("Invalid elicitation/create request: {}", inner),
        ),
        ElicitationCreateError::SessionMismatch { .. } => (ErrorCode::InvalidParams, e.to_string()),
        ElicitationCreateError::ClientError(inner) => (inner.code, inner.message.clone()),
    }
}

/// Handles elicitation/create: decodes wire request params, calls client, and publishes
/// a wire-encoded reply. Reply is required (request-reply pattern).
#[instrument(name = ACP_CLIENT_ELICITATION_CREATE, skip(headers, payload, client, nats))]
pub async fn handle<N: PublishClient + FlushClient, C: ClientHandler + Sync>(
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
                "elicitation/create requires reply subject; ignoring message"
            );
            return;
        }
    };

    let response_id = response_id_from_request_headers(headers);
    match forward_to_client(headers, payload, client, session_id).await {
        Ok(response) => {
            rpc_reply::publish_success_reply(nats, reply_to, response_id, &response, "elicitation/create reply").await;
        }
        Err(e) => {
            let (code, message) = error_code_and_message(&e);
            warn!(
                error = %e,
                session_id = %session_id,
                "Failed to handle elicitation/create"
            );
            rpc_reply::publish_error_reply(
                nats,
                reply_to,
                response_id,
                code,
                &message,
                "elicitation/create error reply",
            )
            .await;
        }
    }
}

async fn forward_to_client<C: ClientHandler + Sync>(
    headers: &HeaderMap,
    payload: &[u8],
    client: &C,
    expected_session_id: &str,
) -> Result<CreateElicitationResponse, ElicitationCreateError> {
    let request: CreateElicitationRequest = decode_request_params("elicitation/create", headers, payload)
        .map_err(ElicitationCreateError::InvalidRequest)?;

    if let ElicitationScope::Session(scope) = request.scope() {
        let params_session_id = scope.session_id.to_string();
        if params_session_id != expected_session_id {
            return Err(ElicitationCreateError::SessionMismatch {
                params: params_session_id,
                subject: expected_session_id.to_string(),
            });
        }
    }

    client
        .elicitation_create(request)
        .await
        .map_err(ElicitationCreateError::ClientError)
}

#[cfg(test)]
mod tests;
