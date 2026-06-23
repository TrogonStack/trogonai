use std::collections::HashMap;
use std::future::{Ready, ready};
use std::sync::{Arc, Mutex};

use async_nats::jetstream::publish::PublishAck;
use async_nats::subject::ToSubject;
use trogon_nats::jetstream::{MockJetStreamPublisher, MockJetStreamPurger};

use super::*;
use crate::commands::domain::{Delivery, MessageContent, Schedule, ScheduleHeaders, ScheduleId, ScheduleMessage};

fn request() -> ScheduleRequest {
    ScheduleRequest::build(
        &ScheduleId::parse("orders/created").unwrap(),
        &Schedule::every(std::time::Duration::from_secs(30)).unwrap(),
        &Delivery::nats_event("agent.run").unwrap(),
        &ScheduleMessage {
            content: MessageContent::json(r#"{"ok":true}"#),
            headers: ScheduleHeaders::new([("x-kind", "heartbeat")]).unwrap(),
        },
    )
    .unwrap()
}

fn dispatch_request() -> DispatchRequest {
    DispatchRequest::build(
        &ScheduleId::parse("orders/created").unwrap(),
        &Delivery::nats_event("agent.run").unwrap(),
        &ScheduleMessage {
            content: MessageContent::json(r#"{"ok":true}"#),
            headers: ScheduleHeaders::new([("x-kind", "heartbeat")]).unwrap(),
        },
    )
    .unwrap()
}

fn trace_headers() -> HeaderMap {
    let mut headers = HeaderMap::new();
    headers.insert("traceparent", "00-trace-span-01");
    headers
}

#[tokio::test]
async fn upsert_purges_then_publishes_with_msg_id_scheduler_and_trace_headers() {
    let publisher = MockJetStreamPublisher::new();
    let purger = MockJetStreamPurger::new();
    let writer = ExecutionScheduleWriter::new(publisher.clone(), purger.clone());
    let request = request();

    writer.upsert(&request, "event-7", &trace_headers()).await.unwrap();

    assert_eq!(purger.purged_subjects(), vec![request.subject().as_str().to_string()]);

    let published = publisher.published_messages();
    assert_eq!(published.len(), 1);
    let message = &published[0];
    assert_eq!(message.subject, request.subject().as_str());
    assert_eq!(message.headers.get("Nats-Msg-Id").unwrap().as_str(), "event-7");
    assert_eq!(message.headers.get("Nats-Schedule").unwrap().as_str(), "@every 30s");
    assert_eq!(message.headers.get("traceparent").unwrap().as_str(), "00-trace-span-01");
    assert_eq!(message.headers.get("x-kind").unwrap().as_str(), "heartbeat");
    assert_eq!(message.payload, Bytes::from_static(br#"{"ok":true}"#));
}

#[tokio::test]
async fn dispatch_publishes_user_payload_with_msg_id_and_trace_headers() {
    let publisher = MockJetStreamPublisher::new();
    let purger = MockJetStreamPurger::new();
    let writer = ExecutionScheduleWriter::new(publisher.clone(), purger.clone());
    let request = dispatch_request();

    writer
        .dispatch(&request, "event-7:dispatch", &trace_headers())
        .await
        .unwrap();

    assert!(purger.purged_subjects().is_empty());
    let published = publisher.published_messages();
    assert_eq!(published.len(), 1);
    let message = &published[0];
    assert_eq!(message.subject, request.subject().as_str());
    assert_eq!(message.headers.get("Nats-Msg-Id").unwrap().as_str(), "event-7:dispatch");
    assert_eq!(
        message.headers.get("Content-Type").unwrap().as_str(),
        "application/json"
    );
    assert_eq!(message.headers.get("traceparent").unwrap().as_str(), "00-trace-span-01");
    assert_eq!(message.headers.get("x-kind").unwrap().as_str(), "heartbeat");
    assert_eq!(message.payload, Bytes::from_static(br#"{"ok":true}"#));
}

#[tokio::test]
async fn purge_only_targets_the_subject_and_succeeds_when_absent() {
    let publisher = MockJetStreamPublisher::new();
    let purger = MockJetStreamPurger::new();
    let writer = ExecutionScheduleWriter::new(publisher.clone(), purger.clone());
    let subject = request().subject().clone();

    writer.purge(&subject).await.unwrap();

    assert_eq!(purger.purged_subjects(), vec![subject.as_str().to_string()]);
    assert!(publisher.published_messages().is_empty());
}

#[tokio::test]
async fn publish_failure_is_transient() {
    let publisher = MockJetStreamPublisher::new();
    publisher.fail_next_js_publish();
    let purger = MockJetStreamPurger::new();
    let writer = ExecutionScheduleWriter::new(publisher, purger);

    let error = writer
        .upsert(&request(), "event-7", &HeaderMap::new())
        .await
        .unwrap_err();
    assert!(error.is_transient());
    assert!(matches!(error, ExecutionScheduleWriteError::Publish { .. }));
}

#[tokio::test]
async fn dispatch_publish_failure_is_transient() {
    let publisher = MockJetStreamPublisher::new();
    publisher.fail_next_js_publish();
    let purger = MockJetStreamPurger::new();
    let writer = ExecutionScheduleWriter::new(publisher, purger);

    let error = writer
        .dispatch(&dispatch_request(), "event-7:dispatch", &HeaderMap::new())
        .await
        .unwrap_err();
    assert!(error.is_transient());
    assert!(matches!(error, ExecutionScheduleWriteError::Publish { .. }));
}

#[tokio::test]
async fn purge_failure_is_transient() {
    let publisher = MockJetStreamPublisher::new();
    let purger = MockJetStreamPurger::new();
    purger.fail_next_purge();
    let writer = ExecutionScheduleWriter::new(publisher, purger);

    let error = writer.purge(&request().subject().clone()).await.unwrap_err();
    assert!(matches!(error, ExecutionScheduleWriteError::Purge { .. }));
}

#[tokio::test]
async fn purge_not_acknowledged_as_successful_is_an_error() {
    #[derive(Debug, Clone)]
    struct UnacknowledgedPurger;

    impl JetStreamSubjectPurger for UnacknowledgedPurger {
        type PurgeResponse = async_nats::jetstream::stream::PurgeResponse;
        type Error = std::convert::Infallible;

        async fn purge_subject_messages(&self, _subject: &str) -> Result<Self::PurgeResponse, Self::Error> {
            Ok(async_nats::jetstream::stream::PurgeResponse {
                success: false,
                purged: 0,
            })
        }
    }

    let publisher = MockJetStreamPublisher::new();
    let writer = ExecutionScheduleWriter::new(publisher.clone(), UnacknowledgedPurger);

    let error = writer.purge(&request().subject().clone()).await.unwrap_err();
    assert!(error.is_transient());
    assert!(matches!(error, ExecutionScheduleWriteError::Purge { .. }));

    let error = writer
        .upsert(&request(), "event-7", &HeaderMap::new())
        .await
        .unwrap_err();
    assert!(matches!(error, ExecutionScheduleWriteError::Purge { .. }));
    assert!(publisher.published_messages().is_empty());
}

#[tokio::test]
async fn ack_failure_is_transient_and_exposes_source() {
    #[derive(Debug, Clone)]
    struct AckFailingPublisher;

    #[derive(Debug, Clone, thiserror::Error)]
    #[error("ack failed")]
    struct AckError;

    impl JetStreamPublisher for AckFailingPublisher {
        type PublishError = AckError;
        type AckFuture = Ready<Result<PublishAck, AckError>>;

        async fn publish_with_headers<S: ToSubject + Send>(
            &self,
            _subject: S,
            _headers: HeaderMap,
            _payload: Bytes,
        ) -> Result<Self::AckFuture, Self::PublishError> {
            Ok(ready(Err(AckError)))
        }
    }

    let purger = MockJetStreamPurger::new();
    let writer = ExecutionScheduleWriter::new(AckFailingPublisher, purger);

    let error = writer
        .upsert(&request(), "event-7", &HeaderMap::new())
        .await
        .unwrap_err();

    assert!(error.is_transient());
    assert!(matches!(error, ExecutionScheduleWriteError::Ack { .. }));
    assert_eq!(error.to_string(), "execution schedule publish ack failed: ack failed");
    assert!(std::error::Error::source(&error).is_some());

    let writer = ExecutionScheduleWriter::new(AckFailingPublisher, MockJetStreamPurger::new());
    let error = writer
        .dispatch(&dispatch_request(), "event-7:dispatch", &HeaderMap::new())
        .await
        .unwrap_err();
    assert!(error.is_transient());
    assert!(matches!(error, ExecutionScheduleWriteError::Ack { .. }));
}

/// A single test double that backs both publish and purge with shared
/// per-subject state, so we can prove purge-then-publish converges to
/// exactly one scheduled message even when the same writer repeats far past
/// any `Nats-Msg-Id` duplicate window.
#[derive(Clone, Default)]
struct SharedExecutionStream {
    messages: Arc<Mutex<HashMap<String, Vec<Bytes>>>>,
}

impl SharedExecutionStream {
    fn count(&self, subject: &str) -> usize {
        self.messages.lock().unwrap().get(subject).map_or(0, Vec::len)
    }
}

impl JetStreamPublisher for SharedExecutionStream {
    type PublishError = std::convert::Infallible;
    type AckFuture = std::future::Ready<Result<PublishAck, std::convert::Infallible>>;

    async fn publish_with_headers<S: ToSubject + Send>(
        &self,
        subject: S,
        _headers: HeaderMap,
        payload: Bytes,
    ) -> Result<Self::AckFuture, Self::PublishError> {
        let subject = subject.to_subject().to_string();
        self.messages.lock().unwrap().entry(subject).or_default().push(payload);
        Ok(std::future::ready(Ok(PublishAck {
            stream: "execution".to_string(),
            sequence: 1,
            domain: String::new(),
            duplicate: false,
            value: None,
        })))
    }
}

impl JetStreamSubjectPurger for SharedExecutionStream {
    type PurgeResponse = ();
    type Error = std::convert::Infallible;

    async fn purge_subject_messages(&self, subject: &str) -> Result<Self::PurgeResponse, Self::Error> {
        self.messages.lock().unwrap().remove(subject);
        Ok(())
    }
}

#[tokio::test]
async fn repeated_upsert_converges_to_exactly_one_scheduled_message() {
    let stream = SharedExecutionStream::default();
    let writer = ExecutionScheduleWriter::new(stream.clone(), stream.clone());
    let request = request();
    let subject = request.subject().as_str().to_string();

    // Write the same schedule many times, well past any duplicate window.
    for attempt in 0..5 {
        writer
            .upsert(&request, &format!("event-{attempt}"), &HeaderMap::new())
            .await
            .unwrap();
    }

    assert_eq!(stream.count(&subject), 1);
}
