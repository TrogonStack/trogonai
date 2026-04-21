mod connect;
mod cron_jobs_bucket;
mod events_stream;
mod snapshot_bucket;
mod stream_subject;

use trogon_eventsourcing::snapshot::{SnapshotSchema, SnapshotStoreConfig};

pub use connect::{EventStore, Store, connect_store};
pub(crate) use cron_jobs_bucket::run as open_cron_jobs_bucket;
pub(crate) use events_stream::run as open_events_stream;
pub use snapshot_bucket::run as open_snapshot_bucket;

struct CronJobsSnapshotSchema;

impl SnapshotSchema for CronJobsSnapshotSchema {
    const SNAPSHOT_STREAM_PREFIX: &'static str = "cron_jobs.v2.";
    const CHECKPOINT_NAME: Option<&'static str> = Some("last_event_sequence");
}

pub fn snapshot_store_config() -> SnapshotStoreConfig {
    CronJobsSnapshotSchema::snapshot_store_config()
}
