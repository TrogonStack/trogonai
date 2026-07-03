use super::*;
use a2a_nats::client::A2aClient;
use a2a_nats::{A2aAgentId, A2aPrefix};
use bytes::Bytes;
use jsonrpc_nats::{Message as JrpcMessage, ResponseId, encode};
use serde_json::json;
use trogon_nats::AdvancedMockNatsClient;
use trogon_nats::jetstream::mocks::{MockJetStreamConsumer, MockJetStreamConsumerFactory};

use crate::wire::RpcError;

fn make_client(
    nats: AdvancedMockNatsClient,
    js: MockJetStreamConsumerFactory,
) -> A2aClient<AdvancedMockNatsClient, MockJetStreamConsumerFactory> {
    let prefix = A2aPrefix::new("a2a").unwrap();
    let agent_id = A2aAgentId::new("bot").unwrap();
    A2aClient::new(prefix, agent_id, nats, js)
}

fn task_response(task_id: &str) -> (async_nats::HeaderMap, Bytes) {
    let task = a2a::types::Task {
        id: task_id.to_string(),
        context_id: String::new(),
        status: a2a::types::TaskStatus {
            state: a2a::types::TaskState::Completed,
            message: None,
            timestamp: None,
        },
        artifacts: None,
        history: None,
        metadata: None,
    };
    let encoded = encode(&JrpcMessage::Success {
        id: ResponseId::String("any".into()),
        result: serde_json::json!(task),
    })
    .unwrap();
    (encoded.headers, encoded.body)
}

fn send_message_response(task_id: &str) -> (async_nats::HeaderMap, Bytes) {
    let task = a2a::types::Task {
        id: task_id.to_string(),
        context_id: String::new(),
        status: a2a::types::TaskStatus {
            state: a2a::types::TaskState::Unspecified,
            message: None,
            timestamp: None,
        },
        artifacts: None,
        history: None,
        metadata: None,
    };
    let response = a2a::types::SendMessageResponse::Task(task);
    let encoded = encode(&JrpcMessage::Success {
        id: ResponseId::String("any".into()),
        result: serde_json::json!(response),
    })
    .unwrap();
    (encoded.headers, encoded.body)
}

async fn dispatch(
    client: &A2aClient<AdvancedMockNatsClient, MockJetStreamConsumerFactory>,
    id: RpcId,
    method: &str,
    params: Value,
) -> OutboundFrame {
    let (tx, mut rx) = mpsc::channel(8);
    dispatch_request(client, id, method, params, &tx).await;
    drop(tx);
    rx.recv().await.expect("expected at least one frame")
}

#[tokio::test]
async fn tasks_get_success() {
    let nats = AdvancedMockNatsClient::new();
    let (headers, body) = task_response("t1");
    nats.set_response_wire("a2a.agents.bot.tasks.get", headers, body);
    let client = make_client(nats, MockJetStreamConsumerFactory::new());
    let frame = dispatch(
        &client,
        RpcId::Number(1),
        "tasks/get",
        json!({"id": "t1", "tenant": ""}),
    )
    .await;
    assert!(matches!(frame, OutboundFrame::Response(_)));
}

#[tokio::test]
async fn tasks_get_error_maps_to_rpc_error() {
    let nats = AdvancedMockNatsClient::new();
    nats.fail_next_request();
    let client = make_client(nats, MockJetStreamConsumerFactory::new());
    let frame = dispatch(
        &client,
        RpcId::Number(2),
        "tasks/get",
        json!({"id": "t1", "tenant": ""}),
    )
    .await;
    assert!(matches!(frame, OutboundFrame::Error(_)));
}

#[tokio::test]
async fn tasks_cancel_success() {
    let nats = AdvancedMockNatsClient::new();
    let (headers, body) = task_response("tc");
    nats.set_response_wire("a2a.agents.bot.tasks.cancel", headers, body);
    let client = make_client(nats, MockJetStreamConsumerFactory::new());
    let frame = dispatch(
        &client,
        RpcId::String("x".into()),
        "tasks/cancel",
        json!({"id": "tc", "tenant": ""}),
    )
    .await;
    assert!(matches!(frame, OutboundFrame::Response(_)));
}

