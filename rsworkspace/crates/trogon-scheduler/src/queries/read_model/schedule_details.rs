use super::{MessageEnvelope, ScheduleEventDelivery, ScheduleEventSchedule, ScheduleEventStatus};

#[derive(Debug, Clone, PartialEq)]
pub struct ScheduleDetails {
    pub status: ScheduleEventStatus,
    pub schedule: ScheduleEventSchedule,
    pub delivery: ScheduleEventDelivery,
    pub message: MessageEnvelope,
}
