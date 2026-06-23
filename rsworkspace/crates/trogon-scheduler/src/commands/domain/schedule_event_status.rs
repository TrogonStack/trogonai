use trogonai_proto::scheduler::schedules::v1;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ScheduleEventStatus {
    #[default]
    Scheduled,
    Paused,
}

impl From<ScheduleEventStatus> for v1::ScheduleStatus {
    fn from(value: ScheduleEventStatus) -> Self {
        let kind = match value {
            ScheduleEventStatus::Scheduled => v1::schedule_status::Scheduled {}.into(),
            ScheduleEventStatus::Paused => v1::schedule_status::Paused {}.into(),
        };
        Self { kind: Some(kind) }
    }
}

#[cfg(test)]
mod tests;
