use async_nats::jetstream::{self, stream};
use trogon_decider_nats::{JetStreamStore, ensure_bucket, ensure_stream};

use super::{
    PeerSnapshotConfigMismatchError, PeerStoreConfig, PeerStoreError, PeerStreamConfigMismatchError,
    PeerSubjectResolver,
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

fn validate_event_stream(
    required: &stream::Config,
    actual: &stream::Config,
) -> Result<(), PeerStreamConfigMismatchError> {
    if actual.discard != stream::DiscardPolicy::New {
        return Err(PeerStreamConfigMismatchError::Discard);
    }
    if actual.retention != stream::RetentionPolicy::Limits {
        return Err(PeerStreamConfigMismatchError::Retention);
    }
    if actual.max_bytes != required.max_bytes {
        return Err(PeerStreamConfigMismatchError::MaxBytes);
    }
    if !actual.max_age.is_zero() {
        return Err(PeerStreamConfigMismatchError::MaxAge);
    }
    if actual.max_messages != -1 {
        return Err(PeerStreamConfigMismatchError::MaxMessages);
    }
    if actual.max_messages_per_subject != -1 {
        return Err(PeerStreamConfigMismatchError::MaxMessagesPerSubject);
    }
    if !actual.allow_atomic_publish {
        return Err(PeerStreamConfigMismatchError::AtomicPublish);
    }
    if actual.subjects != required.subjects {
        return Err(PeerStreamConfigMismatchError::Subjects);
    }
    if actual.storage != stream::StorageType::File {
        return Err(PeerStreamConfigMismatchError::Storage);
    }
    if actual.allow_rollup {
        return Err(PeerStreamConfigMismatchError::Rollup);
    }
    if actual.allow_message_ttl {
        return Err(PeerStreamConfigMismatchError::MessageTtl);
    }
    if actual.mirror.is_some() || actual.sources.as_ref().is_some_and(|sources| !sources.is_empty()) {
        return Err(PeerStreamConfigMismatchError::Sources);
    }
    Ok(())
}

fn validate_snapshots(
    required: &jetstream::kv::Config,
    actual: &stream::Config,
) -> Result<(), PeerSnapshotConfigMismatchError> {
    if actual.max_messages_per_subject != required.history {
        return Err(PeerSnapshotConfigMismatchError::History);
    }
    if actual.max_bytes != required.max_bytes {
        return Err(PeerSnapshotConfigMismatchError::MaxBytes);
    }
    if actual.max_message_size != required.max_value_size {
        return Err(PeerSnapshotConfigMismatchError::MaxValueSize);
    }
    if !actual.max_age.is_zero() {
        return Err(PeerSnapshotConfigMismatchError::MaxAge);
    }
    if actual.storage != required.storage {
        return Err(PeerSnapshotConfigMismatchError::Storage);
    }
    if actual.subjects != [format!("$KV.{}.>", required.bucket)] {
        return Err(PeerSnapshotConfigMismatchError::Subjects);
    }
    if actual.discard != stream::DiscardPolicy::New {
        return Err(PeerSnapshotConfigMismatchError::Discard);
    }
    if actual.mirror.is_some() || actual.sources.as_ref().is_some_and(|sources| !sources.is_empty()) {
        return Err(PeerSnapshotConfigMismatchError::Sources);
    }
    Ok(())
}