#[tokio::test]
async fn message_send_success() {
    let nats = AdvancedMockNatsClient::new();
    let (headers, body) = send_message_response("ms");
    nats.set_response_wire("a2a.agents.bot.message.send", headers, body);
    let client = make_client(nats, MockJetStreamConsumerFactory::new());
    let frame = dispatch(
        &client,
        RpcId::Number(3),
        "message/send",
        json!({"message": {"messageId": "m1", "role": "ROLE_USER", "parts": []}}),
    )
    .await;
    assert!(
        matches!(frame, OutboundFrame::Response(_)),
        "got: {}",
        serde_json::to_string(&frame).unwrap()
    );
}

#[tokio::test]
async fn message_stream_returns_when_bootstrap_send_fails() {
    // Drop the receiver before dispatch so the bootstrap send hits the
    // closed-channel branch — the handler must return without consuming
    // the JetStream loop (which would ack events the caller never saw).
    let nats = AdvancedMockNatsClient::new();
    let (headers, body) = send_message_response("ms-drop");
    nats.set_response_wire("a2a.agents.bot.message.stream", headers, body);

    let js = MockJetStreamConsumerFactory::new();
    let (consumer, tx) = MockJetStreamConsumer::new();
    js.add_consumer(consumer);
    drop(tx);

    let client = make_client(nats, js);
    let (chan_tx, chan_rx) = mpsc::channel(16);
    drop(chan_rx);
    // Should return promptly — if the bootstrap-drop check is missing,
    // this would hang waiting on the JetStream loop.
    tokio::time::timeout(
        std::time::Duration::from_secs(2),
        dispatch_request(
            &client,
            RpcId::Number(4),
            "message/stream",
            json!({"message": {"messageId": "m-drop", "role": "ROLE_USER", "parts": []}}),
            &chan_tx,
        ),
    )
    .await
    .expect("dispatch must exit when bootstrap send fails");
}

#[tokio::test]
async fn tasks_resubscribe_returns_when_bootstrap_send_fails() {
    let nats = AdvancedMockNatsClient::new();
    let (headers, body) = task_response("rsub-drop");
    nats.set_response_wire("a2a.agents.bot.tasks.resubscribe", headers, body);

    let js = MockJetStreamConsumerFactory::new();
    let (consumer, tx) = MockJetStreamConsumer::new();
    js.add_consumer(consumer);
    drop(tx);

    let client = make_client(nats, js);
    let (chan_tx, chan_rx) = mpsc::channel(16);
    drop(chan_rx);
    tokio::time::timeout(
        std::time::Duration::from_secs(2),
        dispatch_request(
            &client,
            RpcId::Number(5),
            "tasks/resubscribe",
            json!({"id": "rsub-drop", "lastSeq": 0}),
            &chan_tx,
        ),
    )
    .await
    .expect("dispatch must exit when bootstrap send fails");
}

#[tokio::test]
async fn message_stream_emits_bootstrap_then_events() {
    let nats = AdvancedMockNatsClient::new();
    let (headers, body) = send_message_response("ms2");
    nats.set_response_wire("a2a.agents.bot.message.stream", headers, body);

    let js = MockJetStreamConsumerFactory::new();
    let (consumer, tx) = MockJetStreamConsumer::new();
    js.add_consumer(consumer);
    drop(tx);

    let client = make_client(nats, js);
    let (chan_tx, mut chan_rx) = mpsc::channel(16);
    dispatch_request(
        &client,
        RpcId::Number(4),
        "message/stream",
        json!({"message": {"messageId": "m2", "role": "ROLE_USER", "parts": []}}),
        &chan_tx,
    )
    .await;
    drop(chan_tx);

    let first = chan_rx.recv().await.expect("bootstrap frame");
    assert!(matches!(first, OutboundFrame::Response(_)));
}

