use async_nats::jetstream::stream::{DiscardPolicy, Info, RetentionPolicy, StorageType};

pub(crate) fn is_exclusive_snapshot(info: &Info, subject: &str) -> bool {
    let config = &info.config;
    config.subjects.as_slice() == [subject]
        && config.max_messages == 1
        && config.max_messages_per_subject == 1
        && config.discard == DiscardPolicy::Old
        && config.retention == RetentionPolicy::Limits
        && config.storage == StorageType::File
        && !config.no_ack
        && !config.sealed
        && config.mirror.is_none()
        && config.sources.as_ref().is_none_or(Vec::is_empty)
        && config.republish.is_none()
        && config.subject_transform.is_none()
}
