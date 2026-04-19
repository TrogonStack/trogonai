use crate::{CronJob, error::CronError, store::GetCronJobsBucket};

#[derive(Debug, Clone)]
pub struct GetJobCommand {
    pub id: String,
}

pub async fn run<J>(store: &J, command: GetJobCommand) -> Result<Option<CronJob>, CronError>
where
    J: GetCronJobsBucket<Store = async_nats::jetstream::kv::Store>,
{
    let Some(entry) = store
        .cron_jobs_bucket()
        .entry(command.id)
        .await
        .map_err(|source| CronError::kv_source("failed to read projected cron job", source))?
    else {
        return Ok(None);
    };

    serde_json::from_slice(&entry.value)
        .map(Some)
        .map_err(CronError::from)
}
