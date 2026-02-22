/// Integration workflow tests
///
/// This module contains end-to-end workflow tests that exercise the full
/// ACP protocol flow, including initialization, session management, and
/// streaming responses.
use super::*;
use crate::agent::Bridge;
use crate::config::Config;
use crate::{FlushClient, PublishClient, RequestClient};
use crate::tests::noop_meter;
use agent_client_protocol::{PromptResponse, StopReason};
use trogon_std::time::MockClock;

#[tokio::test]
async fn test_bridge_with_mock_nats() {
    let mock_nats = MockNatsClient::new();
    let _bridge = Bridge::new(
        mock_nats.clone(),
        MockClock::new(),
        &noop_meter(),
        Config::for_test("acp"),
    );
    assert_eq!(mock_nats.published_messages().len(), 0);
}

#[tokio::test]
async fn test_publish_through_mock() {
    let mock_nats = MockNatsClient::new();
    let headers = async_nats::HeaderMap::new();
    let payload = serde_json::to_vec(&serde_json::json!({
        "protocolVersion": 1,
        "clientInfo": { "name": "Cursor" }
    }))
    .unwrap();
    mock_nats
        .publish_with_headers("acp.agent.initialize", headers, payload.into())
        .await
        .unwrap();
    let messages = mock_nats.published_messages();
    assert_eq!(messages.len(), 1);
    assert_eq!(messages[0], "acp.agent.initialize");
}

#[tokio::test]
async fn test_request_with_headers_success() {
    let mock = AdvancedMockNatsClient::new();
    let response_json = serde_json::to_vec(&serde_json::json!({
        "protocolVersion": 1,
        "serverCapabilities": { "prompts": true }
    }))
    .unwrap();
    mock.set_response("acp.agent.initialize", response_json.clone().into());
    let headers = async_nats::HeaderMap::new();
    let request_json = serde_json::to_vec(&serde_json::json!({
        "protocolVersion": 1,
        "clientInfo": { "name": "Cursor" }
    }))
    .unwrap();
    let result = mock
        .request_with_headers("acp.agent.initialize", headers, request_json.into())
        .await;
    assert!(result.is_ok());
    let message = result.unwrap();
    assert!(!message.payload.is_empty());
    assert_eq!(message.payload.len(), response_json.len());
}

#[tokio::test]
async fn test_request_with_headers_failure() {
    let mock = AdvancedMockNatsClient::new();
    mock.fail_next_request();
    let headers = async_nats::HeaderMap::new();
    let payload = bytes::Bytes::from("test payload");
    let result = mock
        .request_with_headers("test.request", headers, payload)
        .await;
    assert!(result.is_err());
    assert!(
        result
            .unwrap_err()
            .to_string()
            .contains("simulated request failure")
    );
}

#[tokio::test]
async fn test_request_missing_response() {
    let mock = AdvancedMockNatsClient::new();
    let headers = async_nats::HeaderMap::new();
    let payload = bytes::Bytes::from("test payload");
    let result = mock
        .request_with_headers("unknown.subject", headers, payload)
        .await;
    assert!(result.is_err());
    assert!(
        result
            .unwrap_err()
            .to_string()
            .contains("no response configured")
    );
}

#[tokio::test]
async fn test_multiple_publish_operations() {
    let mock = MockNatsClient::new();
    let headers = async_nats::HeaderMap::new();
    let session_ids = vec![
        "550e8400-e29b-41d4-a716-446655440001",
        "550e8400-e29b-41d4-a716-446655440002",
        "550e8400-e29b-41d4-a716-446655440003",
        "550e8400-e29b-41d4-a716-446655440004",
        "550e8400-e29b-41d4-a716-446655440005",
    ];
    for session_id in session_ids {
        let subject = format!("acp.{}.agent.session.prompt", session_id);
        let payload = serde_json::to_vec(&serde_json::json!({
            "sessionId": session_id,
            "content": [{ "type": "text", "text": "test" }]
        }))
        .unwrap();
        mock.publish_with_headers(subject, headers.clone(), payload.into())
            .await
            .unwrap();
    }
    let messages = mock.published_messages();
    assert_eq!(messages.len(), 5);
    assert!(messages[0].contains("550e8400-e29b-41d4-a716-446655440001"));
    assert!(messages[4].contains("550e8400-e29b-41d4-a716-446655440005"));
}

