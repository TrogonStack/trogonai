use super::{JobEventDelivery, JobEventSchedule, JobEventStatus, MessageEnvelope};

#[derive(Debug, Clone, PartialEq)]
pub struct JobDetails {
    pub status: JobEventStatus,
    pub schedule: JobEventSchedule,
    pub delivery: JobEventDelivery,
    pub message: MessageEnvelope,
}