#[tokio::test]
async fn tasks_resubscribe_emits_snapshot_then_empty_stream() {
    let nats = AdvancedMockNatsClient::new();
    let (headers, body) = task_response("task1");
    nats.set_response_wire("a2a.agents.bot.tasks.resubscribe", headers, body);

    let js = MockJetStreamConsumerFactory::new();
    let (consumer, tx) = MockJetStreamConsumer::new();
    js.add_consumer(consumer);
    drop(tx);

    let client = make_client(nats, js);
    let (chan_tx, mut chan_rx) = mpsc::channel(8);
    dispatch_request(
        &client,
        RpcId::Number(5),
        "tasks/resubscribe",
        json!({"id": "task1", "lastSeq": 0}),
        &chan_tx,
    )
    .await;
    drop(chan_tx);

    let first = chan_rx.recv().await.expect("expected bootstrap frame");
    assert!(matches!(first, OutboundFrame::Response(_)));
    assert!(chan_rx.recv().await.is_none());
}

#[tokio::test]
async fn agent_card_success() {
    let nats = AdvancedMockNatsClient::new();
    let card = a2a::agent_card::AgentCard {
        name: "BotA".into(),
        description: "desc".into(),
        version: "1.0".into(),
        capabilities: a2a::agent_card::AgentCapabilities::default(),
        supported_interfaces: vec![],
        default_input_modes: vec![],
        default_output_modes: vec![],
        skills: vec![],
        provider: None,
        documentation_url: None,
        icon_url: None,
        security_schemes: None,
        security_requirements: None,
        signatures: None,
    };
    let encoded = encode(&JrpcMessage::Success {
        id: ResponseId::String("any".into()),
        result: serde_json::json!(card),
    })
    .unwrap();
    nats.set_response_wire("a2a.agents.bot.card", encoded.headers, encoded.body);
    let client = make_client(nats, MockJetStreamConsumerFactory::new());
    let frame = dispatch(
        &client,
        RpcId::Number(6),
        "agent/getAuthenticatedExtendedCard",
        json!({}),
    )
    .await;
    assert!(matches!(frame, OutboundFrame::Response(_)));
}

#[tokio::test]
async fn unknown_method_returns_method_not_found() {
    let nats = AdvancedMockNatsClient::new();
    let client = make_client(nats, MockJetStreamConsumerFactory::new());
    let frame = dispatch(&client, RpcId::Number(7), "bogus/method", json!({})).await;
    assert!(matches!(
        frame,
        OutboundFrame::Error(OutboundError {
            error: RpcError { code: -32601, .. },
            ..
        })
    ));
}

#[tokio::test]
async fn invalid_params_returns_error() {
    let nats = AdvancedMockNatsClient::new();
    let client = make_client(nats, MockJetStreamConsumerFactory::new());
    let frame = dispatch(&client, RpcId::Number(8), "tasks/get", json!("not an object")).await;
    assert!(matches!(
        frame,
        OutboundFrame::Error(OutboundError {
            error: RpcError { code: -32602, .. },
            ..
        })
    ));
}

fn err_response(code: i32, msg: &str) -> (async_nats::HeaderMap, Bytes) {
    let encoded = encode(&JrpcMessage::Error {
        id: ResponseId::String("any".into()),
        code,
        message: msg.to_string(),
        data: None,
    })
    .unwrap();
    (encoded.headers, encoded.body)
}

#[track_caller]
fn assert_err_code(frame: OutboundFrame, expected: i32) {
    let OutboundFrame::Error(OutboundError {
        error: RpcError { code, .. },
        ..
    }) = frame
    else {
        panic!("expected error frame, got non-error variant");
    };
    assert_eq!(code, expected);
}