#[tokio::test]
async fn test_cancelled_sessions_tracking() {
    let bridge = Bridge::new(
        MockNatsClient::new(),
        MockClock::new(),
        &noop_meter(),
        Config::for_test("acp"),
    );
    let session_id: agent_client_protocol::SessionId =
        "550e8400-e29b-41d4-a716-446655440001".into();

    // Initially not cancelled
    assert!(
        bridge
            .cancelled_sessions
            .take_if_cancelled(&session_id, &bridge.clock)
            .is_none()
    );

    // Mark as cancelled
    bridge
        .cancelled_sessions
        .mark_cancelled(session_id.clone(), &bridge.clock);

    // Now it's cancelled (and consumed)
    assert!(
        bridge
            .cancelled_sessions
            .take_if_cancelled(&session_id, &bridge.clock)
            .is_some()
    );

    // After consuming, it's gone
    assert!(
        bridge
            .cancelled_sessions
            .take_if_cancelled(&session_id, &bridge.clock)
            .is_none()
    );
}

#[tokio::test]
async fn test_pending_prompt_responses() {
    let bridge = Bridge::new(
        MockNatsClient::new(),
        MockClock::new(),
        &noop_meter(),
        Config::for_test("acp"),
    );
    let session_id1: agent_client_protocol::SessionId =
        "550e8400-e29b-41d4-a716-446655440001".into();
    let session_id2: agent_client_protocol::SessionId =
        "550e8400-e29b-41d4-a716-446655440002".into();
    let _receiver1 = bridge
        .pending_session_prompt_responses
        .register_waiter(session_id1.clone())
        .expect("first waiter registration should succeed");
    let _receiver2 = bridge
        .pending_session_prompt_responses
        .register_waiter(session_id2.clone())
        .expect("second waiter registration should succeed");
    bridge
        .pending_session_prompt_responses
        .remove_waiter(&session_id1);
    assert!(
        !bridge
            .pending_session_prompt_responses
            .has_waiter(&session_id1),
        "first remove should remove the waiter"
    );
    bridge
        .pending_session_prompt_responses
        .remove_waiter(&session_id1);
    assert!(
        !bridge
            .pending_session_prompt_responses
            .has_waiter(&session_id1),
        "second remove should remain no-op"
    );
}

#[tokio::test]
async fn test_zero_cost_abstraction_concrete_types() {
    let mock1 = MockNatsClient::new();
    let bridge1 = Bridge::new(
        mock1,
        MockClock::new(),
        &noop_meter(),
        Config::for_test("acp"),
    );
    let mock2 = AdvancedMockNatsClient::new();
    let bridge2 = Bridge::new(
        mock2,
        MockClock::new(),
        &noop_meter(),
        Config::for_test("acp"),
    );
    assert_eq!(bridge1.config.acp_prefix, "acp");
    assert_eq!(bridge2.config.acp_prefix, "acp");
}

#[tokio::test]
async fn test_flush_operation() {
    let mock = MockNatsClient::new();
    let result = mock.flush().await;
    assert!(result.is_ok());
}

#[tokio::test]
async fn test_publish_with_impl_into_string() {
    let mock = MockNatsClient::new();
    let headers = async_nats::HeaderMap::new();
    let payload = bytes::Bytes::from("test");
    mock.publish_with_headers("test.with.str", headers.clone(), payload.clone())
        .await
        .unwrap();
    let subject = "test.with.string".to_string();
    mock.publish_with_headers(subject, headers, payload)
        .await
        .unwrap();
    let messages = mock.published_messages();
    assert_eq!(messages.len(), 2);
}

#[tokio::test]
async fn test_mock_nats_request_response_round_trip() {
    let mock = AdvancedMockNatsClient::new();
    let response_payload = AcpInitializeResponseBuilder::success().build_payload();
    mock.set_response("acp.agent.initialize", response_payload);
    let headers = async_nats::HeaderMap::new();
    let request_payload = AcpInitializeRequestBuilder::new().build_payload();
    let result = mock
        .request_with_headers("acp.agent.initialize", headers, request_payload)
        .await;
    assert!(result.is_ok());
    assert!(!result.unwrap().payload.is_empty());
}

#[tokio::test]
async fn test_mock_nats_error_response_round_trip() {
    let mock = AdvancedMockNatsClient::new();
    let error_payload =
        AcpInitializeResponseBuilder::error(-32603, "Request timed out").build_payload();
    mock.set_response("acp.agent.initialize", error_payload);
    let headers = async_nats::HeaderMap::new();
    let request_payload = AcpInitializeRequestBuilder::new().build_payload();
    let result = mock
        .request_with_headers("acp.agent.initialize", headers, request_payload)
        .await;
    assert!(result.is_ok());
    assert!(!result.unwrap().payload.is_empty());
}

