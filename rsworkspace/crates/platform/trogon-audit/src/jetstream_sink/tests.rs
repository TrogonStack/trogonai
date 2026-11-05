use std::collections::VecDeque;
use std::future::Future;
use std::pin::Pin;
use std::sync::{Arc, Mutex};

use async_nats::HeaderMap;
use async_nats::header::NATS_MESSAGE_ID;
use async_nats::jetstream::context::{PublishError, PublishErrorKind};
use async_nats::jetstream::publish::PublishAck;
use async_nats::subject::ToSubject;
use bytes::Bytes;
use trogon_nats::jetstream::{JetStreamPublisher, MockJetStreamPublisher};
use trogon_nats::mocks::MockError;
use trogon_std::CapturedEvents;
use trogon_std::log_capture::CapturedLogs;
use trogon_std::log_capture::LevelFilter;

use super::*;
use crate::constants::{HEADER_SEQUENCE, HEADER_WRITER};
use crate::fixtures::{allow, stamper, writer_id};

fn sealed() -> SealedRecord {
    SealedRecord::seal(allow(&stamper()), &writer_id(), 4)
}

fn attempts(count: u32) -> NonZeroU32 {
    NonZeroU32::new(count).unwrap()
}

#[derive(Debug, Clone, Copy)]
enum Reply {
    Ack,
    NotAccepted,
    AckStreamFull,
    AckHangs,
}

type AckFuture = Pin<Box<dyn Future<Output = Result<PublishAck, PublishError>> + Send>>;

/// A publisher answering from a script with the real client error types, so the
/// sink's classification is exercised against what the client returns.
#[derive(Debug, Clone, Default)]
struct ScriptedPublisher {
    script: Arc<Mutex<VecDeque<Reply>>>,
    message_ids: Arc<Mutex<Vec<String>>>,
}

impl ScriptedPublisher {
    fn replying(replies: impl IntoIterator<Item = Reply>) -> Self {
        Self {
            script: Arc::new(Mutex::new(replies.into_iter().collect())),
            message_ids: Arc::default(),
        }
    }

    fn message_ids(&self) -> Vec<String> {
        self.message_ids.lock().unwrap().clone()
    }
}

fn ack() -> PublishAck {
    PublishAck {
        stream: "AUDIT".to_owned(),
        sequence: 1,
        domain: String::new(),
        duplicate: false,
        value: None,
    }
}

impl JetStreamPublisher for ScriptedPublisher {
    type PublishError = PublishError;
    type AckFuture = AckFuture;

    async fn publish_with_headers<S: ToSubject + Send>(
        &self,
        _subject: S,
        headers: HeaderMap,
        _payload: Bytes,
    ) -> Result<Self::AckFuture, PublishError> {
        let id = headers.get(NATS_MESSAGE_ID).unwrap().as_str().to_owned();
        self.message_ids.lock().unwrap().push(id);
        let reply = self.script.lock().unwrap().pop_front().unwrap_or(Reply::Ack);
        match reply {
            Reply::Ack => Ok(Box::pin(async { Ok(ack()) })),
            Reply::NotAccepted => Err(PublishError::new(PublishErrorKind::BrokenPipe)),
            Reply::AckStreamFull => Ok(Box::pin(async {
                Err(PublishError::with_source(
                    PublishErrorKind::Other,
                    crate::fixtures::server_error(503, 10077),
                ))
            })),
            Reply::AckHangs => Ok(Box::pin(std::future::pending())),
        }
    }
}

#[tokio::test]
async fn publishes_the_encoded_record_under_its_record_id() {
    let publisher = MockJetStreamPublisher::new();
    let sink = JetStreamSink::with_defaults(publisher.clone());
    let record = sealed();

    sink.publish(&record).await.unwrap();

    let messages = publisher.published_messages();
    assert_eq!(messages.len(), 1);
    assert_eq!(messages[0].subject, "platform.v1.audit.decision.project_1.allow");
    assert_eq!(messages[0].payload, *record.payload());
    assert_eq!(
        messages[0].headers.get(NATS_MESSAGE_ID).unwrap().as_str(),
        record.id().to_string()
    );
    assert_eq!(messages[0].headers.get(HEADER_WRITER).unwrap().as_str(), "gateway-1");
    assert_eq!(messages[0].headers.get(HEADER_SEQUENCE).unwrap().as_str(), "4");
}

#[tokio::test]
async fn an_unreachable_publisher_fails_at_once_without_retrying() {
    let publisher = MockJetStreamPublisher::new();
    publisher.fail_js_publish_count(10);
    let sink = JetStreamSink::new(publisher.clone(), DEFAULT_ACK_TIMEOUT, attempts(3));

    assert_eq!(sink.publish(&sealed()).await, Err(SinkError::Unreachable));
    assert!(publisher.published_messages().is_empty());
}

