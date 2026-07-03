use crate::client::rpc_reply;
use crate::nats::{FlushClient, PublishClient};
use crate::wire::{decode_request_params, response_id_from_request_headers};
use agent_client_protocol::{ElicitationAction, ElicitationRequest, ElicitationResponse, ErrorCode};
use async_nats::header::HeaderMap;
use serde::de::Error as SerdeDeError;
use tracing::{instrument, warn};

/// Implemented by anything that can field an elicitation request from the agent.
/// The default returns `Cancel`, which is correct for bridges that do not yet
/// support forwarding elicitations to the real client.
#[async_trait::async_trait(?Send)]
pub trait ElicitationClient {
    async fn request_elicitation(
        &self,
        _request: ElicitationRequest,
    ) -> agent_client_protocol::Result<ElicitationResponse> {
        Ok(ElicitationResponse::new(ElicitationAction::Cancel))
    }
}

impl<T> ElicitationClient for T {}

#[derive(Debug, thiserror::Error)]
pub enum SessionElicitationError {
    #[error("invalid request: {0}")]
    InvalidRequest(#[source] serde_json::Error),
    #[error("client error: {0}")]
    ClientError(#[source] agent_client_protocol::Error),
}

pub fn error_code_and_message(e: &SessionElicitationError) -> (ErrorCode, String) {
    match e {
        SessionElicitationError::InvalidRequest(inner) => (
            ErrorCode::InvalidParams,
            format!("Invalid elicitation request: {}", inner),
        ),
        SessionElicitationError::ClientError(inner) => (inner.code, inner.message.clone()),
    }
}

/// Handles `session.elicitation`: decodes wire request params, calls client, and publishes
/// a wire-encoded reply. Reply is required (request-reply pattern).
#[instrument(name = "acp.client.session.elicitation", skip(headers, payload, client, nats))]
pub async fn handle<N: PublishClient + FlushClient, C: ElicitationClient>(
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
                "session.elicitation requires reply subject; ignoring message"
            );
            return;
        }
    };

    let response_id = response_id_from_request_headers(headers);
    match forward_to_client(headers, payload, client, session_id).await {
        Ok(response) => {
            rpc_reply::publish_success_reply(nats, reply_to, response_id, &response, "elicitation reply").await;
        }
        Err(e) => {
            let (code, message) = error_code_and_message(&e);
            warn!(
                error = %e,
                session_id = %session_id,
                "Failed to handle session.elicitation"
            );
            rpc_reply::publish_error_reply(nats, reply_to, response_id, code, &message, "elicitation error reply")
                .await;
        }
    }
}

async fn forward_to_client<C: ElicitationClient>(
    headers: &HeaderMap,
    payload: &[u8],
    client: &C,
    expected_session_id: &str,
) -> Result<ElicitationResponse, SessionElicitationError> {
    let request: ElicitationRequest = decode_request_params("session/elicitation", headers, payload)
        .map_err(|e| SessionElicitationError::InvalidRequest(serde_json::Error::custom(format!("{e}"))))?;
    let params_session_id = request.session_id.to_string();
    if params_session_id != expected_session_id {
        return Err(SessionElicitationError::InvalidRequest(serde_json::Error::custom(
            format!(
                "params.sessionId ({}) does not match subject session id ({})",
                params_session_id, expected_session_id
            ),
        )));
    }
    client
        .request_elicitation(request)
        .await
        .map_err(SessionElicitationError::ClientError)
}

#[cfg(test)]
mod tests;