#[tokio::test]
async fn tasks_list_success() {
    let nats = AdvancedMockNatsClient::new();
    let list = a2a::types::ListTasksResponse {
        tasks: vec![],
        next_page_token: String::new(),
        page_size: 0,
        total_size: 0,
    };
    let encoded = encode(&JrpcMessage::Success {
        id: ResponseId::String("any".into()),
        result: serde_json::json!(list),
    })
    .unwrap();
    nats.set_response_wire("a2a.agents.bot.tasks.list", encoded.headers, encoded.body);
    let client = make_client(nats, MockJetStreamConsumerFactory::new());
    let frame = dispatch(&client, RpcId::Number(1), "tasks/list", json!({})).await;
    assert!(matches!(frame, OutboundFrame::Response(_)));
}

#[tokio::test]
async fn push_set_success() {
    let nats = AdvancedMockNatsClient::new();
    let cfg = a2a::types::TaskPushNotificationConfig {
        url: "https://example.com".into(),
        id: Some("c".into()),
        task_id: "t1".into(),
        token: None,
        authentication: None,
        tenant: None,
    };
    let encoded = encode(&JrpcMessage::Success {
        id: ResponseId::String("any".into()),
        result: serde_json::json!(cfg),
    })
    .unwrap();
    nats.set_response_wire("a2a.agents.bot.push.set", encoded.headers, encoded.body);
    let client = make_client(nats, MockJetStreamConsumerFactory::new());
    let frame = dispatch(
        &client,
        RpcId::Number(2),
        "tasks/pushNotificationConfig/set",
        json!({"url":"https://example.com","id":"c","taskId":"t1"}),
    )
    .await;
    assert!(matches!(frame, OutboundFrame::Response(_)));
}

#[tokio::test]
async fn push_get_success() {
    let nats = AdvancedMockNatsClient::new();
    let cfg = a2a::types::TaskPushNotificationConfig {
        url: "https://example.com".into(),
        id: Some("c".into()),
        task_id: "t1".into(),
        token: None,
        authentication: None,
        tenant: None,
    };
    let encoded = encode(&JrpcMessage::Success {
        id: ResponseId::String("any".into()),
        result: serde_json::json!(cfg),
    })
    .unwrap();
    nats.set_response_wire("a2a.agents.bot.push.get", encoded.headers, encoded.body);
    let client = make_client(nats, MockJetStreamConsumerFactory::new());
    let frame = dispatch(
        &client,
        RpcId::Number(3),
        "tasks/pushNotificationConfig/get",
        json!({"taskId":"t1","id":"c"}),
    )
    .await;
    assert!(matches!(frame, OutboundFrame::Response(_)));
}

#[tokio::test]
async fn push_list_success() {
    let nats = AdvancedMockNatsClient::new();
    let resp = a2a::types::ListTaskPushNotificationConfigsResponse {
        configs: vec![],
        next_page_token: None,
    };
    let encoded = encode(&JrpcMessage::Success {
        id: ResponseId::String("any".into()),
        result: serde_json::json!(resp),
    })
    .unwrap();
    nats.set_response_wire("a2a.agents.bot.push.list", encoded.headers, encoded.body);
    let client = make_client(nats, MockJetStreamConsumerFactory::new());
    let frame = dispatch(
        &client,
        RpcId::Number(4),
        "tasks/pushNotificationConfig/list",
        json!({"taskId":"t1"}),
    )
    .await;
    assert!(matches!(frame, OutboundFrame::Response(_)));
}

#[tokio::test]
async fn push_delete_success() {
    let nats = AdvancedMockNatsClient::new();
    let encoded = encode(&JrpcMessage::Success {
        id: ResponseId::String("any".into()),
        result: serde_json::json!(null),
    })
    .unwrap();
    nats.set_response_wire("a2a.agents.bot.push.delete", encoded.headers, encoded.body);
    let client = make_client(nats, MockJetStreamConsumerFactory::new());
    let frame = dispatch(
        &client,
        RpcId::Number(5),
        "tasks/pushNotificationConfig/delete",
        json!({"taskId":"t1","id":"c"}),
    )
    .await;
    assert!(matches!(frame, OutboundFrame::Response(_)));
}

