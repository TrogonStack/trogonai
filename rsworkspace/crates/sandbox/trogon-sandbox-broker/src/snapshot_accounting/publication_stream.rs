use async_nats::jetstream;
use async_nats::jetstream::stream::{Config, DiscardPolicy, RetentionPolicy, StorageType};
use trogon_sandbox_protocol::subjects::SNAPSHOT_PUBLISHED;

use super::snapshot_accounting_error::SnapshotAccountingError;
use crate::constants::{
    SNAPSHOT_ACCOUNTING_BUCKET_MAX_BYTES, SNAPSHOT_PUBLICATION_MAX_MESSAGE_BYTES, SNAPSHOT_PUBLICATION_MAX_MESSAGES,
    SNAPSHOT_PUBLICATION_STREAM,
};

pub async fn ensure_publication_stream(
    js: &jetstream::Context,
) -> std::result::Result<jetstream::stream::Stream, SnapshotAccountingError> {
    let stream = js
        .get_or_create_stream(Config {
            name: SNAPSHOT_PUBLICATION_STREAM.to_string(),
            subjects: vec![SNAPSHOT_PUBLISHED.to_string()],
            retention: RetentionPolicy::WorkQueue,
            discard: DiscardPolicy::New,
            max_bytes: SNAPSHOT_ACCOUNTING_BUCKET_MAX_BYTES,
            max_messages: SNAPSHOT_PUBLICATION_MAX_MESSAGES,
            max_message_size: SNAPSHOT_PUBLICATION_MAX_MESSAGE_BYTES,
            max_consumers: 1,
            storage: StorageType::File,
            num_replicas: 1,
            ..Default::default()
        })
        .await?;
    let config = &stream.cached_info().config;
    if !(config.retention == RetentionPolicy::WorkQueue
        && config.discard == DiscardPolicy::New
        && config.storage == StorageType::File
        && !config.no_ack
        && config.max_age.is_zero()
        && (1..=SNAPSHOT_ACCOUNTING_BUCKET_MAX_BYTES).contains(&config.max_bytes)
        && (1..=SNAPSHOT_PUBLICATION_MAX_MESSAGES).contains(&config.max_messages)
        && (1..=SNAPSHOT_PUBLICATION_MAX_MESSAGE_BYTES).contains(&config.max_message_size)
        && config.max_consumers == 1
        && config.subjects == [SNAPSHOT_PUBLISHED])
    {
        return Err(SnapshotAccountingError::StreamPolicy);
    }
    Ok(stream)
}
