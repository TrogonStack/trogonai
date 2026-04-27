use serde::{Deserialize, Serialize};

use crate::{JobEventProtoError, proto::v1};

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

impl TryFrom<(String, v1::JobDetails)> for CronJob {
    type Error = JobEventProtoError;

    fn try_from((id, job): (String, v1::JobDetails)) -> Result<Self, Self::Error> {
        Ok(Self::from((id, JobDetails::try_from(job)?)))
    }
}
