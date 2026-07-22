use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use async_nats::jetstream;
use async_nats::jetstream::consumer::{AckPolicy, pull};
use bytes::Bytes;
use tokio_util::sync::CancellationToken;
use trogon_nats::test_support::JetStreamTestServer;

use super::{HandlerVerdict, MessageHandler, PoisonReason, Processor, RedeliveryDecision, RedeliveryPolicy};

#[test]
fn redelivery_policy_retries_with_bounded_exponential_backoff() {
    let policy = RedeliveryPolicy::new(3, Duration::from_millis(100), Duration::from_secs(1));

    assert_eq!(
        policy.decide(0),
        RedeliveryDecision::Retry {
            delay: Duration::from_millis(100)
        }
    );
    assert_eq!(
        policy.decide(1),
        RedeliveryDecision::Retry {
            delay: Duration::from_millis(200)
        }
    );
    assert_eq!(policy.decide(2), RedeliveryDecision::Poison);
}

#[test]
fn redelivery_policy_caps_delay_at_max_delay() {
    let policy = RedeliveryPolicy::new(10, Duration::from_millis(100), Duration::from_millis(250));

    assert_eq!(
        policy.decide(5),
        RedeliveryDecision::Retry {
            delay: Duration::from_millis(250)
        }
    );
}

#[test]
fn redelivery_policy_reports_max_deliveries() {
    let policy = RedeliveryPolicy::new(7, Duration::from_millis(1), Duration::from_millis(1));

    assert_eq!(policy.max_deliveries(), 7);
}

async fn create_events_stream(
    js: &jetstream::Context,
    name: &str,
    subject_wildcard: &str,
) -> jetstream::stream::Stream {
    js.create_stream(jetstream::stream::Config {
        name: name.to_string(),
        subjects: vec![subject_wildcard.to_string()],
        allow_atomic_publish: true,
        ..Default::default()
    })
    .await
    .expect("create test events stream")
}

async fn publish(js: &jetstream::Context, subject: &str, payload: &str) {
    js.publish(subject.to_string(), Bytes::from(payload.to_string()))
        .await
        .expect("send publish")
        .await
        .expect("ack publish");
}

fn consumer_config(durable_name: &str) -> pull::Config {
    pull::Config {
        durable_name: Some(durable_name.to_string()),
        ack_policy: AckPolicy::Explicit,
        ack_wait: Duration::from_secs(10),
        ..Default::default()
    }
}

