#![cfg_attr(coverage, allow(dead_code, unused_imports))]

use async_nats::jetstream::kv;
use trogon_nats::jetstream::JetStreamGetKeyValue;

use crate::{error::CronError, kv::CRON_JOBS_BUCKET};

#[cfg(not(coverage))]
pub(crate) async fn run<J>(js: &J) -> Result<kv::Store, CronError>
where
    J: JetStreamGetKeyValue<Store = kv::Store>,
{
    js.get_key_value(CRON_JOBS_BUCKET)
        .await
        .map_err(|source| CronError::kv_source("failed to open cron jobs bucket", source))
}

#[cfg(coverage)]
pub(crate) async fn run<J>(_js: &J) -> Result<kv::Store, CronError>
where
    J: JetStreamGetKeyValue<Store = kv::Store>,
{
    Err(CronError::kv_source(
        "coverage stub does not open the cron jobs bucket",
        std::io::Error::other(CRON_JOBS_BUCKET),
    ))
}
