use async_nats::jetstream::kv;
use trogon_eventsourcing::{Snapshot, list_snapshots};
use trogon_nats::jetstream::JetStreamGetKeyValue;

use crate::{
    JobSpec,
    error::CronError,
    store::{SNAPSHOT_STORE_CONFIG, open_snapshot_bucket},
};

#[derive(Debug, Clone, Default)]
pub struct ListJobsCommand;

pub async fn run<J>(js: &J, _command: ListJobsCommand) -> Result<Vec<Snapshot<JobSpec>>, CronError>
where
    J: JetStreamGetKeyValue<Store = kv::Store>,
{
    let bucket = open_snapshot_bucket(js).await?;
    list_snapshots::<JobSpec>(&bucket, SNAPSHOT_STORE_CONFIG)
        .await
        .map_err(CronError::from)
}
