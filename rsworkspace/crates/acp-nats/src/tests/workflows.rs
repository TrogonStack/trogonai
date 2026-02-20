/// Integration workflow tests
///
/// This module contains end-to-end workflow tests that exercise the full
/// ACP protocol flow, including initialization, session management, and
/// streaming responses.
use super::*;
use crate::agent::Bridge;
use crate::{FlushClient, PublishClient, RequestClient};
use agent_client_protocol::{PromptResponse, StopReason};

#[tokio::test]
async fn test_bridge_with_mock_nats() {
    let mock_nats = MockNatsClient::new();
    let _bridge = Bridge::<MockNatsClient>::new(Some(mock_nats.clone()), "acp".to_string());
    assert_eq!(mock_nats.published_messages().len(), 0);
}

#[tokio::test]
async fn test_publish_through_mock() {
    let mock_nats = MockNatsClient::new();
    let headers = async_nats::HeaderMap::new();
    let payload = bytes::Bytes::from(vec![
        0x0a, 0x0b, 0x08, 0x01, 0x10, 0x01, 0x12, 0x06, 0x43, 0x75, 0x72, 0x73, 0x6f, 0x72,
    ]);
    mock_nats
        .publish_with_headers("acp.agent.initialize", headers, payload)
        .await
        .unwrap();
    let messages = mock_nats.published_messages();
    assert_eq!(messages.len(), 1);
    assert_eq!(messages[0], "acp.agent.initialize");
}

#[tokio::test]
async fn test_bridge_requires_nats_when_none() {
    let bridge = Bridge::<MockNatsClient>::new(None, "acp".to_string());
    let result = bridge.require_nats();
    assert!(result.is_err());
    assert!(
        result
            .unwrap_err()
            .to_string()
            .contains("NATS connection unavailable")
    );
}

#[tokio::test]
async fn test_bridge_requires_nats_when_some() {
    let mock_nats = MockNatsClient::new();
    let bridge = Bridge::<MockNatsClient>::new(Some(mock_nats), "acp".to_string());
    let result = bridge.require_nats();
    assert!(result.is_ok());
}

#[tokio::test]
async fn test_request_with_headers_success() {
    let mock = AdvancedMockNatsClient::new();
    let response = bytes::Bytes::from(vec![0x0a, 0x0a, 0x08, 0x01, 0x10, 0x01, 0x20, 0x01]);
    mock.set_response("acp.agent.initialize", response);
    let headers = async_nats::HeaderMap::new();
    let payload = bytes::Bytes::from(vec![
        0x0a, 0x0b, 0x08, 0x01, 0x10, 0x01, 0x12, 0x06, 0x43, 0x75, 0x72, 0x73, 0x6f, 0x72,
    ]);
    let result = mock
        .request_with_headers("acp.agent.initialize", headers, payload)
        .await;
    assert!(result.is_ok());
    let message = result.unwrap();
    assert!(!message.payload.is_empty());
    assert_eq!(message.payload.len(), 8);
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
        let payload = bytes::Bytes::from(vec![0x12, 0x04, 0x74, 0x65, 0x73, 0x74]);
        mock.publish_with_headers(subject, headers.clone(), payload)
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
    let bridge = Bridge::<MockNatsClient>::new(Some(MockNatsClient::new()), "acp".to_string());
    let session_id: agent_client_protocol::SessionId =
        "550e8400-e29b-41d4-a716-446655440001".into();
    assert!(!bridge.cancelled_sessions.is_cancelled(&session_id));
    bridge.cancelled_sessions.mark_cancelled(session_id.clone());
    assert!(bridge.cancelled_sessions.is_cancelled(&session_id));
    let was_cancelled = bridge.cancelled_sessions.clear_cancellation(&session_id);
    assert!(was_cancelled);
    assert!(!bridge.cancelled_sessions.is_cancelled(&session_id));
}

#[tokio::test]
async fn test_pending_prompt_responses() {
    let bridge = Bridge::<MockNatsClient>::new(Some(MockNatsClient::new()), "acp".to_string());
    let session_id1: agent_client_protocol::SessionId =
        "550e8400-e29b-41d4-a716-446655440001".into();
    let session_id2: agent_client_protocol::SessionId =
        "550e8400-e29b-41d4-a716-446655440002".into();
    let _receiver1 = bridge
        .pending_session_prompt_responses
        .register_waiter(session_id1.clone());
    let _receiver2 = bridge
        .pending_session_prompt_responses
        .register_waiter(session_id2.clone());
    let removed = bridge
        .pending_session_prompt_responses
        .remove_waiter(&session_id1);
    assert!(removed);
    let removed_again = bridge
        .pending_session_prompt_responses
        .remove_waiter(&session_id1);
    assert!(!removed_again);
}

