use serde::{Deserialize, Serialize};

use crate::events::{JobDetails, JobEventDelivery, JobEventSchedule, JobEventState, MessageSpec};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct CronJob {
    pub id: String,
    #[serde(default)]
    pub state: JobEventState,
    pub schedule: JobEventSchedule,
    pub delivery: JobEventDelivery,
    pub message: MessageSpec,
}

impl CronJob {
    pub fn is_enabled(&self) -> bool {
        matches!(self.state, JobEventState::Enabled)
    }
}

impl From<(String, JobDetails)> for CronJob {
    fn from((id, job): (String, JobDetails)) -> Self {
        Self {
            id,
            state: job.state,
            schedule: job.schedule,
            delivery: job.delivery,
            message: job.message,
        }
    }
}
