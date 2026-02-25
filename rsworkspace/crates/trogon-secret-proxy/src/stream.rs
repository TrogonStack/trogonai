//! JetStream stream provisioning for proxy requests.

use async_nats::jetstream::{self, context::CreateStreamError, stream::Config as StreamConfig};
use async_nats::jetstream::stream::{RetentionPolicy, StorageType};

pub const STREAM_NAME: &str = "PROXY_REQUESTS";

/// Ensure the `PROXY_REQUESTS` JetStream stream exists.
///
/// Idempotent — safe to call on every startup. Uses a work-queue retention
/// policy so messages are removed after a worker acknowledges them.
pub async fn ensure_stream(
    jetstream: &jetstream::Context,
    outbound_subject: &str,
) -> Result<jetstream::stream::Stream, CreateStreamError> {
    let config = StreamConfig {
        name: STREAM_NAME.to_string(),
        subjects: vec![outbound_subject.to_string()],
        retention: RetentionPolicy::WorkQueue,
        storage: StorageType::Memory,
        ..Default::default()
    };

    jetstream.get_or_create_stream(config).await
}
