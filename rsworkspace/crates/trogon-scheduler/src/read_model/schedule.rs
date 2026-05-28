use serde::{Deserialize, Serialize};

use super::{MessageEnvelope, ScheduleDetails, ScheduleEventDelivery, ScheduleEventSchedule, ScheduleEventStatus};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Schedule {
    pub id: String,
    #[serde(default, rename = "state")]
    pub status: ScheduleEventStatus,
    pub schedule: ScheduleEventSchedule,
    pub delivery: ScheduleEventDelivery,
    pub message: MessageEnvelope,
}

impl Schedule {
    pub fn is_enabled(&self) -> bool {
        matches!(self.status, ScheduleEventStatus::Scheduled)
    }
}

impl From<(String, ScheduleDetails)> for Schedule {
    fn from((id, job): (String, ScheduleDetails)) -> Self {
        Self {
            id,
            status: job.status,
            schedule: job.schedule,
            delivery: job.delivery,
            message: job.message,
        }
    }
}