async fn wait_until(timeout: Duration, mut condition: impl FnMut() -> bool) {
    let deadline = tokio::time::Instant::now() + timeout;
    loop {
        if condition() {
            return;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "condition not met within timeout"
        );
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}

#[derive(Debug, thiserror::Error)]
#[error("scripted handler failure")]
struct TestHandlerError;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PoisonRecord {
    Verdict,
    RedeliveryExhausted,
    HandlerError,
    Panic,
}

#[derive(Clone)]
struct RecordingHandler {
    verdicts: Arc<Vec<HandlerVerdict>>,
    calls: Arc<AtomicUsize>,
    poisoned: Arc<Mutex<Vec<PoisonRecord>>>,
}

impl RecordingHandler {
    fn new(verdicts: Vec<HandlerVerdict>) -> Self {
        assert!(!verdicts.is_empty(), "scripted handler needs at least one verdict");
        Self {
            verdicts: Arc::new(verdicts),
            calls: Arc::new(AtomicUsize::new(0)),
            poisoned: Arc::new(Mutex::new(Vec::new())),
        }
    }

    fn call_count(&self) -> usize {
        self.calls.load(Ordering::SeqCst)
    }

    fn poisoned_records(&self) -> Vec<PoisonRecord> {
        self.poisoned.lock().expect("poisoned records lock").clone()
    }
}

impl MessageHandler for RecordingHandler {
    type Error = TestHandlerError;

    async fn handle(&mut self, _message: &jetstream::Message) -> Result<HandlerVerdict, Self::Error> {
        let index = self.calls.fetch_add(1, Ordering::SeqCst);
        let verdict = self
            .verdicts
            .get(index)
            .copied()
            .unwrap_or_else(|| *self.verdicts.last().expect("at least one scripted verdict"));
        Ok(verdict)
    }

    async fn on_poison(&mut self, reason: PoisonReason<Self::Error>) {
        let record = match reason {
            PoisonReason::Verdict => PoisonRecord::Verdict,
            PoisonReason::RedeliveryExhausted => PoisonRecord::RedeliveryExhausted,
            PoisonReason::HandlerError(_) => PoisonRecord::HandlerError,
            PoisonReason::Panic => PoisonRecord::Panic,
        };
        self.poisoned.lock().expect("poisoned records lock").push(record);
    }
}

#[tokio::test]
async fn processor_acks_message_and_never_redelivers_it() {
    let server = JetStreamTestServer::start().await;
    let js = server.jetstream().await;
    let stream = create_events_stream(&js, "PROCESSOR_ACK", "processor.ack.>").await;
    publish(&js, "processor.ack.one", "payload").await;

    let handler = RecordingHandler::new(vec![HandlerVerdict::Ack]);
    let processor = Processor::new(stream, consumer_config("processor-ack"), handler.clone()).with_redelivery_policy(
        RedeliveryPolicy::new(5, Duration::from_millis(20), Duration::from_millis(100)),
    );
    let shutdown = CancellationToken::new();
    let run_shutdown = shutdown.clone();
    let run_handle = tokio::spawn(async move { processor.run(run_shutdown).await });

    wait_until(Duration::from_secs(5), || handler.call_count() >= 1).await;
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert_eq!(handler.call_count(), 1, "an acked message must not be redelivered");
    assert!(handler.poisoned_records().is_empty());

    shutdown.cancel();
    run_handle
        .await
        .expect("processor task should not panic")
        .expect("processor should shut down cleanly");
}

#[tokio::test]
async fn processor_naks_with_backoff_and_redelivers_until_ack() {
    let server = JetStreamTestServer::start().await;
    let js = server.jetstream().await;
    let stream = create_events_stream(&js, "PROCESSOR_RETRY", "processor.retry.>").await;
    publish(&js, "processor.retry.one", "payload").await;

    let handler = RecordingHandler::new(vec![HandlerVerdict::Retry, HandlerVerdict::Retry, HandlerVerdict::Ack]);
    let processor = Processor::new(stream, consumer_config("processor-retry"), handler.clone()).with_redelivery_policy(
        RedeliveryPolicy::new(5, Duration::from_millis(30), Duration::from_millis(100)),
    );
    let shutdown = CancellationToken::new();
    let run_shutdown = shutdown.clone();
    let run_handle = tokio::spawn(async move { processor.run(run_shutdown).await });

    wait_until(Duration::from_secs(5), || handler.call_count() >= 3).await;
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert_eq!(
        handler.call_count(),
        3,
        "the message should stop being redelivered once acked"
    );
    assert!(handler.poisoned_records().is_empty());

    shutdown.cancel();
    run_handle
        .await
        .expect("processor task should not panic")
        .expect("processor should shut down cleanly");
}

#[tokio::test]
async fn processor_poisons_message_on_explicit_poison_verdict() {
    let server = JetStreamTestServer::start().await;
    let js = server.jetstream().await;
    let stream = create_events_stream(&js, "PROCESSOR_POISON", "processor.poison.>").await;
    publish(&js, "processor.poison.one", "payload").await;

    let handler = RecordingHandler::new(vec![HandlerVerdict::Poison]);
    let processor =
        Processor::new(stream, consumer_config("processor-poison"), handler.clone()).with_redelivery_policy(
            RedeliveryPolicy::new(5, Duration::from_millis(30), Duration::from_millis(100)),
        );
    let shutdown = CancellationToken::new();
    let run_shutdown = shutdown.clone();
    let run_handle = tokio::spawn(async move { processor.run(run_shutdown).await });

    wait_until(Duration::from_secs(5), || !handler.poisoned_records().is_empty()).await;
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert_eq!(
        handler.call_count(),
        1,
        "an explicitly poisoned message must not be redelivered"
    );
    assert_eq!(handler.poisoned_records(), vec![PoisonRecord::Verdict]);

    shutdown.cancel();
    run_handle
        .await
        .expect("processor task should not panic")
        .expect("processor should shut down cleanly");
}

#[tokio::test]
async fn processor_poisons_message_after_exhausting_redeliveries() {
    let server = JetStreamTestServer::start().await;
    let js = server.jetstream().await;
    let stream = create_events_stream(&js, "PROCESSOR_EXHAUST", "processor.exhaust.>").await;
    publish(&js, "processor.exhaust.one", "payload").await;

    let handler = RecordingHandler::new(vec![HandlerVerdict::Retry]);
    let processor =
        Processor::new(stream, consumer_config("processor-exhaust"), handler.clone()).with_redelivery_policy(
            RedeliveryPolicy::new(3, Duration::from_millis(20), Duration::from_millis(50)),
        );
    let shutdown = CancellationToken::new();
    let run_shutdown = shutdown.clone();
    let run_handle = tokio::spawn(async move { processor.run(run_shutdown).await });

    wait_until(Duration::from_secs(5), || !handler.poisoned_records().is_empty()).await;
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert_eq!(handler.poisoned_records(), vec![PoisonRecord::RedeliveryExhausted]);
    assert!(
        handler.call_count() >= 1 && handler.call_count() <= 3,
        "delivery count should be bounded by max_deliveries, got {}",
        handler.call_count()
    );

    shutdown.cancel();
    run_handle
        .await
        .expect("processor task should not panic")
        .expect("processor should shut down cleanly");
}

#[tokio::test]
async fn processor_poisons_message_on_handler_error_after_exhausting_redeliveries() {
    struct FailingHandler {
        calls: Arc<AtomicUsize>,
        poisoned: Arc<Mutex<Vec<PoisonRecord>>>,
    }

    impl MessageHandler for FailingHandler {
        type Error = TestHandlerError;

        async fn handle(&mut self, _message: &jetstream::Message) -> Result<HandlerVerdict, Self::Error> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            Err(TestHandlerError)
        }

        async fn on_poison(&mut self, reason: PoisonReason<Self::Error>) {
            let record = match reason {
                PoisonReason::HandlerError(_) => PoisonRecord::HandlerError,
                PoisonReason::Verdict => PoisonRecord::Verdict,
                PoisonReason::RedeliveryExhausted => PoisonRecord::RedeliveryExhausted,
                PoisonReason::Panic => PoisonRecord::Panic,
            };
            self.poisoned.lock().expect("poisoned records lock").push(record);
        }
    }

    let server = JetStreamTestServer::start().await;
    let js = server.jetstream().await;
    let stream = create_events_stream(&js, "PROCESSOR_HANDLER_ERROR", "processor.handler_error.>").await;
    publish(&js, "processor.handler_error.one", "payload").await;

    let calls = Arc::new(AtomicUsize::new(0));
    let poisoned = Arc::new(Mutex::new(Vec::new()));
    let handler = FailingHandler {
        calls: calls.clone(),
        poisoned: poisoned.clone(),
    };
    let processor = Processor::new(stream, consumer_config("processor-handler-error"), handler).with_redelivery_policy(
        RedeliveryPolicy::new(2, Duration::from_millis(20), Duration::from_millis(50)),
    );
    let shutdown = CancellationToken::new();
    let run_shutdown = shutdown.clone();
    let run_handle = tokio::spawn(async move { processor.run(run_shutdown).await });

    wait_until(Duration::from_secs(5), || {
        !poisoned.lock().expect("poisoned records lock").is_empty()
    })
    .await;
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert_eq!(
        poisoned.lock().expect("poisoned records lock").clone(),
        vec![PoisonRecord::HandlerError]
    );

    shutdown.cancel();
    run_handle
        .await
        .expect("processor task should not panic")
        .expect("processor should shut down cleanly");
}

