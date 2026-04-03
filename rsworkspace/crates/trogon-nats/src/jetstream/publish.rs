use std::fmt;
use std::time::Duration;

use bytes::Bytes;
use tracing::error;

use crate::jetstream::JetStreamPublisher;

#[derive(Debug)]
pub enum PublishOutcome<E: fmt::Display> {
    Published,
    PublishFailed(E),
    AckFailed(E),
    AckTimedOut(Duration),
}

impl<E: fmt::Display> PublishOutcome<E> {
    pub fn is_ok(&self) -> bool {
        matches!(self, PublishOutcome::Published)
    }

    pub fn log_on_error(&self, source_name: &str) {
        match self {
            PublishOutcome::Published => {}
            PublishOutcome::PublishFailed(e) => {
                error!(error = %e, source = source_name, "Failed to publish event to NATS");
            }
            PublishOutcome::AckFailed(e) => {
                error!(error = %e, source = source_name, "NATS ack failed");
            }
            PublishOutcome::AckTimedOut(timeout) => {
                error!(?timeout, source = source_name, "NATS ack timed out");
            }
        }
    }
}

pub async fn publish_event<P: JetStreamPublisher>(
    js: &P,
    subject: String,
    headers: async_nats::HeaderMap,
    body: Bytes,
    ack_timeout: Duration,
) -> PublishOutcome<P::PublishError> {
    let ack_future = match js.publish_with_headers(subject, headers, body).await {
        Ok(f) => f,
        Err(e) => return PublishOutcome::PublishFailed(e),
    };

    match tokio::time::timeout(ack_timeout, ack_future).await {
        Ok(Ok(_)) => PublishOutcome::Published,
        Ok(Err(e)) => PublishOutcome::AckFailed(e),
        Err(_) => PublishOutcome::AckTimedOut(ack_timeout),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(feature = "test-support")]
    mod with_mocks {
        use super::*;
        use crate::jetstream::MockJetStreamPublisher;

        #[tokio::test]
        async fn publish_event_returns_published_on_success() {
            let publisher = MockJetStreamPublisher::new();
            let result = publish_event(
                &publisher,
                "test.subject".to_string(),
                async_nats::HeaderMap::new(),
                Bytes::from_static(b"payload"),
                Duration::from_secs(10),
            )
            .await;
            assert!(result.is_ok());
        }

        #[tokio::test]
        async fn publish_event_returns_publish_failed_on_error() {
            let publisher = MockJetStreamPublisher::new();
            publisher.fail_next_js_publish();
            let result = publish_event(
                &publisher,
                "test.subject".to_string(),
                async_nats::HeaderMap::new(),
                Bytes::from_static(b"payload"),
                Duration::from_secs(10),
            )
            .await;
            assert!(matches!(result, PublishOutcome::PublishFailed(_)));
        }
    }

    #[test]
    fn log_on_error_does_nothing_for_published() {
        let outcome: PublishOutcome<String> = PublishOutcome::Published;
        outcome.log_on_error("test");
    }
}
