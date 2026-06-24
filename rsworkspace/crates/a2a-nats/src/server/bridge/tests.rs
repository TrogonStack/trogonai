use bytes::Bytes;

use super::*;
use crate::server::test_support::stub;

fn prefix() -> A2aPrefix {
    A2aPrefix::new("a2a").unwrap()
}

fn agent() -> A2aAgentId {
    A2aAgentId::new("bot").unwrap()
}

fn msg(subject: &str, reply: Option<&str>, payload: &[u8]) -> async_nats::Message {
    async_nats::Message {
        subject: subject.into(),
        reply: reply.map(|r| r.into()),
        payload: Bytes::copy_from_slice(payload),
        headers: None,
        status: None,
        description: None,
        length: payload.len(),
    }
}

#[tokio::test]
async fn dispatch_routes_agent_card_to_handler() {
    use crate::server::handler::A2aError;
    use trogon_nats::AdvancedMockNatsClient;
    use trogon_nats::jetstream::mocks::MockJetStreamPublisher;

    let nats = AdvancedMockNatsClient::new();
    let js = MockJetStreamPublisher::new();
    let handler = Arc::new(stub());
    handler.lock().unwrap().agent_card_result = Some(Err(A2aError::unsupported_operation("stub")));

    let payload = serde_json::to_vec(&serde_json::json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "agent/getAuthenticatedExtendedCard",
        "params": {}
    }))
    .unwrap();
    let prefix_len = format!("{}.agents.{}", prefix().as_str(), agent().as_str()).len();
    dispatch_message(
        &prefix(),
        &handler,
        &nats,
        &js,
        msg("a2a.agents.bot.card", Some("r"), &payload),
        prefix_len,
    )
    .await;
    let body: serde_json::Value = serde_json::from_slice(&nats.published_payloads()[0]).unwrap();
    assert_eq!(
        body["error"]["code"].as_i64(),
        Some(i64::from(crate::error::UNSUPPORTED_OPERATION))
    );
}

fn rpc_payload_for(method: &str) -> Vec<u8> {
    serde_json::to_vec(&serde_json::json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": method,
        "params": {}
    }))
    .unwrap()
}

async fn route_to_unsupported_handler(method_subject: &str, method: &str) -> serde_json::Value {
    use trogon_nats::AdvancedMockNatsClient;
    use trogon_nats::jetstream::mocks::MockJetStreamPublisher;

    let nats = AdvancedMockNatsClient::new();
    let js = MockJetStreamPublisher::new();
    let handler = Arc::new(stub());
    let prefix_len = format!("{}.agents.{}", prefix().as_str(), agent().as_str()).len();
    dispatch_message(
        &prefix(),
        &handler,
        &nats,
        &js,
        msg(method_subject, Some("r"), &rpc_payload_for(method)),
        prefix_len,
    )
    .await;
    serde_json::from_slice(&nats.published_payloads()[0]).unwrap()
}

#[tokio::test]
async fn dispatch_routes_message_send_to_handler() {
    let body = route_to_unsupported_handler("a2a.agents.bot.message.send", "message/send").await;
    assert!(body["error"]["code"].is_i64());
}

#[tokio::test]
async fn dispatch_routes_tasks_get_to_handler() {
    let body = route_to_unsupported_handler("a2a.agents.bot.tasks.get", "tasks/get").await;
    assert!(body["error"]["code"].is_i64());
}

#[tokio::test]
async fn dispatch_routes_tasks_list_to_handler() {
    let body = route_to_unsupported_handler("a2a.agents.bot.tasks.list", "tasks/list").await;
    assert!(body["error"]["code"].is_i64());
}

#[tokio::test]
async fn dispatch_routes_tasks_cancel_to_handler() {
    let body = route_to_unsupported_handler("a2a.agents.bot.tasks.cancel", "tasks/cancel").await;
    assert!(body["error"]["code"].is_i64());
}

#[tokio::test]
async fn dispatch_routes_tasks_resubscribe_to_handler() {
    let body = route_to_unsupported_handler("a2a.agents.bot.tasks.resubscribe", "tasks/resubscribe").await;
    assert!(body["error"]["code"].is_i64());
}

#[tokio::test]
async fn dispatch_routes_push_set_to_handler() {
    let body = route_to_unsupported_handler("a2a.agents.bot.push.set", "tasks/pushNotificationConfig/set").await;
    assert!(body["error"]["code"].is_i64());
}

#[tokio::test]
async fn dispatch_routes_push_get_to_handler() {
    let body = route_to_unsupported_handler("a2a.agents.bot.push.get", "tasks/pushNotificationConfig/get").await;
    assert!(body["error"]["code"].is_i64());
}

#[tokio::test]
async fn dispatch_routes_push_list_to_handler() {
    let body = route_to_unsupported_handler("a2a.agents.bot.push.list", "tasks/pushNotificationConfig/list").await;
    assert!(body["error"]["code"].is_i64());
}

#[tokio::test]
async fn dispatch_routes_push_delete_to_handler() {
    let body = route_to_unsupported_handler("a2a.agents.bot.push.delete", "tasks/pushNotificationConfig/delete").await;
    assert!(body["error"]["code"].is_i64());
}

