use async_nats::jetstream::kv;

use crate::{error::SchedulerError, read_model::Schedule};

use super::ScheduleId;

#[derive(Debug, Clone)]
pub struct GetScheduleCommand {
    pub id: ScheduleId,
}

impl GetScheduleCommand {
    pub const fn new(id: ScheduleId) -> Self {
        Self { id }
    }
}

pub async fn run(store: &kv::Store, command: GetScheduleCommand) -> Result<Option<Schedule>, SchedulerError> {
    let Some(entry) = store
        .entry(command.id.as_str())
        .await
        .map_err(|source| SchedulerError::kv_source("failed to read projected schedule", source))?
    else {
        return Ok(None);
    };

    serde_json::from_slice(&entry.value)
        .map(Some)
        .map_err(SchedulerError::from)
}