#[tokio::test]
async fn client_err_to_frame_maps_every_typed_variant() {
    // Each entry maps a ClientError variant produced by the agent's JSON-RPC error code
    // to the expected outbound error code.
    let cases = [
        (a2a_nats::error::TASK_NOT_FOUND, a2a_nats::error::TASK_NOT_FOUND),
        (
            a2a_nats::error::TASK_NOT_CANCELABLE,
            a2a_nats::error::TASK_NOT_CANCELABLE,
        ),
        (
            a2a_nats::error::PUSH_NOTIFICATION_NOT_SUPPORTED,
            a2a_nats::error::PUSH_NOTIFICATION_NOT_SUPPORTED,
        ),
        (
            a2a_nats::error::UNSUPPORTED_OPERATION,
            a2a_nats::error::UNSUPPORTED_OPERATION,
        ),
        (
            a2a_nats::error::CONTENT_TYPE_NOT_SUPPORTED,
            a2a_nats::error::CONTENT_TYPE_NOT_SUPPORTED,
        ),
        (
            a2a_nats::error::INVALID_AGENT_RESPONSE,
            a2a_nats::error::INVALID_AGENT_RESPONSE,
        ),
        (
            a2a_nats::error::EXTENDED_AGENT_CARD_NOT_CONFIGURED,
            a2a_nats::error::EXTENDED_AGENT_CARD_NOT_CONFIGURED,
        ),
        (
            a2a_nats::error::EXTENSION_SUPPORT_REQUIRED,
            a2a_nats::error::EXTENSION_SUPPORT_REQUIRED,
        ),
        (
            a2a_nats::error::VERSION_NOT_SUPPORTED,
            a2a_nats::error::VERSION_NOT_SUPPORTED,
        ),
        (a2a_nats::error::AGENT_UNAVAILABLE, a2a_nats::error::AGENT_UNAVAILABLE),
        (-32099, -32099), // generic JSON-RPC fallthrough preserves the upstream code
    ];
    for (input, expected) in cases {
        let nats = AdvancedMockNatsClient::new();
        let (headers, body) = err_response(input, "x");
        nats.set_response_wire("a2a.agents.bot.tasks.get", headers, body);
        let client = make_client(nats, MockJetStreamConsumerFactory::new());
        let frame = dispatch(
            &client,
            RpcId::Number(input as i64),
            "tasks/get",
            json!({"id":"t","tenant":""}),
        )
        .await;
        assert_err_code(frame, expected);
    }
}

#[tokio::test]
async fn transport_error_falls_back_to_internal_code() {
    let nats = AdvancedMockNatsClient::new();
    nats.fail_next_request();
    let client = make_client(nats, MockJetStreamConsumerFactory::new());
    let frame = dispatch(&client, RpcId::Number(99), "tasks/get", json!({"id":"t","tenant":""})).await;
    assert_err_code(frame, -32603);
}

#[tokio::test]
async fn agent_error_routes_to_outbound_error_for_every_typed_method() {
    // For each method, configure the agent to reply with a typed JSON-RPC
    // error and confirm the dispatcher forwards it as an OutboundError
    // through that method's Err arm.
    let cases = [
        (
            "a2a.agents.bot.message.send",
            "message/send",
            json!({"message": {"messageId": "m", "role": "ROLE_USER", "parts": []}}),
        ),
        ("a2a.agents.bot.tasks.list", "tasks/list", json!({})),
        (
            "a2a.agents.bot.tasks.cancel",
            "tasks/cancel",
            json!({"id": "t", "tenant": ""}),
        ),
        (
            "a2a.agents.bot.push.set",
            "tasks/pushNotificationConfig/set",
            json!({"url":"https://example.com","id":"c","taskId":"t1"}),
        ),
        (
            "a2a.agents.bot.push.get",
            "tasks/pushNotificationConfig/get",
            json!({"taskId":"t1","id":"c"}),
        ),
        (
            "a2a.agents.bot.push.list",
            "tasks/pushNotificationConfig/list",
            json!({"taskId":"t1"}),
        ),
        (
            "a2a.agents.bot.push.delete",
            "tasks/pushNotificationConfig/delete",
            json!({"taskId":"t1","id":"c"}),
        ),
        ("a2a.agents.bot.card", "agent/getAuthenticatedExtendedCard", json!({})),
    ];
    for (subject, method, params) in cases {
        let nats = AdvancedMockNatsClient::new();
        let (headers, body) = err_response(a2a_nats::error::TASK_NOT_FOUND, "missing");
        nats.set_response_wire(subject, headers, body);
        let client = make_client(nats, MockJetStreamConsumerFactory::new());
        let frame = dispatch(&client, RpcId::Number(1), method, params).await;
        assert_err_code(frame, a2a_nats::error::TASK_NOT_FOUND);
    }
}

