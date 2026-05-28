use buffa::MessageField;
use chrono::{DateTime, Utc};
use trogonai_proto::scheduler::schedules::v1;

use super::{
    Job, MessageEnvelope, ScheduleActor, ScheduleEventDelivery, ScheduleEventSchedule, ScheduleEventStatus,
    proto_timestamp,
};

#[derive(Debug, Clone, PartialEq)]
pub struct ScheduleDetails {
    pub status: ScheduleEventStatus,
    pub schedule: ScheduleEventSchedule,
    pub delivery: ScheduleEventDelivery,
    pub message: MessageEnvelope,
}

pub(crate) fn job_added_from_job(job: &Job, added_at: &DateTime<Utc>, actor: &ScheduleActor) -> v1::ScheduleAdded {
    let details = ScheduleDetails::from(job);

    v1::ScheduleAdded {
        schedule_id: job.id.as_str().to_string(),
        added_at: proto_timestamp(added_at),
        status: v1::ScheduleStatus::from(details.status),
        schedule: MessageField::some(v1::Schedule::from(&details.schedule)),
        delivery: MessageField::some(v1::Delivery::from(&details.delivery)),
        message: MessageField::some(v1::Message::from(&details.message)),
        added_by: actor.into(),
    }
}
