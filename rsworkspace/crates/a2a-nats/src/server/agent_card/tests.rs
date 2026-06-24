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
    let payload =
        serde_json::to_vec(&serde_json::json!({"jsonrpc":"2.0","id":1,"method":"agent/getAuthenticatedExtendedCard"}))
            .unwrap();
    handle(&handler, &payload, Some("r".into()), &nats).await;
    let body = parse_response(&nats.published_payloads()[0]);
    assert!(body.get("result").is_some());
}

#[tokio::test]
async fn invalid_params_shape_returns_invalid_params_code() {
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
    assert_eq!(body["error"]["code"], -32602);
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
