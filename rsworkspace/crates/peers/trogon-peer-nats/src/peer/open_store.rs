use async_nats::jetstream::{self, stream};
use trogon_decider_nats::{JetStreamStore, ensure_bucket, ensure_stream};

use super::{
    PeerSnapshotConfigMismatch, PeerStoreConfig, PeerStoreError, PeerStreamConfigMismatch, PeerSubjectResolver,
};

pub async fn open_store(
    context: jetstream::Context,
    config: &PeerStoreConfig,
) -> Result<JetStreamStore<PeerSubjectResolver>, PeerStoreError> {
    let required_stream = config.stream_config();
    let events = ensure_stream(&context, required_stream.clone()).await?;
    validate_event_stream(&required_stream, &events.cached_info().config)?;
    let required_snapshots = config.snapshot_config();
    let snapshots = ensure_bucket(&context, required_snapshots.clone()).await?;
    validate_snapshots(&required_snapshots, &snapshots.stream.cached_info().config)?;
    let resolver = PeerSubjectResolver::new(config.tenant().clone())?;
    Ok(JetStreamStore::builder(context, events, snapshots).with_subject_resolver(resolver))
}

fn validate_event_stream(required: &stream::Config, actual: &stream::Config) -> Result<(), PeerStreamConfigMismatch> {
    if actual.discard != stream::DiscardPolicy::New {
        return Err(PeerStreamConfigMismatch::Discard);
    }
    if actual.retention != stream::RetentionPolicy::Limits {
        return Err(PeerStreamConfigMismatch::Retention);
    }
    if actual.max_bytes != required.max_bytes {
        return Err(PeerStreamConfigMismatch::MaxBytes);
    }
    if !actual.max_age.is_zero() {
        return Err(PeerStreamConfigMismatch::MaxAge);
    }
    if actual.max_messages != -1 {
        return Err(PeerStreamConfigMismatch::MaxMessages);
    }
    if actual.max_messages_per_subject != -1 {
        return Err(PeerStreamConfigMismatch::MaxMessagesPerSubject);
    }
    if !actual.allow_atomic_publish {
        return Err(PeerStreamConfigMismatch::AtomicPublish);
    }
    if actual.subjects != required.subjects {
        return Err(PeerStreamConfigMismatch::Subjects);
    }
    if actual.storage != stream::StorageType::File {
        return Err(PeerStreamConfigMismatch::Storage);
    }
    if actual.allow_rollup {
        return Err(PeerStreamConfigMismatch::Rollup);
    }
    if actual.allow_message_ttl {
        return Err(PeerStreamConfigMismatch::MessageTtl);
    }
    if actual.mirror.is_some() || actual.sources.as_ref().is_some_and(|sources| !sources.is_empty()) {
        return Err(PeerStreamConfigMismatch::Sources);
    }
    Ok(())
}

fn validate_snapshots(
    required: &jetstream::kv::Config,
    actual: &stream::Config,
) -> Result<(), PeerSnapshotConfigMismatch> {
    if actual.max_messages_per_subject != required.history {
        return Err(PeerSnapshotConfigMismatch::History);
    }
    if actual.max_bytes != required.max_bytes {
        return Err(PeerSnapshotConfigMismatch::MaxBytes);
    }
    if actual.max_message_size != required.max_value_size {
        return Err(PeerSnapshotConfigMismatch::MaxValueSize);
    }
    if !actual.max_age.is_zero() {
        return Err(PeerSnapshotConfigMismatch::MaxAge);
    }
    if actual.storage != required.storage {
        return Err(PeerSnapshotConfigMismatch::Storage);
    }
    if actual.subjects != [format!("$KV.{}.>", required.bucket)] {
        return Err(PeerSnapshotConfigMismatch::Subjects);
    }
    if actual.discard != stream::DiscardPolicy::New {
        return Err(PeerSnapshotConfigMismatch::Discard);
    }
    if actual.mirror.is_some() || actual.sources.as_ref().is_some_and(|sources| !sources.is_empty()) {
        return Err(PeerSnapshotConfigMismatch::Sources);
    }
    Ok(())
}
