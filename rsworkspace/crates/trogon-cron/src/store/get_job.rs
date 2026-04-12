use async_nats::jetstream::kv;
use trogon_nats::jetstream::JetStreamGetKeyValue;

use crate::{JobId, VersionedJobSpec, error::CronError};

use super::load_snapshot;

#[derive(Debug, Clone)]
pub struct GetJobCommand {
    pub id: JobId,
}

pub(super) async fn run<J>(
    js: &J,
    command: GetJobCommand,
) -> Result<Option<VersionedJobSpec>, CronError>
where
    J: JetStreamGetKeyValue<Store = kv::Store>,
{
    load_snapshot::run(js, command.id.as_str()).await
}
