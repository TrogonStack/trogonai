use async_nats::jetstream::kv;

use crate::error::SchedulerError;

use super::ScheduleId;
use super::decode::decode_schedule;
use super::read_model::Schedule;

#[derive(Debug, Clone)]
pub struct GetSchedule {
    pub id: ScheduleId,
}

impl GetSchedule {
    pub const fn new(id: ScheduleId) -> Self {
        Self { id }
    }
}

pub async fn run(store: &kv::Store, command: GetSchedule) -> Result<Option<Schedule>, SchedulerError> {
    let Some(value) = store
        .get(command.id.as_str())
        .await
        .map_err(|source| SchedulerError::kv_source("failed to read projected schedule", source))?
    else {
        return Ok(None);
    };

    decode_schedule(&value).map(Some)
}
