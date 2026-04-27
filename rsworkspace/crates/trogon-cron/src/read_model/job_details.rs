use trogon_cron_jobs_proto::v1;

use crate::proto::JobEventProtoError;

use super::{JobEventDelivery, JobEventSchedule, JobEventStatus, MessageEnvelope};

#[derive(Debug, Clone, PartialEq)]
pub struct JobDetails {
    pub status: JobEventStatus,
    pub schedule: JobEventSchedule,
    pub delivery: JobEventDelivery,
    pub message: MessageEnvelope,
}

impl From<&JobDetails> for v1::JobDetails {
    fn from(value: &JobDetails) -> Self {
        let mut job = v1::JobDetails::new();
        job.set_status(v1::JobStatus::from(value.status));
        job.set_schedule(v1::JobSchedule::from(&value.schedule));
        job.set_delivery(v1::JobDelivery::from(&value.delivery));
        job.set_message(v1::JobMessage::from(&value.message));
        job
    }
}

impl TryFrom<v1::JobDetails> for JobDetails {
    type Error = JobEventProtoError;

    fn try_from(value: v1::JobDetails) -> Result<Self, Self::Error> {
        if !value.has_schedule() {
            return Err(JobEventProtoError::MissingSchedule);
        }
        if !value.has_delivery() {
            return Err(JobEventProtoError::MissingDelivery);
        }
        if !value.has_message() {
            return Err(JobEventProtoError::MissingMessage);
        }
        Ok(Self {
            status: value.status().try_into()?,
            schedule: value.schedule().to_owned().try_into()?,
            delivery: value.delivery().to_owned().try_into()?,
            message: value.message().to_owned().try_into()?,
        })
    }
}
