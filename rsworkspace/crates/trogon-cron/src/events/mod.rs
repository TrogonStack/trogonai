use serde::{Deserialize, Serialize};
use trogon_eventsourcing::{EventData, EventType, RecordedEvent, StreamEvent, SubjectEvent};

use crate::{
    config::{JobEnabledState, JobSpec},
    kv::EVENTS_SUBJECT_PREFIX,
};

mod decision;

pub use decision::JobDecisionError;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum JobEvent {
    JobRegistered { spec: JobSpec },
    JobStateChanged { id: String, state: JobEnabledState },
    JobRemoved { id: String },
}

pub type JobEventData = EventData<JobEvent>;
pub type RecordedJobEvent = RecordedEvent<JobEvent>;

impl StreamEvent for JobEvent {
    fn stream_id(&self) -> &str {
        self.job_id()
    }
}

impl SubjectEvent for JobEvent {
    const SUBJECT_PREFIX: &'static str = EVENTS_SUBJECT_PREFIX;
}

impl EventType for JobEvent {
    fn event_type(&self) -> &'static str {
        match self {
            Self::JobRegistered { .. } => "job_registered",
            Self::JobStateChanged { .. } => "job_state_changed",
            Self::JobRemoved { .. } => "job_removed",
        }
    }
}

impl JobEvent {
    pub fn job_registered(spec: JobSpec) -> Self {
        Self::JobRegistered { spec }
    }

    pub fn job_state_changed(id: impl Into<String>, state: JobEnabledState) -> Self {
        Self::JobStateChanged {
            id: id.into(),
            state,
        }
    }

    pub fn job_removed(id: impl Into<String>) -> Self {
        Self::JobRemoved { id: id.into() }
    }

    pub fn job_id(&self) -> &str {
        match self {
            Self::JobRegistered { spec } => &spec.id,
            Self::JobStateChanged { id, .. } => id,
            Self::JobRemoved { id } => id,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn event_data_and_recorded_event_helpers_work() {
        let event = JobEventData::new(JobEvent::job_removed("cleanup"));
        assert_eq!(event.stream_id(), "cleanup");
        assert_eq!(event.event_type, "job_removed");
        assert_eq!(event.subject(), "cron.jobs.events.cleanup");
        assert_eq!(
            event.subject_with_prefix("cron.events.jobs."),
            "cron.events.jobs.cleanup"
        );

        let payload = serde_json::to_vec(&event).unwrap();
        let decoded = JobEventData::decode(&payload).unwrap();
        assert_eq!(decoded, event);

        let recorded = event.record(
            "cron.jobs.events.cleanup",
            None,
            Some(9),
            chrono::DateTime::<chrono::Utc>::from_timestamp(1_700_000_000, 0).unwrap(),
        );
        assert_eq!(recorded.stream_id(), "cleanup");
        assert_eq!(recorded.stream_id, "cron.jobs.events.cleanup");
        assert_eq!(recorded.log_position, Some(9));
        assert_eq!(recorded.subject(), "cron.jobs.events.cleanup");
        let recorded_payload = serde_json::to_vec(&recorded).unwrap();
        let decoded = RecordedJobEvent::decode(&recorded_payload).unwrap();
        assert_eq!(decoded, recorded);
    }

    #[test]
    fn invalid_payload_fails_decode() {
        assert!(JobEventData::decode(br#"not-json"#).is_err());
        assert!(RecordedJobEvent::decode(br#"not-json"#).is_err());
    }
}
