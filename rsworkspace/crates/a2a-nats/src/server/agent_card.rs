use a2a_pack::{AgentCardSource, accept_agent_card_on_read};
use tracing::{instrument, warn};

use crate::jsonrpc::extract_request_id;
use crate::server::handler::{A2aError, A2aExecutor};
use crate::server::wire::{JsonRpcErrorResponse, JsonRpcResponse, is_notification, parse_request};

#[instrument(name = "a2a.server.agent_card", skip(handler, payload, reply_subject, nats))]
pub async fn handle<H, N>(handler: &H, payload: &[u8], reply_subject: Option<String>, nats: &N)
where
    H: A2aExecutor,
    N: trogon_nats::PublishClient,
{
    let Some(reply) = reply_subject else {
        warn!("agent/getAuthenticatedExtendedCard received without reply subject; dropping");
        return;
    };

    // Recover the request id even if the full envelope fails to parse, so error
    // replies stay correlated with the caller's request rather than going to a
    // bare-null id.
    let id = extract_request_id(payload);
    // Only drop on a true JSON-RPC notification — payload is a parseable JSON
    // object that omits the `id` key. Malformed payloads or unparseable id
    // values still get a JSON-RPC error reply so clients don't hang waiting.
    if id.is_none() && is_notification(payload) {
        return;
    }

    let result = match parse_request::<serde_json::Value>(payload) {
        Err(_) => Err(parse_error()),
        Ok(envelope) => {
            let params = match envelope.params {
                None => Ok(a2a::types::GetExtendedAgentCardRequest { tenant: None }),
                Some(raw) => serde_json::from_value::<a2a::types::GetExtendedAgentCardRequest>(raw)
                    .map_err(|e| A2aError::new(-32602, format!("Invalid params: {e}"))),
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
    let bytes = match result {
        Ok(resp) => JsonRpcResponse::new(id, resp).to_bytes(),
        Err(e) => JsonRpcErrorResponse::new(id, e.code, e.message).to_bytes(),
    };
    match bytes {
        Ok(b) => {
            let headers = async_nats::HeaderMap::new();
            if let Err(e) = nats
                .publish_with_headers(async_nats::Subject::from(reply.as_str()), headers, b)
                .await
            {
                warn!(error = %e, "failed to publish agent_card reply");
            }
        }
        Err(e) => warn!(error = %e, "failed to serialize agent_card response"),
    }
}

fn parse_error() -> A2aError {
    A2aError::new(-32700, "Parse error")
}

#[cfg(test)]
mod tests;
