use async_nats::jetstream::kv;
use trogon_nats::jetstream::JetStreamGetKeyValue;

use crate::{VersionedJobSpec, error::CronError, nats::snapshot_key};

use super::snapshot_bucket;

pub(super) async fn run<J>(js: &J, id: &str) -> Result<Option<VersionedJobSpec>, CronError>
where
    J: JetStreamGetKeyValue<Store = kv::Store>,
{
    let bucket = snapshot_bucket::run(js).await?;
    let Some(entry) = bucket.entry(snapshot_key(id)).await.map_err(|source| {
        CronError::kv_source("failed to read aggregate snapshot entry", source)
    })?
    else {
        return Ok(None);
    };

    serde_json::from_slice::<VersionedJobSpec>(&entry.value)
        .map(Some)
        .map_err(|source| CronError::kv_source("failed to decode aggregate snapshot entry", source))
}
