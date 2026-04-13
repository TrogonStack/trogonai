use async_nats::jetstream::kv;
use trogon_eventsourcing::{Snapshot, StreamCommand, load_snapshot};
use trogon_nats::jetstream::JetStreamGetKeyValue;

use crate::{JobId, JobSpec, error::CronError};

use super::{SNAPSHOT_STORE_CONFIG, snapshot_bucket};

#[derive(Debug, Clone)]
pub struct GetJobCommand {
    pub id: JobId,
}

impl StreamCommand for GetJobCommand {
    type StreamId = JobId;

    fn stream_id(&self) -> &Self::StreamId {
        &self.id
    }
}

pub async fn run<J>(js: &J, command: GetJobCommand) -> Result<Option<Snapshot<JobSpec>>, CronError>
where
    J: JetStreamGetKeyValue<Store = kv::Store>,
{
    let bucket = snapshot_bucket::run(js).await?;
    load_snapshot(&bucket, SNAPSHOT_STORE_CONFIG, command.stream_id().as_str())
        .await
        .map_err(CronError::from)
}