#[tokio::test]
async fn test_zero_cost_abstraction_concrete_types() {
    let mock1 = MockNatsClient::new();
    let bridge1: Bridge<MockNatsClient> = Bridge::new(Some(mock1), "acp".to_string());
    let mock2 = AdvancedMockNatsClient::new();
    let bridge2: Bridge<AdvancedMockNatsClient> = Bridge::new(Some(mock2), "acp".to_string());
    assert!(bridge1.require_nats().is_ok());
    assert!(bridge2.require_nats().is_ok());
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
async fn test_acp_initialization_with_named_handler() {
    let bridge_config = BridgeConfiguration::standard_acp_bridge();
    let handler = bridge_config
        .find_handler_for_subject("acp.agent.initialize")
        .expect("InitializeHandler must be listening on acp.agent.initialize");
    assert_eq!(handler.name, "InitializeHandler");
    let mock = AdvancedMockNatsClient::new();
    let request_builder = AcpInitializeRequestBuilder::new().with_client("zed", "Zed", "0.220.7");
    let request_payload = request_builder.build_payload();
    let request_json = request_builder.build_json();
    let response_builder = AcpInitializeResponseBuilder::success();
    let response_payload = response_builder.build_payload();
    let response_json = response_builder.build_json();
    mock.set_response("acp.agent.initialize", response_payload);
    let headers = async_nats::HeaderMap::new();
    let result = mock
        .request_with_headers("acp.agent.initialize", headers, request_payload)
        .await;
    assert!(result.is_ok(), "Initialize request should succeed");
    let message = result.unwrap();
    assert!(!message.payload.is_empty(), "Response should contain data");
    assert_eq!(request_json["method"], "initialize");
    assert_eq!(request_json["params"]["clientInfo"]["name"], "zed");
    assert_eq!(
        response_json["params"]["serverCapabilities"]["prompts"],
        true
    );
}

#[tokio::test]
async fn test_acp_initialization_no_listener_timeout() {
    let bridge_config = BridgeConfiguration::standard_acp_bridge();
    let _handler = bridge_config
        .find_handler_for_subject("acp.agent.initialize")
        .expect("InitializeHandler must be configured");
    let mock = AdvancedMockNatsClient::new();
    let request_builder = AcpInitializeRequestBuilder::new();
    let request_payload = request_builder.build_payload();
    let error_response = AcpInitializeResponseBuilder::error(
        -32603,
        "Request to 'acp.agent.initialize' timed out. The backend may be overloaded or unresponsive.",
    );
    let response_payload = error_response.build_payload();
    let error_json = error_response.build_json();
    mock.set_response("acp.agent.initialize", response_payload);
    let headers = async_nats::HeaderMap::new();
    let result = mock
        .request_with_headers("acp.agent.initialize", headers, request_payload)
        .await;
    assert!(
        result.is_ok(),
        "Request should complete (response is an error)"
    );
    let message = result.unwrap();
    assert!(
        !message.payload.is_empty(),
        "Error response should contain error details"
    );
    assert_eq!(error_json["params"]["code"], -32603);
    assert!(
        error_json["params"]["message"]
            .as_str()
            .unwrap()
            .contains("timed out")
    );
}

#[tokio::test]
async fn test_acp_multi_client_initialization_same_handler() {
    let bridge_config = BridgeConfiguration::standard_acp_bridge();
    let _init_handler = bridge_config
        .find_handler_for_subject("acp.agent.initialize")
        .expect("InitializeHandler required");
    let mock = MockNatsClient::new();
    let clients = vec![
        ("zed", "Zed", "0.220.7"),
        ("cursor", "Cursor", "0.1.0"),
        ("aider", "Aider", "0.35.0"),
    ];
    for (name, title, version) in clients {
        let builder = AcpInitializeRequestBuilder::new().with_client(name, title, version);
        let payload = builder.build_payload();
        let headers = async_nats::HeaderMap::new();
        mock.publish_with_headers("acp.agent.initialize", headers, payload)
            .await
            .unwrap();
    }
    let messages = mock.published_messages();
    assert_eq!(messages.len(), 3, "Should have 3 client initializations");
}

#[tokio::test]
async fn test_acp_session_prompt_with_named_handlers() {
    let session_id = "550e8400-e29b-41d4-a716-446655440001";
    let bridge_config = BridgeConfiguration::standard_acp_bridge().with_session_handler(session_id);
    let session_handler = bridge_config
        .find_handler_for_subject(&format!("acp.{}.agent.session.prompt", session_id))
        .expect("SessionPromptHandler must exist for this session");
    assert_eq!(session_handler.handler_type, HandlerType::SessionPrompt);
    let mock = AdvancedMockNatsClient::new();
    let subject = format!("acp.{}.agent.session.prompt", session_id);
    let prompt_payload = bytes::Bytes::from(vec![0x0a, 0x06, 0x63, 0x6f, 0x64, 0x65, 0x67, 0x65]);
    let response_payload = bytes::Bytes::from(vec![0x12, 0x05, 0x68, 0x65, 0x6c, 0x6c, 0x6f]);
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
async fn test_acp_handler_routing_verification() {
    let bridge_config = BridgeConfiguration::standard_acp_bridge();
    let init_subject = "acp.agent.initialize";
    let init_handler = bridge_config.find_handler_for_subject(init_subject);
    assert!(init_handler.is_some(), "No handler for {}", init_subject);
    let auth_subject = "acp.agent.authenticate";
    let auth_handler = bridge_config.find_handler_for_subject(auth_subject);
    assert!(auth_handler.is_some(), "No handler for {}", auth_subject);
    let unknown_subject = "acp.unknown.command";
    let unknown_handler = bridge_config.find_handler_for_subject(unknown_subject);
    assert!(
        unknown_handler.is_none(),
        "Should have no handler for {}",
        unknown_subject
    );
    let session_id = "550e8400-e29b-41d4-a716-446655440001";
    let _session_config =
        BridgeConfiguration::standard_acp_bridge().with_session_handler(session_id);
    let session_subject = format!("acp.{}.agent.session.prompt", session_id);
    let session_handler = _session_config.find_handler_for_subject(&session_subject);
    assert!(
        session_handler.is_some(),
        "No handler for {}",
        session_subject
    );
}

#[tokio::test]
async fn test_prompt_cancel_resolves_waiter() {
    let bridge = Bridge::<MockNatsClient>::new(Some(MockNatsClient::new()), "acp".to_string());
    let session_id: agent_client_protocol::SessionId =
        "550e8400-e29b-41d4-a716-446655440010".into();

    let rx = bridge
        .pending_session_prompt_responses
        .register_waiter(session_id.clone());

    bridge
        .pending_session_prompt_responses
        .resolve_waiter(&session_id, PromptResponse::new(StopReason::Cancelled));

    let response = rx.await.expect("Receiver should get the cancelled response");
    assert_eq!(response.stop_reason, StopReason::Cancelled);
}

#[tokio::test]
async fn test_prompt_response_resolves_waiter() {
    let bridge = Bridge::<MockNatsClient>::new(Some(MockNatsClient::new()), "acp".to_string());
    let session_id: agent_client_protocol::SessionId =
        "550e8400-e29b-41d4-a716-446655440011".into();

    let rx = bridge
        .pending_session_prompt_responses
        .register_waiter(session_id.clone());

    bridge
        .pending_session_prompt_responses
        .resolve_waiter(&session_id, PromptResponse::new(StopReason::EndTurn));

    let response = rx.await.expect("Receiver should get the response");
    assert_eq!(response.stop_reason, StopReason::EndTurn);
}

#[tokio::test]
async fn test_duplicate_waiter_replaces_old() {
    let bridge = Bridge::<MockNatsClient>::new(Some(MockNatsClient::new()), "acp".to_string());
    let session_id: agent_client_protocol::SessionId =
        "550e8400-e29b-41d4-a716-446655440012".into();

    let rx_old = bridge
        .pending_session_prompt_responses
        .register_waiter(session_id.clone());

    let rx_new = bridge
        .pending_session_prompt_responses
        .register_waiter(session_id.clone());

    assert!(
        rx_old.await.is_err(),
        "Old receiver should see channel-closed error"
    );

    bridge
        .pending_session_prompt_responses
        .resolve_waiter(&session_id, PromptResponse::new(StopReason::EndTurn));

    let response = rx_new.await.expect("New receiver should get the response");
    assert_eq!(response.stop_reason, StopReason::EndTurn);
}
