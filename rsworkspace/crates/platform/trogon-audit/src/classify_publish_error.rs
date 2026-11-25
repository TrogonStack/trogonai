use async_nats::jetstream::ErrorCode;
use async_nats::jetstream::context::{PublishError, PublishErrorKind};

use crate::sink_error::SinkError;

/// Sorts a publisher's error into the three things the audit path distinguishes.
///
/// `trogon_nats::jetstream::JetStreamPublisher::PublishError` is only an error
/// type, so the sink cannot tell a full stream from an unreachable one without
/// this.
pub trait ClassifyPublishError {
    fn classify(&self) -> SinkError;
}

impl ClassifyPublishError for PublishError {
    fn classify(&self) -> SinkError {
        match self.kind() {
            PublishErrorKind::TimedOut => SinkError::Timeout,
            PublishErrorKind::Other if is_stream_refusing_writes(self) => SinkError::StreamFull,
            _ => SinkError::Unreachable,
        }
    }
}

/// A `discard: new` stream that is at a limit acknowledges with the server's
/// "stream store failed" code. The server uses the same code for a store fault
/// such as a failing disk, and both mean the stream will not take the record, so
/// they refuse identically.
fn is_stream_refusing_writes(error: &PublishError) -> bool {
    std::error::Error::source(error)
        .and_then(|source| source.downcast_ref::<async_nats::jetstream::Error>())
        .is_some_and(|error| error.error_code() == ErrorCode::STREAM_STORE_FAILED)
}

#[cfg(any(test, feature = "test-support"))]
impl ClassifyPublishError for trogon_nats::mocks::MockError {
    fn classify(&self) -> SinkError {
        SinkError::Unreachable
    }
}

#[cfg(test)]
mod tests;
