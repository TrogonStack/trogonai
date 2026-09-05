use futures::stream;
use trogon_nats::AdvancedMockNatsClient;
use trogon_nats::jetstream::mocks::{AckKindSnapshot, AckKindValue};
use trogon_std::log_capture::{CapturedEvents, LevelFilter};

use super::super::*;
use crate::gateway_test_support::{ObservedMessage, event};

fn routed_event() -> async_nats::Message {
    let mut headers = async_nats::HeaderMap::new();
    headers.insert(a2a_nats::constants::REQ_ID_HEADER, "request-1");
    headers.insert(a2a_nats::constants::GATEWAY_CALLER_ID_HEADER, "alice");
    event(headers)
}

async fn deliver(
    client: &AdvancedMockNatsClient,
    attempts: &Arc<ForwardAttempts>,
    message: ObservedMessage,
    sequence: u64,
) {
    let gate = Arc::new(CallerInflightGate::new(1));
    forward_egress_deliveries(
        client,
        &A2aPrefix::new("a2a").unwrap(),
        &BaselineTaskEventsEgressPlanner::new(),
        gate.clone(),
        attempts.clone(),
        CancellationToken::new(),
        stream::iter([Ok::<_, std::io::Error>(EgressDelivery { message, sequence })]),
    )
    .await
    .unwrap();
    tokio::time::timeout(Duration::from_secs(1), async {
        while !gate.inflight.lock().unwrap().is_empty() {
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("forward task releases its caller permit");
}

#[tokio::test]
async fn retries_share_one_message_budget_even_when_acknowledgements_fail() {
    let client = AdvancedMockNatsClient::new();
    let attempts = Arc::new(ForwardAttempts::new());
    let events = CapturedEvents::new();
    let _capture = events.install(LevelFilter::DEBUG);
    for (attempt, disposition) in [
        (1, AckKindValue::Nak(None)),
        (2, AckKindValue::Nak(None)),
        (3, AckKindValue::Term),
    ] {
        let message = if attempt == 2 {
            ObservedMessage::new(routed_event())
        } else {
            ObservedMessage::with_failing_signals(routed_event())
        };
        client.fail_next_publish();
        deliver(&client, &attempts, message.clone(), 41).await;
        assert_eq!(message.signals(), [AckKindSnapshot::AckWith(disposition)]);
        assert_eq!(
            attempts.by_sequence.lock().unwrap().get(&41).copied(),
            (attempt < 3).then_some(attempt)
        );
    }
    assert!(client.published_messages().is_empty());
    let captured = events.events();
    assert!(
        captured
            .iter()
            .any(|event| event.message() == Some("gateway events pull nak failed; jetstream will redeliver"))
    );
    assert!(
        captured
            .iter()
            .any(|event| event.message() == Some("gateway events pull term failed; jetstream will redeliver"))
    );
}

#[tokio::test]
async fn a_successful_terminal_ack_clears_an_exhausted_retry_budget() {
    let client = AdvancedMockNatsClient::new();
    let attempts = Arc::new(ForwardAttempts::new());
    attempts.record_attempt(41);
    attempts.record_attempt(41);
    let message = ObservedMessage::new(routed_event());
    client.fail_next_publish();

    deliver(&client, &attempts, message.clone(), 41).await;

    assert_eq!(message.signals(), [AckKindSnapshot::AckWith(AckKindValue::Term)]);
    assert!(attempts.by_sequence.lock().unwrap().is_empty());
    assert!(client.published_messages().is_empty());
}

#[tokio::test]
async fn a_failed_double_ack_retires_state_without_duplicate_publication() {
    let client = AdvancedMockNatsClient::new();
    let attempts = Arc::new(ForwardAttempts::new());
    attempts.record_attempt(41);
    let message = ObservedMessage::with_failing_signals(routed_event());
    deliver(&client, &attempts, message.clone(), 41).await;
    assert_eq!(client.published_messages(), ["a2a.v1.gateway.alice.events"]);
    assert_eq!(
        client.published_payloads(),
        [bytes::Bytes::from_static(b"event payload")]
    );
    assert_eq!(
        message.signals(),
        [AckKindSnapshot::DoubleAck, AckKindSnapshot::AckWith(AckKindValue::Term)]
    );
    assert!(attempts.by_sequence.lock().unwrap().is_empty());
}

#[tokio::test]
async fn unroutable_and_backpressured_ack_failures_preserve_context_and_retry_budget() {
    let client = AdvancedMockNatsClient::new();
    let attempts = Arc::new(ForwardAttempts::new());
    let gate = Arc::new(CallerInflightGate::new(1));
    let held = gate.clone().try_acquire("alice").unwrap();
    for (message, expected) in [
        (
            ObservedMessage::with_failing_signals(event(async_nats::HeaderMap::new())),
            AckKindValue::Term,
        ),
        (
            ObservedMessage::with_failing_signals(routed_event()),
            AckKindValue::Nak(Some(GATE_NAK_DELAY)),
        ),
    ] {
        let error = forward_egress_deliveries(
            &client,
            &A2aPrefix::new("a2a").unwrap(),
            &BaselineTaskEventsEgressPlanner::new(),
            gate.clone(),
            attempts.clone(),
            CancellationToken::new(),
            stream::iter([Ok::<_, std::io::Error>(EgressDelivery {
                message: message.clone(),
                sequence: 41,
            })]),
        )
        .await
        .unwrap_err();
        match error {
            PullCycleError::Ack { subject, source } => {
                assert_eq!(subject, "a2a.v1.tasks.task-1.events");
                assert_eq!(source.to_string(), "ack_with failed");
            }
            error => panic!("unexpected error: {error}"),
        }
        assert_eq!(message.signals(), [AckKindSnapshot::AckWith(expected)]);
        assert!(attempts.by_sequence.lock().unwrap().is_empty());
        assert!(client.published_messages().is_empty());
        assert!(gate.clone().try_acquire("bob").is_some());
    }
    drop(held);
    assert!(gate.try_acquire("alice").is_some());
}

#[tokio::test]
async fn a_fetch_error_is_returned_before_any_forward_task_is_spawned() {
    let client = AdvancedMockNatsClient::new();
    let message = ObservedMessage::new(routed_event());
    let error = forward_egress_deliveries(
        &client,
        &A2aPrefix::new("a2a").unwrap(),
        &BaselineTaskEventsEgressPlanner::new(),
        Arc::new(CallerInflightGate::new(1)),
        Arc::new(ForwardAttempts::new()),
        CancellationToken::new(),
        stream::iter([
            Err(std::io::Error::other("consumer removed")),
            Ok(EgressDelivery {
                message: message.clone(),
                sequence: 41,
            }),
        ]),
    )
    .await
    .unwrap_err();
    match error {
        PullCycleError::FetchItem { source, .. } => assert_eq!(source.to_string(), "consumer removed"),
        error => panic!("unexpected error: {error}"),
    }
    assert!(message.signals().is_empty());
    assert!(client.published_messages().is_empty());
}

#[test]
fn poisoned_attempt_counters_still_retire_completed_messages() {
    let attempts = ForwardAttempts::new();
    attempts.record_attempt(41);
    let _ = std::panic::catch_unwind(|| {
        let _guard = attempts.by_sequence.lock().unwrap();
        panic!("interrupted counter update");
    });
    assert_eq!(attempts.record_attempt(41), 2);
    attempts.clear(41);
    assert_eq!(attempts.record_attempt(41), 1);
}
