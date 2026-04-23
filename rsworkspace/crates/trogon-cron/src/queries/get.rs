use async_nats::jetstream::kv;

use crate::{CronJob, JobId, error::CronError};

#[derive(Debug, Clone)]
pub struct GetJobCommand {
    pub id: JobId,
}

impl GetJobCommand {
    pub const fn new(id: JobId) -> Self {
        Self { id }
    }
}

pub async fn run(store: &kv::Store, command: GetJobCommand) -> Result<Option<CronJob>, CronError> {
    let Some(entry) = store
        .entry(command.id.as_str())
        .await
        .map_err(|source| CronError::kv_source("failed to read projected cron job", source))?
    else {
        return Ok(None);
    };

    serde_json::from_slice(&entry.value).map(Some).map_err(CronError::from)
}