#[tokio::test]
async fn a_publisher_that_does_not_accept_the_message_is_unreachable() {
    let publisher = ScriptedPublisher::replying([Reply::NotAccepted]);
    let sink = JetStreamSink::new(publisher.clone(), DEFAULT_ACK_TIMEOUT, attempts(3));
    assert_eq!(sink.publish(&sealed()).await, Err(SinkError::Unreachable));
    assert_eq!(publisher.message_ids().len(), 1);
}

#[tokio::test]
async fn a_full_stream_is_reported_as_full_and_not_retried() {
    let publisher = ScriptedPublisher::replying([Reply::AckStreamFull, Reply::Ack]);
    let sink = JetStreamSink::new(publisher.clone(), DEFAULT_ACK_TIMEOUT, attempts(3));
    assert_eq!(sink.publish(&sealed()).await, Err(SinkError::StreamFull));
    assert_eq!(publisher.message_ids().len(), 1);
}

#[tokio::test(start_paused = true)]
async fn a_timeout_is_retried_under_the_same_message_id() {
    let publisher = ScriptedPublisher::replying([Reply::AckHangs, Reply::AckHangs, Reply::Ack]);
    let sink = JetStreamSink::new(publisher.clone(), DEFAULT_ACK_TIMEOUT, attempts(3));
    let record = sealed();

    sink.publish(&record).await.unwrap();

    let ids = publisher.message_ids();
    assert_eq!(ids.len(), 3);
    assert!(ids.iter().all(|id| *id == record.id().to_string()));
}

#[tokio::test(start_paused = true)]
async fn retries_are_bounded() {
    let publisher = ScriptedPublisher::replying([Reply::AckHangs, Reply::AckHangs, Reply::AckHangs, Reply::Ack]);
    let sink = JetStreamSink::new(publisher.clone(), DEFAULT_ACK_TIMEOUT, attempts(3));

    assert_eq!(sink.publish(&sealed()).await, Err(SinkError::Timeout));
    assert_eq!(publisher.message_ids().len(), 3);
}

#[tokio::test(start_paused = true)]
async fn a_single_attempt_does_not_retry() {
    let publisher = ScriptedPublisher::replying([Reply::AckHangs, Reply::Ack]);
    let sink = JetStreamSink::new(publisher.clone(), DEFAULT_ACK_TIMEOUT, attempts(1));
    assert_eq!(sink.publish(&sealed()).await, Err(SinkError::Timeout));
    assert_eq!(publisher.message_ids().len(), 1);
}

#[test]
fn a_failed_claim_check_store_is_unreachable() {
    let outcome = PublishOutcome::<MockError>::StoreFailed(Box::new(MockError("store".to_owned())));
    assert_eq!(failure_of(outcome), Some(SinkError::Unreachable));
}

#[test]
fn a_published_outcome_is_not_a_failure() {
    assert_eq!(failure_of(PublishOutcome::<MockError>::Published), None);
}

#[test]
fn every_failed_outcome_is_classified() {
    let ack_failed = PublishOutcome::AckFailed(MockError("ack".to_owned()));
    assert_eq!(failure_of(ack_failed), Some(SinkError::Unreachable));
    let timed_out = PublishOutcome::<MockError>::AckTimedOut(DEFAULT_ACK_TIMEOUT);
    assert_eq!(failure_of(timed_out), Some(SinkError::Timeout));
}

#[tokio::test(start_paused = true)]
async fn each_unacknowledged_attempt_is_logged_with_its_number() {
    let captured = CapturedEvents::new();
    let _guard = captured.install(LevelFilter::TRACE);
    let publisher = ScriptedPublisher::replying([Reply::AckHangs, Reply::AckHangs]);
    let sink = JetStreamSink::new(publisher, DEFAULT_ACK_TIMEOUT, attempts(2));

    assert_eq!(sink.publish(&sealed()).await, Err(SinkError::Timeout));

    let events = captured.events();
    let attempts: Vec<_> = events.iter().filter_map(|event| event.field("attempt")).collect();
    assert_eq!(attempts, vec!["1", "2"]);
    assert!(events.iter().all(|event| event.field("reason") == Some("timeout")));
}

#[tokio::test(start_paused = true)]
async fn an_unacknowledged_attempt_reaches_the_log_facade_when_no_subscriber_is_installed() {
    let Some(logs) = CapturedLogs::isolated() else {
        return;
    };
    let publisher = ScriptedPublisher::replying([Reply::AckHangs]);
    let sink = JetStreamSink::new(publisher, DEFAULT_ACK_TIMEOUT, attempts(1));
    let record = sealed();

    assert_eq!(sink.publish(&record).await, Err(SinkError::Timeout));

    let records = logs.records();
    let line = records
        .iter()
        .find(|entry| entry.message.contains("audit publish was not acknowledged"))
        .expect("the attempt is logged");
    assert!(line.message.contains(&record.id().to_string()));
    assert!(line.message.contains("timeout"));
}
