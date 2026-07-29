//! Crate-wide constants.

/// Prefix used to encode user event headers into NATS message headers.
pub const TROGON_EVENT_HEADER_PREFIX: &str = "Trogon-Header-";
/// Header that stores the domain event type.
pub const TROGON_EVENT_TYPE: &str = "Trogon-Event-Type";

pub(crate) const NATS_BATCH_COMMIT: &str = "Nats-Batch-Commit";
pub(crate) const NATS_BATCH_ID: &str = "Nats-Batch-Id";
pub(crate) const NATS_BATCH_SEQUENCE: &str = "Nats-Batch-Sequence";

pub(crate) const METER_NAME: &str = "trogon-decider-nats";

pub(crate) const SNAPSHOT_DATA_KEY_NAMESPACE: &str = "snapshots.data";
pub(crate) const SNAPSHOT_CHECKPOINT_KEY_NAMESPACE: &str = "snapshots.checkpoint";