#[tokio::test]
async fn dispatch_routes_message_stream_to_handler() {
    use trogon_nats::AdvancedMockNatsClient;
    use trogon_nats::jetstream::mocks::MockJetStreamPublisher;

    let nats = AdvancedMockNatsClient::new();
    let js = MockJetStreamPublisher::new();
    let handler = Arc::new(stub());
    let payload = serde_json::to_vec(&serde_json::json!({
        "jsonrpc": "2.0",
        "id": "req-1",
        "method": "message/stream",
        "params": {"message": {"messageId":"m","role":"ROLE_USER","parts":[]}}
    }))
    .unwrap();
    let prefix_len = format!("{}.agents.{}", prefix().as_str(), agent().as_str()).len();
    dispatch_message(
        &prefix(),
        &handler,
        &nats,
        &js,
        msg("a2a.agents.bot.message.stream", Some("r"), &payload),
        prefix_len,
    )
    .await;
    let body: serde_json::Value = serde_json::from_slice(&nats.published_payloads()[0]).unwrap();
    // Stub handler returns Err(unsupported_operation), so we expect a JSON-RPC error.
    assert_eq!(
        body["error"]["code"].as_i64(),
        Some(i64::from(crate::error::UNSUPPORTED_OPERATION))
    );
}

#[tokio::test]
async fn run_with_agent_id_returns_on_shutdown() {
    use trogon_nats::AdvancedMockNatsClient;
    use trogon_nats::jetstream::mocks::MockJetStreamPublisher;

    let nats = AdvancedMockNatsClient::new();
    let _msg_tx = nats.inject_messages(); // pre-queue a subscription stream
    let js = MockJetStreamPublisher::new();
    let handler = stub();
    let config = Config::for_test("a2a");
    let bridge = Bridge::new(config, handler, nats, js);
    let shutdown = CancellationToken::new();
    shutdown.cancel();
    let result = bridge.run_with_agent_id(&agent(), shutdown).await;
    assert!(result.is_ok());
}

#[tokio::test]
async fn run_with_agent_id_returns_subscribe_error_when_no_subscription_queued() {
    use trogon_nats::AdvancedMockNatsClient;
    use trogon_nats::jetstream::mocks::MockJetStreamPublisher;

    let nats = AdvancedMockNatsClient::new();
    let js = MockJetStreamPublisher::new();
    let handler = stub();
    let config = Config::for_test("a2a");
    let bridge = Bridge::new(config, handler, nats, js);
    let shutdown = CancellationToken::new();
    let err = bridge.run_with_agent_id(&agent(), shutdown).await.unwrap_err();
    assert!(matches!(err, BridgeError::Subscribe(_)));
}

#[tokio::test]
async fn run_with_agent_id_returns_when_subscription_closes() {
    use trogon_nats::AdvancedMockNatsClient;
    use trogon_nats::jetstream::mocks::MockJetStreamPublisher;

    let nats = AdvancedMockNatsClient::new();
    let tx = nats.inject_messages();
    drop(tx); // close the subscription stream before run starts
    let js = MockJetStreamPublisher::new();
    let handler = stub();
    let config = Config::for_test("a2a");
    let bridge = Bridge::new(config, handler, nats, js);
    let shutdown = CancellationToken::new(); // not cancelled — exit must come from stream close
    let result = bridge.run_with_agent_id(&agent(), shutdown).await;
    assert!(result.is_ok());
}

#[tokio::test]
async fn run_with_agent_id_dispatches_injected_message_then_exits_on_shutdown() {
    use trogon_nats::AdvancedMockNatsClient;
    use trogon_nats::jetstream::mocks::MockJetStreamPublisher;

    let nats = AdvancedMockNatsClient::new();
    let tx = nats.inject_messages();
    let js = MockJetStreamPublisher::new();
    let handler = stub();
    let config = Config::for_test("a2a");
    let bridge = Bridge::new(config, handler, nats.clone(), js);
    let shutdown = CancellationToken::new();

    let shutdown_for_test = shutdown.clone();
    let handle = tokio::spawn(async move { bridge.run_with_agent_id(&agent(), shutdown_for_test).await });

    tx.unbounded_send(msg(
        "a2a.agents.bot.card",
        Some("r"),
        &rpc_payload_for("agent/getAuthenticatedExtendedCard"),
    ))
    .unwrap();
    // Yield to let the dispatch loop process the message before we cancel.
    tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    shutdown.cancel();
    let result = handle.await.unwrap();
    assert!(result.is_ok());
    assert!(!nats.published_messages().is_empty());
}

#[tokio::test]
async fn dispatch_drops_unknown_subject_suffix() {
    use trogon_nats::AdvancedMockNatsClient;
    use trogon_nats::jetstream::mocks::MockJetStreamPublisher;

    let nats = AdvancedMockNatsClient::new();
    let js = MockJetStreamPublisher::new();
    let handler = Arc::new(stub());
    let prefix_len = format!("{}.agents.{}", prefix().as_str(), agent().as_str()).len();
    dispatch_message(
        &prefix(),
        &handler,
        &nats,
        &js,
        msg("a2a.agents.bot.unknown.method", Some("r"), b"{}"),
        prefix_len,
    )
    .await;
    assert!(nats.published_messages().is_empty());
}

#[test]
fn bridge_error_display_and_source() {
    let inner = std::io::Error::other("denied");
    let e = BridgeError::Subscribe(Box::new(inner));
    assert!(e.to_string().contains("subscribe failed"));
    let source = std::error::Error::source(&e).expect("source must be set");
    assert!(source.to_string().contains("denied"));
}