#[tokio::test]
async fn test_multi_client_publish() {
    let mock = MockNatsClient::new();
    let clients = vec!["zed", "cursor", "aider"];
    for name in clients {
        let builder = AcpInitializeRequestBuilder::new().with_client(name);
        let payload = builder.build_payload();
        let headers = async_nats::HeaderMap::new();
        mock.publish_with_headers("acp.agent.initialize", headers, payload)
            .await
            .unwrap();
    }
    let messages = mock.published_messages();
    assert_eq!(messages.len(), 3);
}

#[tokio::test]
async fn test_session_prompt_mock_round_trip() {
    let session_id = "550e8400-e29b-41d4-a716-446655440001";
    let mock = AdvancedMockNatsClient::new();
    let subject = format!("acp.{}.agent.session.prompt", session_id);
    let prompt_payload: bytes::Bytes = serde_json::to_vec(&serde_json::json!({
        "sessionId": session_id,
        "content": [{ "type": "text", "text": "codegen" }]
    }))
    .unwrap()
    .into();
    let response_payload: bytes::Bytes = serde_json::to_vec(&serde_json::json!({
        "stopReason": "endTurn"
    }))
    .unwrap()
    .into();
    mock.set_response(&subject, response_payload);
    let headers = async_nats::HeaderMap::new();
    let result = mock
        .request_with_headers(subject.clone(), headers, prompt_payload)
        .await
        .expect("Session prompt should succeed");
    assert!(!result.payload.is_empty());
    assert_eq!(result.subject.to_string(), subject);
}

#[tokio::test]
async fn test_prompt_cancel_resolves_waiter() {
    let bridge = Bridge::new(
        MockNatsClient::new(),
        MockClock::new(),
        &noop_meter(),
        Config::for_test("acp"),
    );
    let session_id: agent_client_protocol::SessionId =
        "550e8400-e29b-41d4-a716-446655440010".into();

    let rx = bridge
        .pending_session_prompt_responses
        .register_waiter(session_id.clone())
        .expect("waiter registration should succeed");

    bridge
        .pending_session_prompt_responses
        .resolve_waiter(&session_id, Ok(PromptResponse::new(StopReason::Cancelled)));

    let response = rx
        .await
        .expect("Receiver should get the cancelled response")
        .expect("Prompt waiter should receive success response");
    assert_eq!(response.stop_reason, StopReason::Cancelled);
}

#[tokio::test]
async fn test_prompt_response_resolves_waiter() {
    let bridge = Bridge::new(
        MockNatsClient::new(),
        MockClock::new(),
        &noop_meter(),
        Config::for_test("acp"),
    );
    let session_id: agent_client_protocol::SessionId =
        "550e8400-e29b-41d4-a716-446655440011".into();

    let rx = bridge
        .pending_session_prompt_responses
        .register_waiter(session_id.clone())
        .expect("waiter registration should succeed");

    bridge
        .pending_session_prompt_responses
        .resolve_waiter(&session_id, Ok(PromptResponse::new(StopReason::EndTurn)));

    let response = rx
        .await
        .expect("Receiver should get the response")
        .expect("Prompt waiter should receive success response");
    assert_eq!(response.stop_reason, StopReason::EndTurn);
}

#[tokio::test]
async fn test_duplicate_waiter_is_rejected() {
    let bridge = Bridge::new(
        MockNatsClient::new(),
        MockClock::new(),
        &noop_meter(),
        Config::for_test("acp"),
    );
    let session_id: agent_client_protocol::SessionId =
        "550e8400-e29b-41d4-a716-446655440012".into();

    let rx_old = bridge
        .pending_session_prompt_responses
        .register_waiter(session_id.clone())
        .expect("initial waiter registration should succeed");

    assert!(
        bridge
            .pending_session_prompt_responses
            .register_waiter(session_id.clone())
            .is_err(),
        "duplicate waiter should be rejected"
    );

    bridge
        .pending_session_prompt_responses
            .resolve_waiter(&session_id, Ok(PromptResponse::new(StopReason::EndTurn)));

    let response = rx_old
        .await
        .expect("Original waiter should get the response")
        .expect("Prompt waiter should receive success response");
    assert_eq!(response.stop_reason, StopReason::EndTurn);
}
