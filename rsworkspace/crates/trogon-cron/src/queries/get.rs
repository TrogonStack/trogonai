use trogon_eventsourcing::StreamCommand;

use crate::{JobId, JobSpec, error::CronError, store::GetCronJobsBucket};

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

pub async fn run<J>(store: &J, command: GetJobCommand) -> Result<Option<JobSpec>, CronError>
where
    J: GetCronJobsBucket<Store = async_nats::jetstream::kv::Store>,
{
    let Some(entry) = store
        .cron_jobs_bucket()
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
