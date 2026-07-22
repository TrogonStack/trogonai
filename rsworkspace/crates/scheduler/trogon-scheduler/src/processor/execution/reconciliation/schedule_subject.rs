use super::ScheduleKey;

const EXECUTION_SUBJECT_PREFIX: &str = "scheduler.schedules.execution.v1";
pub(crate) const EVENT_SUBJECT_PREFIX: &str = "scheduler.schedules.events.v1";
pub(crate) const RRULE_WAKEUP_SUBJECT_PREFIX: &str = "scheduler.schedules.execution.v1.rrule";
/// Namespace reserved for scheduler-internal sentinel routes (e.g. the
/// corrupt-checkpoint placeholder). Reserving it at request validation keeps
/// sentinel routes unclaimable by user schedules, so sentinel detection can
/// never misclassify a real schedule.
pub(crate) const SCHEDULER_INTERNAL_PREFIX: &str = "trogon.scheduler";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScheduleSubject {
    subject: String,
    key: ScheduleKey,
}

impl ScheduleSubject {
    pub fn execution(key: &ScheduleKey) -> Self {
        Self::with_prefix(EXECUTION_SUBJECT_PREFIX, key)
    }

    pub fn rrule_wakeup(key: &ScheduleKey) -> Self {
        Self::with_prefix(RRULE_WAKEUP_SUBJECT_PREFIX, key)
    }

    pub fn event(key: &ScheduleKey) -> Self {
        Self::with_prefix(EVENT_SUBJECT_PREFIX, key)
    }

    fn with_prefix(prefix: &str, key: &ScheduleKey) -> Self {
        let subject = format!("{prefix}.{}", key.simple());
        Self { subject, key: *key }
    }

    pub fn as_str(&self) -> &str {
        &self.subject
    }

    /// The schedule key this subject was derived from.
    pub fn key(&self) -> &ScheduleKey {
        &self.key
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
