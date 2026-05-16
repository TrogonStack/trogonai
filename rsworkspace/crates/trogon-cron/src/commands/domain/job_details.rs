use buffa::MessageField;
use trogon_cron_jobs_proto::v1;

use super::{Job, JobEventDelivery, JobEventSchedule, JobEventStatus, MessageEnvelope};

#[derive(Debug, Clone, PartialEq)]
pub struct JobDetails {
    pub status: JobEventStatus,
    pub schedule: JobEventSchedule,
    pub delivery: JobEventDelivery,
    pub message: MessageEnvelope,
}

impl From<&JobDetails> for v1::JobDetails {
    fn from(value: &JobDetails) -> Self {
        v1::JobDetails {
            status: v1::JobStatus::from(value.status),
            schedule: MessageField::some(v1::JobSchedule::from(&value.schedule)),
            delivery: MessageField::some(v1::JobDelivery::from(&value.delivery)),
            message: MessageField::some(v1::JobMessage::from(&value.message)),
        }
    }
}

impl From<&Job> for v1::JobDetails {
    fn from(value: &Job) -> Self {
        Self::from(&JobDetails::from(value))
    }
}
