use bytes::Bytes;
use jsonrpc_nats::RequestId;
use trogon_nats::AdvancedMockNatsClient;
use trogon_nats::jetstream::mocks::MockJetStreamPublisher;

use super::*;
use crate::server::handler::A2aError;
use crate::server::test_support::{parse_published_response, rpc_payload, stub, wire_request};

fn prefix() -> A2aPrefix {
    A2aPrefix::new("a2a").unwrap()
}

fn agent() -> A2aAgentId {
    A2aAgentId::new("bot").unwrap()
}

fn msg(subject: &str, reply: Option<&str>, headers: async_nats::HeaderMap, payload: &[u8]) -> async_nats::Message {
    async_nats::Message {
        subject: subject.into(),
        reply: reply.map(|r| r.into()),
        payload: Bytes::copy_from_slice(payload),
        headers: Some(headers),
        status: None,
        description: None,
        length: payload.len(),
    }
}

#[tokio::test]
async fn dispatch_routes_agent_card_to_handler() {
    let nats = AdvancedMockNatsClient::new();
    let js = MockJetStreamPublisher::new();
    let handler = Arc::new(stub());
    handler.lock().unwrap().agent_card_result = Some(Err(A2aError::unsupported_operation("stub")));

    let (headers, payload) = rpc_payload("agent/getAuthenticatedExtendedCard", 1);
    let prefix_len = format!("{}.agents.{}", prefix().as_str(), agent().as_str()).len();
    dispatch_message(
        &prefix(),
        &handler,
        &nats,
        &js,
        msg("a2a.agents.bot.card", Some("r"), headers, &payload),
        prefix_len,
    )
    .await;
    let body = parse_published_response(&nats, 0);
    assert_eq!(
        body["error"]["code"].as_i64(),
        Some(i64::from(crate::error::UNSUPPORTED_OPERATION))
    );
}

fn rpc_payload_for(method: &str) -> (async_nats::HeaderMap, Vec<u8>) {
    rpc_payload(method, 1)
}

async fn route_to_unsupported_handler(method_subject: &str, method: &str) -> serde_json::Value {
    let nats = AdvancedMockNatsClient::new();
    let js = MockJetStreamPublisher::new();
    let handler = Arc::new(stub());
    let prefix_len = format!("{}.agents.{}", prefix().as_str(), agent().as_str()).len();
    let (headers, payload) = rpc_payload_for(method);
    dispatch_message(
        &prefix(),
        &handler,
        &nats,
        &js,
        msg(method_subject, Some("r"), headers, &payload),
        prefix_len,
    )
    .await;
    parse_published_response(&nats, 0)
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
    let nats = AdvancedMockNatsClient::new();
    let js = MockJetStreamPublisher::new();
    let handler = Arc::new(stub());
    let (headers, payload) = wire_request(
        "message/stream",
        RequestId::String("req-1".into()),
        serde_json::json!({"message": {"messageId":"m","role":"ROLE_USER","parts":[]}}),
    );
    let prefix_len = format!("{}.agents.{}", prefix().as_str(), agent().as_str()).len();
    dispatch_message(
        &prefix(),
        &handler,
        &nats,
        &js,
        msg("a2a.agents.bot.message.stream", Some("r"), headers, &payload),
        prefix_len,
    )
    .await;
    let body = parse_published_response(&nats, 0);
    assert_eq!(
        body["error"]["code"].as_i64(),
        Some(i64::from(crate::error::UNSUPPORTED_OPERATION))
    );
}

#[tokio::test]
async fn run_with_agent_id_returns_on_shutdown() {
    let nats = AdvancedMockNatsClient::new();
    let _msg_tx = nats.inject_messages();
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
    let nats = AdvancedMockNatsClient::new();
    let tx = nats.inject_messages();
    drop(tx);
    let js = MockJetStreamPublisher::new();
    let handler = stub();
    let config = Config::for_test("a2a");
    let bridge = Bridge::new(config, handler, nats, js);
    let shutdown = CancellationToken::new();
    let result = bridge.run_with_agent_id(&agent(), shutdown).await;
    assert!(result.is_ok());
}

#[tokio::test]
async fn run_with_agent_id_dispatches_injected_message_then_exits_on_shutdown() {
    let nats = AdvancedMockNatsClient::new();
    let tx = nats.inject_messages();
    let js = MockJetStreamPublisher::new();
    let handler = stub();
    let config = Config::for_test("a2a");
    let bridge = Bridge::new(config, handler, nats.clone(), js);
    let shutdown = CancellationToken::new();

    let shutdown_for_test = shutdown.clone();
    let handle = tokio::spawn(async move { bridge.run_with_agent_id(&agent(), shutdown_for_test).await });

    let (headers, payload) = rpc_payload_for("agent/getAuthenticatedExtendedCard");
    tx.unbounded_send(msg("a2a.agents.bot.card", Some("r"), headers, &payload))
        .unwrap();
    tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    shutdown.cancel();
    let result = handle.await.unwrap();
    assert!(result.is_ok());
    assert!(!nats.published_messages().is_empty());
}

#[tokio::test]
async fn dispatch_drops_unknown_subject_suffix() {
    let nats = AdvancedMockNatsClient::new();
    let js = MockJetStreamPublisher::new();
    let handler = Arc::new(stub());
    let prefix_len = format!("{}.agents.{}", prefix().as_str(), agent().as_str()).len();
    dispatch_message(
        &prefix(),
        &handler,
        &nats,
        &js,
        msg(
            "a2a.agents.bot.unknown.method",
            Some("r"),
            async_nats::HeaderMap::new(),
            b"{}",
        ),
        prefix_len,
    )
    .await;
    assert!(nats.published_messages().is_empty());
}

#[test]
fn bridge_error_display_and_source() {
    let inner = std::io::Error::other("denied");
    let e = BridgeError::Subscribe(Box::new(inner));
    assert_eq!(e.to_string(), "subscribe failed");
    let source = std::error::Error::source(&e).expect("source must be set");
    assert_eq!(source.to_string(), "denied");
}
