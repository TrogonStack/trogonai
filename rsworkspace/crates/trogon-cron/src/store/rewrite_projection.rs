#![cfg_attr(coverage, allow(dead_code))]

use async_nats::jetstream::kv;
use futures::StreamExt;
use trogon_eventsourcing::Snapshot;
use trogon_nats::jetstream::JetStreamGetKeyValue;

use crate::{JobSpec, error::CronError, kv::JOBS_KEY_PREFIX};

use super::config_bucket;

pub(super) async fn run<J>(js: &J, jobs: &[Snapshot<JobSpec>]) -> Result<(), CronError>
where
    J: JetStreamGetKeyValue<Store = kv::Store>,
{
    let kv = config_bucket::run(js).await?;
    let mut keys = kv
        .keys()
        .await
        .map_err(|source| CronError::kv_source("failed to list projection keys", source))?;

    while let Some(result) = keys.next().await {
        let key = result
            .map_err(|source| CronError::kv_source("failed to read projection key", source))?;
        if key.starts_with(JOBS_KEY_PREFIX) {
            let _ = kv.purge(key).await;
        }
    }

    for job in jobs {
        let key = format!("{JOBS_KEY_PREFIX}{}", job.payload.id);
        let value = serde_json::to_vec(&job.payload)?;
        kv.put(key, value.into()).await.map_err(|source| {
            CronError::kv_source("failed to write projected job state", source)
        })?;
    }

    Ok(())
}
