//! Concrete execution-schedule operations against JetStream.
//!
//! NATS does not replace a scheduled message by subject, and `Nats-Msg-Id`
//! deduplication is only bounded by the stream's duplicate window. So "upsert"
//! is defined as **purge-then-publish** on the deterministic execution subject:
//! purge any existing scheduled message for the key, then publish exactly one.
//! This makes "exactly one execution schedule message exists for this key" the
//! post-condition independent of the duplicate window, with `Nats-Msg-Id` as a
//! cheap extra guard inside the window.
//!
//! Pause and remove are purge-only on the same deterministic subject, treating
//! purge of an already-absent subject as success.

use async_nats::HeaderMap;
use bytes::Bytes;
use std::future::IntoFuture;
use trogon_nats::jetstream::{JetStreamPublisher, JetStreamSubjectPurger, PurgeOutcome};

use crate::processor::execution::reconciliation::{ScheduleRequest, ScheduleSubject};

const NATS_MSG_ID_HEADER: &str = "Nats-Msg-Id";

/// Error raised while writing an execution-schedule operation. Every variant is
/// a transient NATS failure: the record must not be acknowledged and should be
/// retried.
#[derive(Debug, thiserror::Error)]
pub enum ExecutionScheduleWriteError {
    /// Purging the execution subject failed.
    #[error("execution subject purge failed: {source}")]
    Purge {
        #[source]
        source: Box<dyn std::error::Error + Send + Sync>,
    },
    /// Publishing the execution schedule message failed.
    #[error("execution schedule publish failed: {source}")]
    Publish {
        #[source]
        source: Box<dyn std::error::Error + Send + Sync>,
    },
    /// The publish was sent but its acknowledgement failed.
    #[error("execution schedule publish ack failed: {source}")]
    Ack {
        #[source]
        source: Box<dyn std::error::Error + Send + Sync>,
    },
}

impl ExecutionScheduleWriteError {
    /// Every execution schedule write failure is transient and safe to retry.
    pub fn is_transient(&self) -> bool {
        true
    }
}

/// Writes execution-schedule side effects against a concrete JetStream
/// publisher and purger.
#[derive(Debug, Clone)]
pub struct ExecutionScheduleWriter<P, U> {
    publisher: P,
    purger: U,
}

impl<P, U> ExecutionScheduleWriter<P, U>
where
    P: JetStreamPublisher,
    U: JetStreamSubjectPurger,
{
    /// Wraps a concrete publisher and purger.
    pub fn new(publisher: P, purger: U) -> Self {
        Self { publisher, purger }
    }

    /// Purge-then-publish upsert for an enabled execution schedule.
    ///
    /// `msg_id` becomes the `Nats-Msg-Id` of the published message and
    /// `trace_headers` carries the propagated trace context; scheduler-owned
    /// headers from the request always win over both.
    pub async fn upsert(
        &self,
        request: &ScheduleRequest,
        msg_id: &str,
        trace_headers: &HeaderMap,
    ) -> Result<(), ExecutionScheduleWriteError> {
        self.purge(request.subject()).await?;

        let headers = build_headers(request, msg_id, trace_headers);
        let payload = Bytes::copy_from_slice(request.payload());

        let ack = self
            .publisher
            .publish_with_headers(request.subject().as_str().to_string(), headers, payload)
            .await
            .map_err(|source| ExecutionScheduleWriteError::Publish {
                source: Box::new(source),
            })?;

        ack.into_future()
            .await
            .map_err(|source| ExecutionScheduleWriteError::Ack {
                source: Box::new(source),
            })?;

        Ok(())
    }

    /// Purge-only operation for pause/remove. Purging an already-absent subject
    /// is a success.
    pub async fn purge(&self, subject: &ScheduleSubject) -> Result<(), ExecutionScheduleWriteError> {
        let response = self
            .purger
            .purge_subject_messages(subject.as_str())
            .await
            .map_err(|source| ExecutionScheduleWriteError::Purge {
                source: Box::new(source),
            })?;

        if !response.is_success() {
            return Err(ExecutionScheduleWriteError::Purge {
                source: Box::new(PurgeUnacknowledged),
            });
        }

        Ok(())
    }
}

/// The server answered the purge without a transport error but did not
/// acknowledge it as successful.
#[derive(Debug, thiserror::Error)]
#[error("purge was not acknowledged as successful by the server")]
struct PurgeUnacknowledged;

fn build_headers(request: &ScheduleRequest, msg_id: &str, trace_headers: &HeaderMap) -> HeaderMap {
    let mut headers = trace_headers.clone();
    for header in request.headers() {
        headers.insert(header.name().as_str(), header.value().as_str());
    }
    headers.insert(NATS_MSG_ID_HEADER, msg_id);
    headers
}

#[cfg(test)]
mod tests {
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
}