#[tokio::test]
async fn message_stream_error_at_bootstrap_routes_to_outbound_error() {
    let nats = AdvancedMockNatsClient::new();
    let (headers, body) = err_response(a2a_nats::error::AGENT_UNAVAILABLE, "down");
    nats.set_response_wire("a2a.agents.bot.message.stream", headers, body);
    let js = MockJetStreamConsumerFactory::new();
    let (consumer, _tx) = MockJetStreamConsumer::new();
    js.add_consumer(consumer);
    let client = make_client(nats, js);
    let frame = dispatch(
        &client,
        RpcId::Number(1),
        "message/stream",
        json!({"message": {"messageId": "m", "role": "ROLE_USER", "parts": []}}),
    )
    .await;
    assert_err_code(frame, a2a_nats::error::AGENT_UNAVAILABLE);
}

#[tokio::test]
async fn tasks_resubscribe_error_at_snapshot_routes_to_outbound_error() {
    let nats = AdvancedMockNatsClient::new();
    let (headers, body) = err_response(a2a_nats::error::TASK_NOT_FOUND, "gone");
    nats.set_response_wire("a2a.agents.bot.tasks.resubscribe", headers, body);
    let js = MockJetStreamConsumerFactory::new();
    let (consumer, _tx) = MockJetStreamConsumer::new();
    js.add_consumer(consumer);
    let client = make_client(nats, js);
    let frame = dispatch(
        &client,
        RpcId::Number(1),
        "tasks/resubscribe",
        json!({"id": "missing", "lastSeq": 0}),
    )
    .await;
    assert_err_code(frame, a2a_nats::error::TASK_NOT_FOUND);
}

#[tokio::test]
async fn invalid_params_returned_for_every_typed_method() {
    let nats = AdvancedMockNatsClient::new();
    let client = make_client(nats, MockJetStreamConsumerFactory::new());
    for method in [
        "message/send",
        "message/stream",
        "tasks/list",
        "tasks/cancel",
        "tasks/resubscribe",
        "tasks/pushNotificationConfig/set",
        "tasks/pushNotificationConfig/get",
        "tasks/pushNotificationConfig/list",
        "tasks/pushNotificationConfig/delete",
    ] {
        let frame = dispatch(&client, RpcId::Number(1), method, json!("not an object")).await;
        assert_err_code(frame, -32602);
    }
}

use trogon_nats::jetstream::mocks::MockJsMessage;

fn js_msg(payload: Vec<u8>) -> MockJsMessage {
    let inner = async_nats::Message {
        subject: "a2a.tasks.task1.events.req".into(),
        reply: Some("$JS.ACK.A2A_EVENTS.consumer.1.1.1.0.0".into()),
        payload: Bytes::from(payload),
        headers: None,
        status: None,
        description: None,
        length: 0,
    };
    MockJsMessage::new(inner)
}

fn status_event(task_id: &str) -> a2a::event::StreamResponse {
    a2a::event::StreamResponse::StatusUpdate(a2a::event::TaskStatusUpdateEvent {
        task_id: task_id.to_string(),
        context_id: "ctx".into(),
        status: a2a::types::TaskStatus {
            state: a2a::types::TaskState::Working,
            message: None,
            timestamp: None,
        },
        metadata: None,
    })
}

