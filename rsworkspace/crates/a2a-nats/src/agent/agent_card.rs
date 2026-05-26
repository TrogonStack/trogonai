use a2a_pack::{AgentCardSource, accept_agent_card_on_read};
use tracing::{instrument, warn};

use crate::agent::handler::{A2aError, A2aHandler};
use crate::agent::wire::{JsonRpcErrorResponse, JsonRpcResponse, parse_request};
use crate::jsonrpc::JsonRpcId;

#[instrument(name = "a2a.agent.agent_card", skip(handler, payload, reply_subject, nats))]
pub async fn handle<H, N>(handler: &H, payload: &[u8], reply_subject: Option<String>, nats: &N)
where
    H: A2aHandler,
    N: trogon_nats::PublishClient,
{
    let Some(reply) = reply_subject else {
        warn!("agent/getAuthenticatedExtendedCard received without reply subject; dropping");
        return;
    };

    let (id, result) = parse_and_call(handler, payload).await;
    let result = match result {
        Ok(card) => {
            let value = match serde_json::to_value(&card) {
                Ok(v) => v,
                Err(_) => {
                    return publish_error(
                        id,
                        reply,
                        nats,
                        A2aError::internal("failed to serialize agent card for validation"),
                    )
                    .await;
                }
            };
            if accept_agent_card_on_read(&value, AgentCardSource::AgentHandler) {
                Ok(card)
            } else {
                Err(A2aError::internal("AgentCard failed read validation"))
            }
        }
        Err(e) => Err(e),
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

async fn parse_and_call<H: A2aHandler>(
    handler: &H,
    payload: &[u8],
) -> (Option<JsonRpcId>, Result<a2a_types::AgentCard, A2aError>) {
    let req = match parse_request::<a2a_types::GetExtendedAgentCardRequest>(payload) {
        Ok(r) => r,
        Err(_) => return (None, Err(A2aError::internal("parse error"))),
    };
    let id = req.id;
    let params = req.params.unwrap_or_default();
    (id, handler.agent_card(params).await)
}

async fn publish_error<N>(
    id: Option<JsonRpcId>,
    reply: String,
    nats: &N,
    error: A2aError,
) where
    N: trogon_nats::PublishClient,
{
    let bytes = match JsonRpcErrorResponse::new(id, error.code, error.message).to_bytes() {
        Ok(b) => b,
        Err(e) => {
            warn!(error = %e, "failed to serialize agent_card error response");
            return;
        }
    };
    let headers = async_nats::HeaderMap::new();
    if let Err(e) = nats
        .publish_with_headers(async_nats::Subject::from(reply.as_str()), headers, bytes)
        .await
    {
        warn!(error = %e, "failed to publish agent_card error reply");
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::test_support::{parse_response, rpc_payload, stub};
    use trogon_nats::AdvancedMockNatsClient;

    fn minimal_valid_card(name: &str) -> a2a_types::AgentCard {
        a2a_types::AgentCard {
            name: name.to_string(),
            supported_interfaces: vec![a2a_types::AgentInterface {
                url: "https://example.com/a2a".to_string(),
                protocol_binding: "JSONRPC".to_string(),
                protocol_version: "0.2.0".to_string(),
                tenant: String::new(),
            }],
            ..Default::default()
        }
    }

    #[tokio::test]
    async fn success_publishes_agent_card() {
        let nats = AdvancedMockNatsClient::new();
        let handler = stub();
        handler.lock().unwrap().agent_card_result = Some(Ok(minimal_valid_card("my-agent")));
        handle(
            &handler,
            &rpc_payload("agent/getAuthenticatedExtendedCard", 1),
            Some("r".into()),
            &nats,
        )
        .await;
        let body = parse_response(&nats.published_payloads()[0]);
        assert_eq!(body["result"]["name"], "my-agent");
    }

    #[tokio::test]
    async fn error_response() {
        let nats = AdvancedMockNatsClient::new();
        let handler = stub();
        handler.lock().unwrap().agent_card_result = Some(Err(A2aError::unsupported_operation("no card")));
        handle(
            &handler,
            &rpc_payload("agent/getAuthenticatedExtendedCard", 2),
            Some("r".into()),
            &nats,
        )
        .await;
        let body = parse_response(&nats.published_payloads()[0]);
        assert_eq!(body["error"]["code"], crate::error::UNSUPPORTED_OPERATION);
    }

    #[tokio::test]
    async fn no_reply_drops() {
        let nats = AdvancedMockNatsClient::new();
        let handler = stub();
        handle(
            &handler,
            &rpc_payload("agent/getAuthenticatedExtendedCard", 3),
            None,
            &nats,
        )
        .await;
        assert!(nats.published_messages().is_empty());
    }

    #[tokio::test]
    async fn invalid_card_publishes_validation_error() {
        let nats = AdvancedMockNatsClient::new();
        let handler = stub();
        handler.lock().unwrap().agent_card_result = Some(Ok(a2a_types::AgentCard {
            name: String::new(),
            ..Default::default()
        }));
        handle(
            &handler,
            &rpc_payload("agent/getAuthenticatedExtendedCard", 9),
            Some("r".into()),
            &nats,
        )
        .await;
        let body = parse_response(&nats.published_payloads()[0]);
        assert!(body.get("error").is_some());
    }

    #[tokio::test]
    async fn request_without_params_still_calls_handler() {
        let nats = AdvancedMockNatsClient::new();
        let handler = stub();
        handler.lock().unwrap().agent_card_result = Some(Ok(minimal_valid_card("default-agent")));
        let payload = serde_json::to_vec(
            &serde_json::json!({"jsonrpc":"2.0","id":1,"method":"agent/getAuthenticatedExtendedCard"}),
        )
        .unwrap();
        handle(&handler, &payload, Some("r".into()), &nats).await;
        let body = parse_response(&nats.published_payloads()[0]);
        assert!(body.get("result").is_some());
    }
}
