use futures::stream;
use trogon_nats::AdvancedMockNatsClient;
use trogon_nats::jetstream::mocks::{AckKindSnapshot, AckKindValue};
use trogon_std::log_capture::{CapturedEvents, LevelFilter};

use super::super::*;
use crate::gateway_test_support::{Diagnostics, ObservedMessage, event};

fn spawn(kind: StreamingIngressKind) -> StreamingIngressSpawn {
    StreamingIngressSpawn {
        kind,
        reply: "_INBOX.stream".into(),
        caller_key: CallerKey::new("alice").unwrap(),
    }
}

fn message_stream() -> StreamingIngressSpawn {
    spawn(StreamingIngressKind::MessageStream {
        req_id: ReqId::from_header("request-1"),
        last_seq: 0,
    })
}

fn headers(req_id: &str) -> async_nats::HeaderMap {
    let mut headers = async_nats::HeaderMap::new();
    headers.insert(a2a_nats::constants::REQ_ID_HEADER, req_id);
    headers
}

async fn deliver(
    client: &AdvancedMockNatsClient,
    spawn: &StreamingIngressSpawn,
    message: ObservedMessage,
    attempt: i64,
) {
    forward_stream_deliveries(
        client,
        spawn,
        CancellationToken::new(),
        stream::iter([Ok::<_, std::io::Error>(StreamingDelivery { message, attempt })]),
    )
    .await;
}

#[tokio::test]
async fn forwarding_failures_retry_twice_then_retire_the_delivery() {
    let client = AdvancedMockNatsClient::new();
    let events = CapturedEvents::new();
    let _capture = events.install(LevelFilter::DEBUG);
    for (attempt, disposition) in [
        (1, AckKindValue::Nak(None)),
        (2, AckKindValue::Nak(None)),
        (3, AckKindValue::Term),
    ] {
        let message = ObservedMessage::new(event(headers("request-1")));
        client.fail_next_publish();
        deliver(&client, &message_stream(), message.clone(), attempt).await;
        assert_eq!(message.signals(), [AckKindSnapshot::AckWith(disposition)]);
    }
    assert!(client.published_messages().is_empty());
    let failures: Vec<_> = events
        .events()
        .into_iter()
        .filter(|event| event.message() == Some("gateway streaming ingress forward to caller reply failed"))
        .collect();
    assert_eq!(failures.len(), 3);
    assert_eq!(failures[2].field("attempt"), Some("3"));
    assert_eq!(failures[2].field("disposition"), Some("Term"));
}

#[tokio::test]
async fn a_failed_acknowledgement_does_not_publish_the_payload_again() {
    let client = AdvancedMockNatsClient::new();
    let message = ObservedMessage::with_failing_signals(event(headers("request-1")));
    deliver(&client, &message_stream(), message.clone(), 1).await;
    assert_eq!(
        client.published_payloads(),
        [bytes::Bytes::from_static(b"event payload")]
    );
    assert_eq!(
        message.signals(),
        [AckKindSnapshot::DoubleAck, AckKindSnapshot::AckWith(AckKindValue::Term)]
    );
}

#[tokio::test]
async fn demultiplexing_and_resubscription_keep_distinct_request_attribution() {
    let diagnostics = Diagnostics::both_outputs();
    let client = AdvancedMockNatsClient::new();
    let unrelated = ObservedMessage::new(event(headers("original-request")));
    deliver(&client, &message_stream(), unrelated.clone(), 1).await;
    assert_eq!(unrelated.signals(), [AckKindSnapshot::Ack]);
    assert!(client.published_messages().is_empty());

    let resubscribe = spawn(StreamingIngressKind::TasksResubscribe {
        req_id: ReqId::from_header("resume-request"),
        task_id: A2aTaskId::new("task-1").unwrap(),
        last_seq: 8,
    });
    let resumed = ObservedMessage::new(event(headers("original-request")));
    deliver(&client, &resubscribe, resumed.clone(), 1).await;
    assert_eq!(resumed.signals(), [AckKindSnapshot::DoubleAck]);
    assert_eq!(client.published_messages(), ["_INBOX.stream"]);
    diagnostics.assert_event(
        "gateway streaming ingress pump started",
        &[("req_id", "request-1"), ("method", "message_stream")],
    );
    diagnostics.assert_event(
        "gateway streaming ingress pump started",
        &[("req_id", "resume-request"), ("method", "tasks_resubscribe")],
    );
}

#[tokio::test]
async fn a_fetch_error_stops_before_the_next_delivery() {
    let client = AdvancedMockNatsClient::new();
    let message = ObservedMessage::new(event(headers("request-1")));
    let events = CapturedEvents::new();
    let _capture = events.install(LevelFilter::DEBUG);
    forward_stream_deliveries(
        &client,
        &message_stream(),
        CancellationToken::new(),
        stream::iter([
            Err(std::io::Error::other("consumer removed")),
            Ok(StreamingDelivery {
                message: message.clone(),
                attempt: 1,
            }),
        ]),
    )
    .await;
    assert!(message.signals().is_empty());
    assert!(client.published_messages().is_empty());
    assert!(events.events().iter().any(
        |event| event.message() == Some("gateway streaming ingress fetch item error")
            && event.field("error") == Some("consumer removed")
    ));
}
