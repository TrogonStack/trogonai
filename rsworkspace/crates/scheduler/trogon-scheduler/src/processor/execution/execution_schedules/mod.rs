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

use crate::processor::execution::reconciliation::{DispatchRequest, ScheduleRequest, ScheduleSubject};

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

    pub async fn dispatch(
        &self,
        request: &DispatchRequest,
        msg_id: &str,
        trace_headers: &HeaderMap,
    ) -> Result<(), ExecutionScheduleWriteError> {
        let headers = build_dispatch_headers(request, msg_id, trace_headers);
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

fn build_dispatch_headers(request: &DispatchRequest, msg_id: &str, trace_headers: &HeaderMap) -> HeaderMap {
    let mut headers = trace_headers.clone();
    for header in request.headers() {
        headers.insert(header.name().as_str(), header.value().as_str());
    }
    headers.insert(NATS_MSG_ID_HEADER, msg_id);
    headers
}

#[cfg(test)]
mod tests;
