mod append_events;
mod config_bucket;
mod connect;
mod events_stream;
mod snapshot_bucket;

use trogon_eventsourcing::{SnapshotSchemaVersion, SnapshotStoreConfig};

use crate::kv::{SNAPSHOT_KEY_PREFIX, SNAPSHOT_LAST_EVENT_SEQUENCE_KEY};

pub use append_events::run as append_events;
pub(crate) use append_events::stream_subject_state;
pub(crate) use config_bucket::run as open_config_bucket;
pub use connect::connect_store;
pub(crate) use events_stream::run as open_events_stream;
pub use snapshot_bucket::run as open_snapshot_bucket;

pub const SNAPSHOT_STORE_CONFIG: SnapshotStoreConfig<'static> = SnapshotStoreConfig::new(
    SNAPSHOT_KEY_PREFIX,
    SNAPSHOT_LAST_EVENT_SEQUENCE_KEY,
    SnapshotSchemaVersion::V2,
);
