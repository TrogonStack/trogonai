use crate::error::TranscriptError;
use async_nats::jetstream;
use bytes::Bytes;
use std::future::Future;

/// Abstraction over the JetStream publish operation.
///
/// Implemented by `NatsTranscriptPublisher` for production and by
/// `MockTranscriptPublisher` (behind the `test-support` feature) for unit tests.
/// Using a trait here keeps `Session<P>` fully testable without a real NATS server.
pub trait TranscriptPublisher: Send + Sync + Clone + 'static {
    fn publish(
        &self,
        subject: String,
        payload: Bytes,
    ) -> impl Future<Output = Result<(), TranscriptError>> + Send;
}

/// Production implementation: publishes to a real NATS JetStream context and
/// awaits the server acknowledgement before returning.
#[derive(Clone)]
pub struct NatsTranscriptPublisher {
    js: jetstream::Context,
}

impl NatsTranscriptPublisher {
    pub fn new(js: jetstream::Context) -> Self {
        Self { js }
    }
}

impl TranscriptPublisher for NatsTranscriptPublisher {
    async fn publish(&self, subject: String, payload: Bytes) -> Result<(), TranscriptError> {
        self.js
            .publish(subject, payload)
            .await
            .map_err(|e| TranscriptError::Publish(e.to_string()))?
            .await
            .map_err(|e| TranscriptError::Publish(e.to_string()))?;
        Ok(())
    }
}

/// In-memory publisher for unit tests.
///
/// Collects every `(subject, payload)` pair into an `Arc<Mutex<Vec<...>>>` so
/// tests can assert on what was published without a real NATS connection.
#[cfg(any(test, feature = "test-support"))]
pub mod mock {
    use super::*;
    use std::sync::{Arc, Mutex};

    #[derive(Clone, Default)]
    pub struct MockTranscriptPublisher {
        pub published: Arc<Mutex<Vec<(String, Bytes)>>>,
    }

    impl MockTranscriptPublisher {
        pub fn new() -> Self {
            Self::default()
        }

        /// Drain and return all published entries so far.
        pub fn take_published(&self) -> Vec<(String, Bytes)> {
            self.published.lock().unwrap().drain(..).collect()
        }
    }

    impl TranscriptPublisher for MockTranscriptPublisher {
        async fn publish(&self, subject: String, payload: Bytes) -> Result<(), TranscriptError> {
            self.published.lock().unwrap().push((subject, payload));
            Ok(())
        }
    }
}
