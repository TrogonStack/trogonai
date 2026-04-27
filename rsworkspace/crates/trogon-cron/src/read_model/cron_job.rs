use serde::{Deserialize, Serialize};

use super::{JobDetails, JobEventDelivery, JobEventSchedule, JobEventStatus, MessageEnvelope};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct CronJob {
    pub id: String,
    #[serde(default, rename = "state")]
    pub status: JobEventStatus,
    pub schedule: JobEventSchedule,
    pub delivery: JobEventDelivery,
    pub message: MessageEnvelope,
}

impl CronJob {
    pub fn is_enabled(&self) -> bool {
        matches!(self.status, JobEventStatus::Enabled)
    }
}

impl From<(String, JobDetails)> for CronJob {
    fn from((id, job): (String, JobDetails)) -> Self {
        Self {
            id,
            status: job.status,
            schedule: job.schedule,
            delivery: job.delivery,
            message: job.message,
        }
    }
}
