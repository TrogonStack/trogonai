use async_nats::jetstream::kv;

use crate::{CronJob, error::CronError};

#[derive(Debug, Clone)]
pub struct GetJobCommand {
    pub id: String,
}

pub async fn run(store: &kv::Store, command: GetJobCommand) -> Result<Option<CronJob>, CronError> {
    let Some(entry) = store
        .entry(command.id)
        .await
        .map_err(|source| CronError::kv_source("failed to read projected cron job", source))?
    else {
        return Ok(None);
    };

    serde_json::from_slice(&entry.value)
        .map(Some)
        .map_err(CronError::from)
}
