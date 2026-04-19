use futures::StreamExt;

use crate::{CronJob, error::CronError, store::GetCronJobsBucket};

#[derive(Debug, Clone, Default)]
pub struct ListJobsCommand;

pub async fn run<J>(store: &J, _command: ListJobsCommand) -> Result<Vec<CronJob>, CronError>
where
    J: GetCronJobsBucket<Store = async_nats::jetstream::kv::Store>,
{
    let mut keys =
        store.cron_jobs_bucket().keys().await.map_err(|source| {
            CronError::kv_source("failed to list projected cron job keys", source)
        })?;
    let mut jobs = Vec::new();

    while let Some(result) = keys.next().await {
        let key = result.map_err(|source| {
            CronError::kv_source("failed to read projected cron job key", source)
        })?;
        let Some(entry) = store
            .cron_jobs_bucket()
            .entry(key)
            .await
            .map_err(|source| {
                CronError::kv_source("failed to read projected cron job value", source)
            })?
        else {
            continue;
        };
        let job = serde_json::from_slice(&entry.value).map_err(CronError::from)?;
        jobs.push(job);
    }

    Ok(jobs)
}
