use async_nats::jetstream::Context;
use async_nats::jetstream::stream::{Config as StreamConfig, RetentionPolicy, StorageType};
use std::time::Duration;

/// Name of the single JetStream stream that carries all Slack traffic.
pub const STREAM_NAME: &str = "SLACK";

/// Subjects covered by the stream.
/// `slack.outbound.stream.start` uses Core NATS request/reply (raw client
/// publish) and therefore never lands in the JetStream stream even though
/// `slack.outbound.>` matches it in theory — Core NATS messages are not
/// persisted to JetStream streams.
const STREAM_SUBJECTS: &[&str] = &[
    "slack.inbound.>",
    "slack.outbound.>",
];

/// Create (or verify) the `SLACK` JetStream stream.
///
/// Safe to call on every startup: `get_or_create_stream` is idempotent.
pub async fn ensure_slack_stream(js: &Context) -> Result<(), async_nats::Error> {
    js.get_or_create_stream(StreamConfig {
        name: STREAM_NAME.to_string(),
        subjects: STREAM_SUBJECTS.iter().map(|s| s.to_string()).collect(),
        // Each message is removed once the first consumer ACKs it.
        retention: RetentionPolicy::WorkQueue,
        // Survive server restarts.
        storage: StorageType::File,
        // Drop unprocessed messages after one hour.
        max_age: Duration::from_secs(3600),
        // Safety cap: 100 k messages per subject.
        max_messages_per_subject: 100_000,
        ..Default::default()
    })
    .await?;
    Ok(())
}
