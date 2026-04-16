use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use trogon_eventsourcing::{EventData, EventType, RecordedEvent, StreamEvent};

use crate::{
    JobId,
    config::{DeliverySpec, JobEnabledState, JobSpec, ScheduleSpec},
};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct RegisteredJobSpec {
    #[serde(default)]
    pub state: JobEnabledState,
    pub schedule: ScheduleSpec,
    pub delivery: DeliverySpec,
    pub payload: serde_json::Value,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub metadata: BTreeMap<String, String>,
}

impl RegisteredJobSpec {
    pub fn into_job_spec(self, id: JobId) -> JobSpec {
        JobSpec {
            id,
            state: self.state,
            schedule: self.schedule,
            delivery: self.delivery,
            payload: self.payload,
            metadata: self.metadata,
        }
    }
}

impl From<JobSpec> for RegisteredJobSpec {
    fn from(spec: JobSpec) -> Self {
        Self {
            state: spec.state,
            schedule: spec.schedule,
            delivery: spec.delivery,
            payload: spec.payload,
            metadata: spec.metadata,
        }
    }
}

impl From<&JobSpec> for RegisteredJobSpec {
    fn from(spec: &JobSpec) -> Self {
        Self {
            state: spec.state,
            schedule: spec.schedule.clone(),
            delivery: spec.delivery.clone(),
            payload: spec.payload.clone(),
            metadata: spec.metadata.clone(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum JobEvent {
    JobRegistered { id: String, spec: RegisteredJobSpec },
    JobStateChanged { id: String, state: JobEnabledState },
    JobRemoved { id: String },
}

pub type JobEventData = EventData;
pub type RecordedJobEvent = RecordedEvent;

impl StreamEvent for JobEvent {
    fn stream_id(&self) -> &str {
        match self {
            Self::JobRegistered { id, .. } => id,
            Self::JobStateChanged { id, .. } => id,
            Self::JobRemoved { id } => id,
        }
    }
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::SamplingSource;

    #[test]
    fn event_data_and_recorded_event_helpers_work() {
        let event = JobEventData::new(JobEvent::JobRemoved {
            id: "cleanup".to_string(),
        })
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
            decoded.decode_data::<JobEvent>().unwrap(),
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
            decoded.decode_data::<JobEvent>().unwrap(),
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
            state: JobEnabledState::Enabled,
            schedule: ScheduleSpec::Every { every_sec: 30 },
            delivery: DeliverySpec::NatsEvent {
                route: "agent.run".to_string(),
                headers: BTreeMap::new(),
                ttl_sec: None,
                source: Some(SamplingSource::LatestFromSubject {
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
