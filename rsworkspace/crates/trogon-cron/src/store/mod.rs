mod connect;
mod cron_jobs_bucket;
mod events_stream;
mod snapshot_bucket;
mod stream_subject;

use trogon_eventsourcing::SnapshotStoreConfig;

use crate::kv::{CRON_JOBS_SNAPSHOT_KEY_PREFIX, CRON_JOBS_SNAPSHOT_LAST_EVENT_SEQUENCE_KEY};

pub use connect::{EventStore, Store, connect_store};
pub(crate) use cron_jobs_bucket::run as open_cron_jobs_bucket;
pub(crate) use events_stream::run as open_events_stream;
pub use snapshot_bucket::run as open_snapshot_bucket;

pub const SNAPSHOT_STORE_CONFIG: SnapshotStoreConfig<'static> = SnapshotStoreConfig::new(
    CRON_JOBS_SNAPSHOT_KEY_PREFIX,
    Some(CRON_JOBS_SNAPSHOT_LAST_EVENT_SEQUENCE_KEY),
);
