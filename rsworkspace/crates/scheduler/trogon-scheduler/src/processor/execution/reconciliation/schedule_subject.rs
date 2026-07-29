use super::ScheduleKey;
use crate::constants::{
    EVENT_SUBJECT_PREFIX, EXECUTION_SUBJECT_PREFIX, RRULE_WAKEUP_SUBJECT_PREFIX, SCHEDULER_INTERNAL_PREFIX,
};

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
