mod append_events;
mod catch_up_snapshots;
mod config_bucket;
mod connect;
mod events_stream;
mod load_and_watch;
mod project_event_to_snapshot;
mod rewrite_projection;
mod snapshot_bucket;

use futures::Stream;
use std::pin::Pin;
use trogon_eventsourcing::{SnapshotSchemaVersion, SnapshotStoreConfig};

use crate::config::JobSpec;
use crate::error::CronError;
use crate::kv::{SNAPSHOT_KEY_PREFIX, SNAPSHOT_LAST_EVENT_SEQUENCE_KEY};

pub use append_events::run as append_events;
pub use connect::connect_store;
pub use load_and_watch::{LoadAndWatchCommand, run as load_and_watch};
pub use snapshot_bucket::run as open_snapshot_bucket;

pub type ConfigWatchStream = Pin<Box<dyn Stream<Item = JobSpecChange> + Send + 'static>>;
pub type LoadAndWatchResult = Result<(Vec<JobSpec>, ConfigWatchStream), CronError>;

#[derive(Debug, Clone)]
pub enum JobSpecChange {
    Put(JobSpec),
    Delete(String),
}

pub const SNAPSHOT_STORE_CONFIG: SnapshotStoreConfig<'static> = SnapshotStoreConfig::new(
    SNAPSHOT_KEY_PREFIX,
    SNAPSHOT_LAST_EVENT_SEQUENCE_KEY,
    SnapshotSchemaVersion::V2,
);
