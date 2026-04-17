use async_nats::jetstream::kv;
use trogon_eventsourcing::StreamCommand;
use trogon_nats::jetstream::JetStreamGetKeyValue;

use crate::{JobId, JobSpec, error::CronError, store::open_cron_jobs_bucket};

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

pub async fn run<J>(js: &J, command: GetJobCommand) -> Result<Option<JobSpec>, CronError>
where
    J: JetStreamGetKeyValue<Store = kv::Store>,
{
    let bucket = open_cron_jobs_bucket(js).await?;
    let Some(entry) = bucket
        .entry(command.stream_id().as_str().to_string())
        .await
        .map_err(|source| CronError::kv_source("failed to read projected cron job", source))?
    else {
        return Ok(None);
    };

    serde_json::from_slice(&entry.value)
        .map(Some)
        .map_err(CronError::from)
}
