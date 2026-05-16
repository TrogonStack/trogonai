use futures::StreamExt;

use async_nats::jetstream::kv;

use crate::{error::CronError, kv::CRON_JOBS_CHECKPOINT_KEY, read_model::CronJob};

#[derive(Debug, Clone, Default)]
pub struct ListJobsCommand;

pub async fn run(store: &kv::Store, _command: ListJobsCommand) -> Result<Vec<CronJob>, CronError> {
    let mut keys = store
        .keys()
        .await
        .map_err(|source| CronError::kv_source("failed to list projected cron job keys", source))?;
    let mut jobs = Vec::new();

    while let Some(result) = keys.next().await {
        let key = result.map_err(|source| CronError::kv_source("failed to read projected cron job key", source))?;
        if key == CRON_JOBS_CHECKPOINT_KEY {
            continue;
        }
        let Some(entry) = store
            .entry(key)
            .await
            .map_err(|source| CronError::kv_source("failed to read projected cron job value", source))?
        else {
            continue;
        };
        let job = serde_json::from_slice(&entry.value).map_err(CronError::from)?;
        jobs.push(job);
    }

    Ok(jobs)
}
