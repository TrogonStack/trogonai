use a2a_pack::{AgentCardSource, accept_agent_card_on_read};
use tracing::{instrument, warn};

use crate::jsonrpc::extract_request_id;
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

    let result = match parse_request::<a2a::types::GetExtendedAgentCardRequest>(payload) {
        Ok(req) => {
            let params = req
                .params
                .unwrap_or(a2a::types::GetExtendedAgentCardRequest { tenant: None });
            match handler.agent_card(params).await {
                Ok(card) => match serde_json::to_value(&card) {
                    Ok(v) if accept_agent_card_on_read(&v, AgentCardSource::AgentHandler) => Ok(card),
                    Ok(_) => Err(A2aError::invalid_agent_response("AgentCard failed read validation")),
                    Err(_) => Err(A2aError::internal("failed to serialize agent card for validation")),
                },
                Err(e) => Err(e),
            }
        }
        Err(_) => Err(parse_error()),
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

/// True only when the payload is a parseable JSON object that omits the `id`
/// key, matching JSON-RPC 2.0's definition of a notification. Anything else —
/// non-JSON, non-object, or an `id` value the extractor couldn't decode — is a
/// malformed request that still warrants an error reply.
fn is_notification(payload: &[u8]) -> bool {
    let Ok(value) = serde_json::from_slice::<serde_json::Value>(payload) else {
        return false;
    };
    matches!(value, serde_json::Value::Object(ref map) if !map.contains_key("id"))
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
    async fn parse_error_returns_jsonrpc_parse_error_code() {
        let nats = AdvancedMockNatsClient::new();
        let handler = stub();
        let payload = serde_json::to_vec(&serde_json::json!({
            "jsonrpc": "2.0",
            "id": 7,
            "method": "agent/getAuthenticatedExtendedCard",
            "params": {"tenant": 42}
        }))
        .unwrap();
        handle(&handler, &payload, Some("r".into()), &nats).await;
        let body = parse_response(&nats.published_payloads()[0]);
        assert_eq!(body["error"]["code"], -32700);
        assert_eq!(body["id"], 7);
    }

    #[tokio::test]
    async fn malformed_json_still_publishes_parse_error_with_null_id() {
        let nats = AdvancedMockNatsClient::new();
        let handler = stub();
        handle(&handler, b"not json at all", Some("r".into()), &nats).await;
        let body = parse_response(&nats.published_payloads()[0]);
        assert_eq!(body["error"]["code"], -32700);
        assert!(body["id"].is_null());
    }

    #[tokio::test]
    async fn id_present_but_undecodable_still_publishes_error() {
        let nats = AdvancedMockNatsClient::new();
        let handler = stub();
        let payload = serde_json::to_vec(&serde_json::json!({
            "jsonrpc": "2.0",
            "id": true,
            "method": "agent/getAuthenticatedExtendedCard",
            "params": {}
        }))
        .unwrap();
        handle(&handler, &payload, Some("r".into()), &nats).await;
        assert!(!nats.published_payloads().is_empty());
    }

    #[tokio::test]
    async fn notification_without_id_is_dropped() {
        let nats = AdvancedMockNatsClient::new();
        let handler = stub();
        let payload = serde_json::to_vec(&serde_json::json!({
            "jsonrpc": "2.0",
            "method": "agent/getAuthenticatedExtendedCard",
            "params": {}
        }))
        .unwrap();
        handle(&handler, &payload, Some("r".into()), &nats).await;
        assert!(nats.published_messages().is_empty());
    }

    #[tokio::test]
    async fn invalid_card_uses_invalid_agent_response_code() {
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
            &rpc_payload("agent/getAuthenticatedExtendedCard", 11),
            Some("r".into()),
            &nats,
        )
        .await;
        let body = parse_response(&nats.published_payloads()[0]);
        assert_eq!(body["error"]["code"], crate::error::INVALID_AGENT_RESPONSE);
    }
}
