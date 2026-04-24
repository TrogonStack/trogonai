use serde::{Deserialize, Serialize};
use trogon_eventsourcing::{CanonicalEventCodec, EventCodec, EventData, EventType, RecordedEvent};

use super::{JobAdded, JobPaused, JobRemoved, JobResumed};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum JobEvent {
    JobAdded(JobAdded),
    JobPaused(JobPaused),
    JobResumed(JobResumed),
    JobRemoved(JobRemoved),
}

pub type JobEventData = EventData;
pub type RecordedJobEvent = RecordedEvent;

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct JobEventCodec;

impl EventCodec<JobEvent> for JobEventCodec {
    type Error = serde_json::Error;

    fn encode(&self, value: &JobEvent) -> Result<String, Self::Error> {
        serde_json::to_string(value)
    }

    fn decode(&self, value: &str) -> Result<JobEvent, Self::Error> {
        serde_json::from_str(value)
    }
}

impl JobEvent {
    pub fn stream_id(&self) -> &str {
        match self {
            Self::JobAdded(event) => &event.id,
            Self::JobPaused(event) => &event.id,
            Self::JobResumed(event) => &event.id,
            Self::JobRemoved(event) => &event.id,
        }
    }
}

impl EventType for JobEvent {
    fn event_type(&self) -> &'static str {
        match self {
            Self::JobAdded(event) => event.event_type(),
            Self::JobPaused(event) => event.event_type(),
            Self::JobResumed(event) => event.event_type(),
            Self::JobRemoved(event) => event.event_type(),
        }
    }
}

impl CanonicalEventCodec for JobEvent {
    type Codec = JobEventCodec;

    fn canonical_codec() -> Self::Codec {
        JobEventCodec
    }
}

impl From<JobAdded> for JobEvent {
    fn from(value: JobAdded) -> Self {
        Self::JobAdded(value)
    }
}

impl From<JobPaused> for JobEvent {
    fn from(value: JobPaused) -> Self {
        Self::JobPaused(value)
    }
}

impl From<JobResumed> for JobEvent {
    fn from(value: JobResumed) -> Self {
        Self::JobResumed(value)
    }
}

impl From<JobRemoved> for JobEvent {
    fn from(value: JobRemoved) -> Self {
        Self::JobRemoved(value)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn event_data_and_recorded_event_helpers_work() {
        let event = JobEventData::new_with_codec(
            "cleanup",
            &JobEventCodec,
            JobEvent::JobRemoved(JobRemoved {
                id: "cleanup".to_string(),
            }),
        )
        .unwrap();
        assert_eq!(event.stream_id(), "cleanup");
        assert_eq!(event.event_type, "job_removed");
        assert_eq!(
            event.subject_with_prefix("cron.events.jobs."),
            "cron.events.jobs.cleanup"
        );

        let payload = serde_json::to_vec(&event).unwrap();
        let decoded = JobEventData::decode(&payload).unwrap();
        assert_eq!(decoded, event);
        assert_eq!(
            decoded.decode_data_with(&JobEventCodec).unwrap(),
            JobEvent::JobRemoved(JobRemoved {
                id: "cleanup".to_string()
            })
        );

        let recorded = event.record(
            "cron.jobs.events.cleanup",
            None,
            Some(9),
            chrono::DateTime::<chrono::Utc>::from_timestamp(1_700_000_000, 0).unwrap(),
        );
        assert_eq!(recorded.stream_id(), "cleanup");
        assert_eq!(recorded.recorded_stream_id, "cron.jobs.events.cleanup");
        assert_eq!(recorded.log_position, Some(9));
        assert_eq!(
            recorded.subject_with_prefix("cron.events.jobs."),
            "cron.events.jobs.cleanup"
        );
        let recorded_payload = serde_json::to_vec(&recorded).unwrap();
        let decoded = RecordedJobEvent::decode(&recorded_payload).unwrap();
        assert_eq!(decoded, recorded);
        assert_eq!(
            decoded.decode_data_with(&JobEventCodec).unwrap(),
            JobEvent::JobRemoved(JobRemoved {
                id: "cleanup".to_string()
            })
        );
    }

    #[test]
    fn invalid_payload_fails_decode() {
        assert!(JobEventData::decode(br#"not-json"#).is_err());
        assert!(RecordedJobEvent::decode(br#"not-json"#).is_err());
    }
}
