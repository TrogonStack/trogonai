use async_nats::jetstream::kv;
use futures::StreamExt;
use trogon_nats::jetstream::JetStreamGetKeyValue;

use crate::{JobSpec, error::CronError, store::open_cron_jobs_bucket};

#[derive(Debug, Clone, Default)]
pub struct ListJobsCommand;

pub async fn run<J>(js: &J, _command: ListJobsCommand) -> Result<Vec<JobSpec>, CronError>
where
    J: JetStreamGetKeyValue<Store = kv::Store>,
{
    let bucket = open_cron_jobs_bucket(js).await?;
    let mut keys = bucket
        .keys()
        .await
        .map_err(|source| CronError::kv_source("failed to list projected cron job keys", source))?;
    let mut jobs = Vec::new();

    while let Some(result) = keys.next().await {
        let key = result.map_err(|source| {
            CronError::kv_source("failed to read projected cron job key", source)
        })?;
        let Some(entry) = bucket.entry(key).await.map_err(|source| {
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