#[tokio::test]
async fn processor_stops_cleanly_on_shutdown() {
    let server = JetStreamTestServer::start().await;
    let js = server.jetstream().await;
    let stream = create_events_stream(&js, "PROCESSOR_SHUTDOWN", "processor.shutdown.>").await;

    let handler = RecordingHandler::new(vec![HandlerVerdict::Ack]);
    let processor = Processor::new(stream, consumer_config("processor-shutdown"), handler);
    let shutdown = CancellationToken::new();
    let run_shutdown = shutdown.clone();
    let run_handle = tokio::spawn(async move { processor.run(run_shutdown).await });

    tokio::time::sleep(Duration::from_millis(100)).await;
    shutdown.cancel();

    let result = tokio::time::timeout(Duration::from_secs(5), run_handle)
        .await
        .expect("processor should shut down promptly after cancellation")
        .expect("processor task should not panic");
    assert!(result.is_ok());
}

#[tokio::test]
async fn processor_rejects_non_explicit_ack_policy() {
    let server = JetStreamTestServer::start().await;
    let js = server.jetstream().await;
    let stream = create_events_stream(&js, "PROCESSOR_ACK_POLICY", "processor.ack_policy.>").await;

    let mut config = consumer_config("processor-ack-policy");
    config.ack_policy = AckPolicy::None;
    let handler = RecordingHandler::new(vec![HandlerVerdict::Ack]);
    let processor = Processor::new(stream, config, handler);

    let error = processor
        .run(CancellationToken::new())
        .await
        .expect_err("non-explicit ack policy must be rejected");

    assert!(matches!(error, super::ProcessorError::InvalidAckPolicy { .. }));
}
