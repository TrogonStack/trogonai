use a2a_pack::{AgentCardSource, accept_agent_card_on_read};
use tracing::{instrument, warn};

use crate::server::handler::{A2aError, A2aExecutor};
use crate::server::wire::{
    encode_error_reply, encode_success_reply, is_notification, parse_request_params, publish_reply, request_id,
};

const METHOD: &str = "agent/getAuthenticatedExtendedCard";

#[instrument(name = "a2a.server.agent_card", skip(handler, headers, payload, reply_subject, nats))]
pub async fn handle<H, N>(
    handler: &H,
    headers: &async_nats::header::HeaderMap,
    payload: &[u8],
    reply_subject: Option<String>,
    nats: &N,
) where
    H: A2aExecutor,
    N: trogon_nats::PublishClient,
{
    let Some(reply) = reply_subject else {
        warn!("agent/getAuthenticatedExtendedCard received without reply subject; dropping");
        return;
    };

    if request_id(headers).is_none() && is_notification(headers) {
        return;
    }

    let result = match parse_request_params::<serde_json::Value>(METHOD, headers, payload) {
        Err(_) => Err(parse_error()),
        Ok(raw) => {
            let params = if raw.is_null() {
                Ok(a2a::types::GetExtendedAgentCardRequest { tenant: None })
            } else {
                serde_json::from_value::<a2a::types::GetExtendedAgentCardRequest>(raw)
                    .map_err(|e| A2aError::new(-32602, format!("Invalid params: {e}")))
            };
            match params {
                Err(e) => Err(e),
                Ok(p) => match handler.agent_card(p).await {
                    Ok(card) => match serde_json::to_value(&card) {
                        Ok(v) if accept_agent_card_on_read(&v, AgentCardSource::AgentHandler) => Ok(card),
                        Ok(_) => Err(A2aError::invalid_agent_response("AgentCard failed read validation")),
                        Err(_) => Err(A2aError::internal("failed to serialize agent card for validation")),
                    },
                    Err(e) => Err(e),
                },
            }
        }
    };
    let encoded = match result {
        Ok(resp) => encode_success_reply(headers, &resp),
        Err(e) => encode_error_reply(headers, e.code, e.message, None),
    };
    publish_reply(nats, &reply, encoded, "agent_card reply").await;
}

fn parse_error() -> A2aError {
    A2aError::new(-32700, "Parse error")
}

#[cfg(test)]
mod tests;
