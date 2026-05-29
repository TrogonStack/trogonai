use buffa::MessageField;
use trogonai_proto::scheduler::schedules::v1;

use super::{Job, MessageEnvelope, ScheduleEventDelivery, ScheduleEventSchedule, ScheduleEventStatus};

#[derive(Debug, Clone, PartialEq)]
pub struct ScheduleDetails {
    pub status: ScheduleEventStatus,
    pub schedule: ScheduleEventSchedule,
    pub delivery: ScheduleEventDelivery,
    pub message: MessageEnvelope,
}

pub(crate) fn schedule_created_from_job(job: &Job) -> v1::ScheduleCreated {
    let details = ScheduleDetails::from(job);

    v1::ScheduleCreated {
        schedule_id: job.id.as_str().to_string(),
        status: MessageField::some(v1::ScheduleStatus::from(details.status)),
        schedule: MessageField::some(v1::Schedule::from(&details.schedule)),
        delivery: MessageField::some(v1::Delivery::from(&details.delivery)),
        message: MessageField::some(v1::Message::from(&details.message)),
    }
}
