#![cfg_attr(coverage, allow(dead_code, unused_imports))]

use async_nats::jetstream::kv;
use trogon_nats::jetstream::JetStreamGetKeyValue;

use crate::{error::CronError, kv::COMMAND_SNAPSHOT_BUCKET};

#[cfg(not(coverage))]
pub async fn run<J>(js: &J) -> Result<kv::Store, CronError>
where
    J: JetStreamGetKeyValue<Store = kv::Store>,
{
    js.get_key_value(COMMAND_SNAPSHOT_BUCKET)
        .await
        .map_err(|source| CronError::kv_source("failed to open command snapshot bucket", source))
}

#[cfg(coverage)]
pub async fn run<J>(_js: &J) -> Result<kv::Store, CronError>
where
    J: JetStreamGetKeyValue<Store = kv::Store>,
{
    Err(CronError::kv_source(
        "coverage stub does not open the command snapshot bucket",
        std::io::Error::other(COMMAND_SNAPSHOT_BUCKET),
    ))
}
