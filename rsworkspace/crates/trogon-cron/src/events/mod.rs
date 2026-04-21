mod message;

use serde::{Deserialize, Serialize};
use trogon_eventsourcing::{
    CanonicalEventCodec, EventCodec, EventData, EventType, RecordedEvent, StreamEvent,
};

pub use message::{MessageContent, MessageHeaders, MessageHeadersError};

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "snake_case")]
pub enum JobEventState {
    #[default]
    Enabled,
    Disabled,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum JobEventSchedule {
    At {
        at: chrono::DateTime<chrono::Utc>,
    },
    Every {
        every_sec: u64,
    },
    Cron {
        expr: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        timezone: Option<String>,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum JobEventSamplingSource {
    LatestFromSubject { subject: String },
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum JobEventDelivery {
    NatsEvent {
        route: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        ttl_sec: Option<u64>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        source: Option<JobEventSamplingSource>,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct JobDetails {
    #[serde(default)]
    pub state: JobEventState,
    pub schedule: JobEventSchedule,
    pub delivery: JobEventDelivery,
    pub content: MessageContent,
    #[serde(default, skip_serializing_if = "MessageHeaders::is_empty")]
    pub headers: MessageHeaders,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct JobAdded {
    pub id: String,
    pub job: JobDetails,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct JobPaused {
    pub id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct JobResumed {
    pub id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct JobRemoved {
    pub id: String,
}

impl StreamEvent for JobAdded {
    fn stream_id(&self) -> &str {
        &self.id
    }
}

impl StreamEvent for JobPaused {
    fn stream_id(&self) -> &str {
        &self.id
    }
}

impl StreamEvent for JobResumed {
    fn stream_id(&self) -> &str {
        &self.id
    }
}

impl StreamEvent for JobRemoved {
    fn stream_id(&self) -> &str {
        &self.id
    }
}

impl EventType for JobAdded {
    fn event_type(&self) -> &'static str {
        "job_added"
    }
}

impl EventType for JobPaused {
    fn event_type(&self) -> &'static str {
        "job_paused"
    }
}

impl EventType for JobResumed {
    fn event_type(&self) -> &'static str {
        "job_resumed"
    }
}

impl EventType for JobRemoved {
    fn event_type(&self) -> &'static str {
        "job_removed"
    }
}

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

impl StreamEvent for JobEvent {
    fn stream_id(&self) -> &str {
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

    #[test]
    fn job_details_round_trip_without_id() {
        let details = JobDetails {
            state: JobEventState::Enabled,
            schedule: JobEventSchedule::Every { every_sec: 30 },
            delivery: JobEventDelivery::NatsEvent {
                route: "agent.run".to_string(),
                ttl_sec: None,
                source: Some(JobEventSamplingSource::LatestFromSubject {
                    subject: "jobs.latest".to_string(),
                }),
            },
            content: MessageContent::from_static(br#"{"kind":"heartbeat"}"#),
            headers: MessageHeaders::new([("owner", "ops")]).unwrap(),
        };

        let json = serde_json::to_string(&details).unwrap();
        let decoded: JobDetails = serde_json::from_str(&json).unwrap();

        assert_eq!(decoded, details);
        assert!(!json.contains("\"id\""));
    }
}
