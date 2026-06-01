use futures::StreamExt;

use async_nats::jetstream::kv;

use crate::{error::SchedulerError, kv::SCHEDULES_CHECKPOINT_KEY, read_model::Schedule};

#[derive(Debug, Clone, Default)]
pub struct ListSchedules;

pub async fn run(store: &kv::Store, _command: ListSchedules) -> Result<Vec<Schedule>, SchedulerError> {
    let mut keys = store
        .keys()
        .await
        .map_err(|source| SchedulerError::kv_source("failed to list projected schedule keys", source))?;
    let mut jobs = Vec::new();

    while let Some(result) = keys.next().await {
        let key =
            result.map_err(|source| SchedulerError::kv_source("failed to read projected schedule key", source))?;
        if key == SCHEDULES_CHECKPOINT_KEY {
            continue;
        }
        let Some(entry) = store
            .entry(key)
            .await
            .map_err(|source| SchedulerError::kv_source("failed to read projected schedule value", source))?
        else {
            continue;
        };
        let job = serde_json::from_slice(&entry.value).map_err(SchedulerError::from)?;
        jobs.push(job);
    }

    Ok(jobs)
}