#[tokio::test]
async fn message_stream_forwards_status_events_as_notifications() {
    let nats = AdvancedMockNatsClient::new();
    let (headers, body) = send_message_response("ms3");
    nats.set_response_wire("a2a.agents.bot.message.stream", headers, body);
    let js = MockJetStreamConsumerFactory::new();
    let (consumer, evt_tx) = MockJetStreamConsumer::new();
    js.add_consumer(consumer);
    let event_payload = serde_json::to_vec(&status_event("task-stream")).unwrap();
    evt_tx.unbounded_send(Ok(js_msg(event_payload))).unwrap();
    drop(evt_tx);
    let client = make_client(nats, js);
    let (chan_tx, mut chan_rx) = mpsc::channel(16);
    dispatch_request(
        &client,
        RpcId::Number(10),
        "message/stream",
        json!({"message": {"messageId": "m3", "role": "ROLE_USER", "parts": []}}),
        &chan_tx,
    )
    .await;
    drop(chan_tx);
    let first = chan_rx.recv().await.expect("bootstrap");
    assert!(matches!(first, OutboundFrame::Response(_)));
    let second = chan_rx.recv().await.expect("event notification");
    match second {
        OutboundFrame::Notification(n) => assert_eq!(n.method, "message/stream"),
        other => panic!("expected notification, got {other:?}"),
    }
}

#[tokio::test]
async fn tasks_resubscribe_forwards_status_events_under_resubscribe_method() {
    let nats = AdvancedMockNatsClient::new();
    let (headers, body) = task_response("rsub");
    nats.set_response_wire("a2a.agents.bot.tasks.resubscribe", headers, body);
    let js = MockJetStreamConsumerFactory::new();
    let (consumer, evt_tx) = MockJetStreamConsumer::new();
    js.add_consumer(consumer);
    let event_payload = serde_json::to_vec(&status_event("rsub")).unwrap();
    evt_tx.unbounded_send(Ok(js_msg(event_payload))).unwrap();
    drop(evt_tx);
    let client = make_client(nats, js);
    let (chan_tx, mut chan_rx) = mpsc::channel(16);
    dispatch_request(
        &client,
        RpcId::Number(11),
        "tasks/resubscribe",
        json!({"id": "rsub", "lastSeq": 0}),
        &chan_tx,
    )
    .await;
    drop(chan_tx);
    let first = chan_rx.recv().await.expect("snapshot");
    assert!(matches!(first, OutboundFrame::Response(_)));
    let second = chan_rx.recv().await.expect("event notification");
    match second {
        // Notification method MUST be tasks/resubscribe, not message/stream.
        OutboundFrame::Notification(n) => assert_eq!(n.method, "tasks/resubscribe"),
        other => panic!("expected notification, got {other:?}"),
    }
}

#[tokio::test]
async fn tasks_resubscribe_rejects_blank_id() {
    let nats = AdvancedMockNatsClient::new();
    let client = make_client(nats, MockJetStreamConsumerFactory::new());
    let frame = dispatch(
        &client,
        RpcId::Number(11),
        "tasks/resubscribe",
        json!({"id": "", "lastSeq": 0}),
    )
    .await;
    assert_err_code(frame, -32602);
}

#[test]
fn make_with_id_overwrites_error_id_and_passes_through_non_error_frames() {
    let from_parse_helper = OutboundFrame::Error(OutboundError::new(RpcId::Null, INVALID_PARAMS, "x".into()));
    let target_id = RpcId::Number(42);
    let rewritten = super::make_with_id(from_parse_helper, &target_id);
    if let OutboundFrame::Error(e) = rewritten {
        assert_eq!(e.id, RpcId::Number(42));
    }
    // Non-Error variants pass through unchanged — there's nothing to rewrite.
    let notif = OutboundFrame::Notification(OutboundNotification::new(
        RpcId::Number(1),
        "message/stream",
        Value::Null,
    ));
    let passed = super::make_with_id(notif, &target_id);
    if let OutboundFrame::Notification(n) = passed {
        assert_eq!(n.id, RpcId::Number(1));
    }
}
