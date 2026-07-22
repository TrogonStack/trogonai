#![cfg_attr(coverage, allow(unused_imports))]

use futures::StreamExt;

use async_nats::jetstream::kv;

use crate::{error::SchedulerError, projections::storage::SCHEDULES_CHECKPOINT_KEY};

use super::decode::decode_schedule;
use super::read_model::Schedule;

#[derive(Debug, Clone, Default)]
pub struct ListSchedules;

#[cfg(not(coverage))]
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
        let Some(value) = store
            .get(key.as_str())
            .await
            .map_err(|source| SchedulerError::kv_source("failed to read projected schedule value", source))?
        else {
            continue;
        };
        // One corrupt entry must not suppress every other schedule in the listing.
        // Skip it with a warning; catch-up rebuilds the read model on restart.
        match decode_schedule(&value) {
            Ok(job) => jobs.push(job),
            Err(source) => {
                tracing::warn!(%key, %source, "skipping unreadable projected schedule entry during list");
            }
        }
    }

    Ok(jobs)
}
