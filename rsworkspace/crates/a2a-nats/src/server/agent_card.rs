use a2a_pack::{AgentCardSource, accept_agent_card_on_read};
use tracing::{instrument, warn};

use crate::jsonrpc::JsonRpcId;
use crate::server::handler::{A2aError, A2aExecutor};
use crate::server::wire::{JsonRpcErrorResponse, JsonRpcResponse, parse_request};

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

    let (id, result) = parse_and_call(handler, payload).await;
    let result = match result {
        Ok(card) => match serde_json::to_value(&card) {
            Ok(v) if accept_agent_card_on_read(&v, AgentCardSource::AgentHandler) => Ok(card),
            Ok(_) => Err(A2aError::internal("AgentCard failed read validation")),
            Err(_) => Err(A2aError::internal("failed to serialize agent card for validation")),
        },
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

async fn parse_and_call<H: A2aExecutor>(
    handler: &H,
    payload: &[u8],
) -> (Option<JsonRpcId>, Result<a2a::agent_card::AgentCard, A2aError>) {
    let req = match parse_request::<a2a::types::GetExtendedAgentCardRequest>(payload) {
        Ok(r) => r,
        Err(_) => return (None, Err(A2aError::internal("parse error"))),
    };
    let id = req.id;
    let params = req
        .params
        .unwrap_or(a2a::types::GetExtendedAgentCardRequest { tenant: None });
    (id, handler.agent_card(params).await)
}

#[cfg(test)]
mod tests {
    use trogon_nats::AdvancedMockNatsClient;

    use super::*;
    use crate::server::test_support::{parse_response, rpc_payload, stub};

    fn minimal_valid_card(name: &str) -> a2a::agent_card::AgentCard {
        a2a::agent_card::AgentCard {
            name: name.to_string(),
            description: String::new(),
            version: String::new(),
            supported_interfaces: vec![a2a::agent_card::AgentInterface {
                url: "https://example.com/a2a".to_string(),
                protocol_binding: "JSONRPC".to_string(),
                protocol_version: "0.2.0".to_string(),
                tenant: None,
            }],
            capabilities: a2a::agent_card::AgentCapabilities::default(),
            default_input_modes: vec![],
            default_output_modes: vec![],
            skills: vec![],
            provider: None,
            documentation_url: None,
            icon_url: None,
            security_schemes: None,
            security_requirements: None,
            signatures: None,
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
    async fn handler_error_response_uses_typed_code() {
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
    async fn no_reply_drops_request() {
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
        handler.lock().unwrap().agent_card_result = Some(Ok(a2a::agent_card::AgentCard {
            name: String::new(),
            description: String::new(),
            version: String::new(),
            supported_interfaces: vec![],
            capabilities: a2a::agent_card::AgentCapabilities::default(),
            default_input_modes: vec![],
            default_output_modes: vec![],
            skills: vec![],
            provider: None,
            documentation_url: None,
            icon_url: None,
            security_schemes: None,
            security_requirements: None,
            signatures: None,
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

    #[tokio::test]
    async fn parse_error_returns_internal_error() {
        let nats = AdvancedMockNatsClient::new();
        let handler = stub();
        handle(&handler, b"not json", Some("r".into()), &nats).await;
        let body = parse_response(&nats.published_payloads()[0]);
        assert_eq!(body["error"]["code"], -32603);
    }
}
