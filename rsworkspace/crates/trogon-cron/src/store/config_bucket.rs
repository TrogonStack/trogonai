use async_nats::jetstream::kv;
use trogon_nats::jetstream::JetStreamGetKeyValue;

use crate::{error::CronError, kv::CONFIG_BUCKET};

pub(super) async fn run<J>(js: &J) -> Result<kv::Store, CronError>
where
    J: JetStreamGetKeyValue<Store = kv::Store>,
{
    js.get_key_value(CONFIG_BUCKET)
        .await
        .map_err(|source| CronError::kv_source("failed to open config bucket", source))
}
