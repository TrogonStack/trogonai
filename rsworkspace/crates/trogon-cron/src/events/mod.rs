use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use trogon_eventsourcing::{EventCodec, EventData, EventType, RecordedEvent, StreamEvent};

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
        #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
        headers: BTreeMap<String, String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        ttl_sec: Option<u64>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        source: Option<JobEventSamplingSource>,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct RegisteredJobSpec {
    #[serde(default)]
    pub state: JobEventState,
    pub schedule: JobEventSchedule,
    pub delivery: JobEventDelivery,
    pub payload: serde_json::Value,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub metadata: BTreeMap<String, String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum JobEvent {
    JobRegistered { id: String, spec: RegisteredJobSpec },
    JobPaused { id: String },
    JobResumed { id: String },
    JobRemoved { id: String },
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
            Self::JobRegistered { id, .. } => id,
            Self::JobPaused { id } => id,
            Self::JobResumed { id } => id,
            Self::JobRemoved { id } => id,
        }
    }
}

impl EventType for JobEvent {
    fn event_type(&self) -> &'static str {
        match self {
            Self::JobRegistered { .. } => "job_registered",
            Self::JobPaused { .. } => "job_paused",
            Self::JobResumed { .. } => "job_resumed",
            Self::JobRemoved { .. } => "job_removed",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn event_data_and_recorded_event_helpers_work() {
        let event = JobEventData::new_with_codec(
            &JobEventCodec,
            JobEvent::JobRemoved {
                id: "cleanup".to_string(),
            },
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
            JobEvent::JobRemoved {
                id: "cleanup".to_string()
            }
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
            JobEvent::JobRemoved {
                id: "cleanup".to_string()
            }
        );
    }

    #[test]
    fn invalid_payload_fails_decode() {
        assert!(JobEventData::decode(br#"not-json"#).is_err());
        assert!(RecordedJobEvent::decode(br#"not-json"#).is_err());
    }

    #[test]
    fn registered_job_spec_round_trips_without_id() {
        let spec = RegisteredJobSpec {
            state: JobEventState::Enabled,
            schedule: JobEventSchedule::Every { every_sec: 30 },
            delivery: JobEventDelivery::NatsEvent {
                route: "agent.run".to_string(),
                headers: BTreeMap::new(),
                ttl_sec: None,
                source: Some(JobEventSamplingSource::LatestFromSubject {
                    subject: "jobs.latest".to_string(),
                }),
            },
            payload: serde_json::json!({"kind": "heartbeat"}),
            metadata: BTreeMap::from([("owner".to_string(), "ops".to_string())]),
        };

        let json = serde_json::to_string(&spec).unwrap();
        let decoded: RegisteredJobSpec = serde_json::from_str(&json).unwrap();

        assert_eq!(decoded, spec);
        assert!(!json.contains("\"id\""));
    }
}
