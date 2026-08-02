use crate::commands::domain::ScheduleId;
use crate::constants::{
    EVENT_SUBJECT_PREFIX, EXECUTION_SUBJECT_PREFIX, RRULE_WAKEUP_SUBJECT_PREFIX, SCHEDULER_INTERNAL_PREFIX,
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScheduleSubject {
    subject: String,
    schedule_id: ScheduleId,
}

impl ScheduleSubject {
    pub fn execution(schedule_id: &ScheduleId) -> Self {
        Self::with_prefix(EXECUTION_SUBJECT_PREFIX, schedule_id)
    }

    pub fn rrule_wakeup(schedule_id: &ScheduleId) -> Self {
        Self::with_prefix(RRULE_WAKEUP_SUBJECT_PREFIX, schedule_id)
    }

    pub fn event(schedule_id: &ScheduleId) -> Self {
        Self::with_prefix(EVENT_SUBJECT_PREFIX, schedule_id)
    }

    fn with_prefix(prefix: &str, schedule_id: &ScheduleId) -> Self {
        let subject = format!("{prefix}.{schedule_id}");
        Self {
            subject,
            schedule_id: schedule_id.clone(),
        }
    }

    pub fn as_str(&self) -> &str {
        &self.subject
    }

    pub fn schedule_id(&self) -> &ScheduleId {
        &self.schedule_id
    }

    /// Reports whether `subject` falls inside a scheduler-owned namespace
    /// (execution or event subjects, or the reserved internal namespace), at
    /// any token depth.
    pub fn is_scheduler_internal(subject: &str) -> bool {
        [
            EXECUTION_SUBJECT_PREFIX,
            EVENT_SUBJECT_PREFIX,
            SCHEDULER_INTERNAL_PREFIX,
        ]
        .iter()
        .any(|prefix| subject == *prefix || subject.strip_prefix(prefix).is_some_and(|rest| rest.starts_with('.')))
    }
}

impl std::fmt::Display for ScheduleSubject {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

#[cfg(test)]
mod tests;
