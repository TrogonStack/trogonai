use async_nats::jetstream::kv;
use trogon_eventsourcing::load_snapshot;
use trogon_nats::jetstream::JetStreamGetKeyValue;

use crate::{JobId, VersionedJobSpec, error::CronError};

use super::{SNAPSHOT_STORE_CONFIG, snapshot_bucket};

#[derive(Debug, Clone)]
pub struct GetJobCommand {
    pub id: JobId,
}

pub async fn run<J>(
    js: &J,
    command: GetJobCommand,
) -> Result<Option<VersionedJobSpec>, CronError>
where
    J: JetStreamGetKeyValue<Store = kv::Store>,
{
    let bucket = snapshot_bucket::run(js).await?;
    load_snapshot(&bucket, SNAPSHOT_STORE_CONFIG, command.id.as_str())
        .await
        .map_err(CronError::from)
}
