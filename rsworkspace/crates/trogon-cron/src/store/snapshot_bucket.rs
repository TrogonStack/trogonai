use async_nats::jetstream::kv;
use trogon_nats::jetstream::JetStreamGetKeyValue;

use crate::{error::CronError, kv::SNAPSHOT_BUCKET};

pub async fn run<J>(js: &J) -> Result<kv::Store, CronError>
where
    J: JetStreamGetKeyValue<Store = kv::Store>,
{
    js.get_key_value(SNAPSHOT_BUCKET)
        .await
        .map_err(|source| CronError::kv_source("failed to open snapshot bucket", source))
}
