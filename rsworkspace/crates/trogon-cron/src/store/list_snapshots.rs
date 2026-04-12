use async_nats::jetstream::kv;
use futures::StreamExt;
use trogon_nats::jetstream::JetStreamGetKeyValue;

use crate::{VersionedJobSpec, error::CronError, kv::SNAPSHOT_KEY_PREFIX};

use super::snapshot_bucket;

pub(super) async fn run<J>(js: &J) -> Result<Vec<VersionedJobSpec>, CronError>
where
    J: JetStreamGetKeyValue<Store = kv::Store>,
{
    let bucket = snapshot_bucket::run(js).await?;
    let mut keys = bucket
        .keys()
        .await
        .map_err(|source| CronError::kv_source("failed to list aggregate snapshot keys", source))?;
    let mut jobs = Vec::new();

    while let Some(result) = keys.next().await {
        let key = result.map_err(|source| {
            CronError::kv_source("failed to read aggregate snapshot key", source)
        })?;
        if !key.starts_with(SNAPSHOT_KEY_PREFIX) {
            continue;
        }
        let Some(entry) = bucket.entry(key).await.map_err(|source| {
            CronError::kv_source("failed to read aggregate snapshot value", source)
        })?
        else {
            continue;
        };
        let job = serde_json::from_slice::<VersionedJobSpec>(&entry.value).map_err(|source| {
            CronError::kv_source("failed to decode aggregate snapshot value", source)
        })?;
        jobs.push(job);
    }

    Ok(jobs)
}
