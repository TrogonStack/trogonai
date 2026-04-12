use async_nats::jetstream::kv;
use trogon_nats::jetstream::JetStreamGetKeyValue;

use crate::{VersionedJobSpec, error::CronError};

use super::list_snapshots;

#[derive(Debug, Clone, Default)]
pub struct ListJobsCommand;

pub(super) async fn run<J>(
    js: &J,
    _command: ListJobsCommand,
) -> Result<Vec<VersionedJobSpec>, CronError>
where
    J: JetStreamGetKeyValue<Store = kv::Store>,
{
    list_snapshots::run(js).await
}
