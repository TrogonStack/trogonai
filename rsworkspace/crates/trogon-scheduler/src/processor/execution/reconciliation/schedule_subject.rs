use trogon_nats::DottedNatsToken;

use super::ScheduleKey;

const EXECUTION_SUBJECT_PREFIX: &str = "scheduler.schedules.execution.v1";
pub(crate) const EVENT_SUBJECT_PREFIX: &str = "scheduler.schedules.events.v1";
/// Namespace reserved for scheduler-internal sentinel routes (e.g. the
/// corrupt-checkpoint placeholder). Reserving it at request validation keeps
/// sentinel routes unclaimable by user schedules, so sentinel detection can
/// never misclassify a real schedule.
pub(crate) const SCHEDULER_INTERNAL_PREFIX: &str = "trogon.scheduler";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScheduleSubject {
    token: DottedNatsToken,
    key: ScheduleKey,
}

impl ScheduleSubject {
    pub fn execution(key: &ScheduleKey) -> Self {
        Self::with_prefix(EXECUTION_SUBJECT_PREFIX, key)
    }

    pub fn event(key: &ScheduleKey) -> Self {
        Self::with_prefix(EVENT_SUBJECT_PREFIX, key)
    }

    fn with_prefix(prefix: &str, key: &ScheduleKey) -> Self {
        let subject = format!("{prefix}.{}", key.simple());
        let token = DottedNatsToken::new(&subject)
            .expect("a subject built from a fixed prefix and a uuid-simple key is a valid dotted token");
        Self { token, key: *key }
    }

    pub fn as_str(&self) -> &str {
        self.token.as_str()
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::commands::domain::ScheduleId;

    fn key(raw: &str) -> ScheduleKey {
        ScheduleKey::derive(&ScheduleId::parse(raw).unwrap())
    }

    #[test]
    fn execution_subject_uses_the_execution_prefix_and_key() {
        let key = key("orders/created");
        let subject = ScheduleSubject::execution(&key);

        assert_eq!(
            subject.as_str(),
            format!("scheduler.schedules.execution.v1.{}", key.simple())
        );
    }

    #[test]
    fn event_subject_uses_the_event_prefix_and_key() {
        let key = key("orders/created");
        let subject = ScheduleSubject::event(&key);

        assert_eq!(
            subject.as_str(),
            format!("scheduler.schedules.events.v1.{}", key.simple())
        );
    }

    #[test]
    fn subjects_are_deterministic_for_a_given_key() {
        let key = key("ns:thing");

        assert_eq!(ScheduleSubject::execution(&key), ScheduleSubject::execution(&key));
    }
}
