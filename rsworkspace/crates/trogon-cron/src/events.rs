use std::collections::BTreeMap;

use async_nats::jetstream::message::StreamMessage;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::{
    config::{JobEnabledState, JobSpec, VersionedJobSpec},
    error::CronError,
    kv::EVENTS_SUBJECT_PREFIX,
};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct RecordedJobEvent {
    pub event_id: String,
    pub occurred_at: DateTime<Utc>,
    #[serde(flatten)]
    pub event: JobEvent,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum JobEvent {
    JobRegistered { spec: JobSpec },
    JobStateChanged { id: String, state: JobEnabledState },
    JobRemoved { id: String },
}

#[derive(Debug, Clone, PartialEq)]
pub enum ProjectionChange {
    Upsert(JobSpec),
    Delete(String),
}

impl RecordedJobEvent {
    pub fn new(event: JobEvent) -> Self {
        Self {
            event_id: Uuid::new_v4().to_string(),
            occurred_at: Utc::now(),
            event,
        }
    }

    pub fn subject(&self) -> String {
        self.subject_with_prefix(EVENTS_SUBJECT_PREFIX)
    }

    pub fn subject_with_prefix(&self, prefix: &str) -> String {
        format!("{prefix}{}", self.job_id())
    }

    pub fn job_id(&self) -> &str {
        self.event.job_id()
    }

    pub fn from_stream_message(message: StreamMessage) -> Result<Self, CronError> {
        serde_json::from_slice::<RecordedJobEvent>(&message.payload)
            .map_err(|source| CronError::event_source("failed to decode stored job event", source))
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
            Self::JobStateChanged { id, .. } | Self::JobRemoved { id } => id,
        }
    }

    pub fn apply_to_state(
        &self,
        jobs: &mut BTreeMap<String, JobSpec>,
    ) -> Result<ProjectionChange, CronError> {
        match self {
            Self::JobRegistered { spec } => {
                jobs.insert(spec.id.clone(), spec.clone());
                Ok(ProjectionChange::Upsert(spec.clone()))
            }
            Self::JobStateChanged { id, state } => {
                let spec = jobs.get_mut(id).ok_or_else(|| {
                    projection_invariant_error("missing job for state change", id)
                })?;
                spec.state = *state;
                Ok(ProjectionChange::Upsert(spec.clone()))
            }
            Self::JobRemoved { id } => {
                jobs.remove(id);
                Ok(ProjectionChange::Delete(id.clone()))
            }
        }
    }
}

fn projection_invariant_error(context: &'static str, id: &str) -> CronError {
    CronError::event_source(context, std::io::Error::other(format!("job '{id}'")))
}

pub fn apply_event_to_versioned_state(
    jobs: &mut BTreeMap<String, VersionedJobSpec>,
    event: &JobEvent,
    version: u64,
) -> Result<(), CronError> {
    match event {
        JobEvent::JobRegistered { spec } => {
            jobs.insert(
                spec.id.clone(),
                VersionedJobSpec {
                    version,
                    spec: spec.clone(),
                },
            );
            Ok(())
        }
        JobEvent::JobStateChanged { id, state } => {
            let job = jobs.get_mut(id).ok_or_else(|| {
                projection_invariant_error("missing versioned job for state change", id)
            })?;
            job.version = version;
            job.spec.state = *state;
            Ok(())
        }
        JobEvent::JobRemoved { id } => {
            jobs.remove(id);
            Ok(())
        }
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use super::*;
    use crate::config::{DeliverySpec, ScheduleSpec};
    use async_nats::HeaderMap;

    fn job(id: &str) -> JobSpec {
        JobSpec {
            id: id.to_string(),
            state: JobEnabledState::Enabled,
            schedule: ScheduleSpec::Every { every_sec: 30 },
            delivery: DeliverySpec::NatsEvent {
                route: "agent.run".to_string(),
                headers: BTreeMap::new(),
                ttl_sec: None,
                source: None,
            },
            payload: serde_json::json!({"kind": "heartbeat"}),
            metadata: BTreeMap::new(),
        }
    }

    #[test]
    fn event_projection_replays_latest_state() {
        let events = [
            JobEvent::job_registered(job("backup")),
            JobEvent::job_state_changed("backup", JobEnabledState::Disabled),
            JobEvent::job_removed("backup"),
            JobEvent::job_registered(job("backup")),
        ];
        let mut jobs = BTreeMap::new();

        for event in events {
            event.apply_to_state(&mut jobs).unwrap();
        }

        assert_eq!(jobs.into_values().collect::<Vec<_>>(), vec![job("backup")]);
    }

    #[test]
    fn state_change_requires_existing_job() {
        let mut jobs = BTreeMap::new();
        let error = JobEvent::job_state_changed("missing", JobEnabledState::Disabled)
            .apply_to_state(&mut jobs)
            .unwrap_err();

        assert!(error.to_string().contains("missing job for state change"));
    }

    #[test]
    fn versioned_projection_tracks_latest_sequence() {
        let mut jobs = BTreeMap::new();
        apply_event_to_versioned_state(&mut jobs, &JobEvent::job_registered(job("backup")), 4)
            .unwrap();
        apply_event_to_versioned_state(
            &mut jobs,
            &JobEvent::job_state_changed("backup", JobEnabledState::Disabled),
            5,
        )
        .unwrap();

        let projected = jobs.get("backup").unwrap();
        assert_eq!(projected.version, 5);
        assert_eq!(projected.spec.state, JobEnabledState::Disabled);
    }

    #[test]
    fn recorded_event_helpers_and_stream_decode_work() {
        let recorded = RecordedJobEvent::new(JobEvent::job_removed("cleanup"));
        assert_eq!(recorded.job_id(), "cleanup");
        assert_eq!(recorded.subject(), "cron.jobs.events.cleanup");
        assert_eq!(
            recorded.subject_with_prefix("cron.events.jobs."),
            "cron.events.jobs.cleanup"
        );

        let payload = serde_json::to_vec(&recorded).unwrap();
        let decoded = RecordedJobEvent::from_stream_message(StreamMessage {
            subject: "cron.jobs.events.cleanup".into(),
            sequence: 9,
            headers: HeaderMap::new(),
            payload: payload.into(),
            time: time::OffsetDateTime::now_utc(),
        })
        .unwrap();
        assert_eq!(decoded, recorded);
    }

    #[test]
    fn invalid_stream_payload_reports_event_decode_error() {
        let error = RecordedJobEvent::from_stream_message(StreamMessage {
            subject: "cron.jobs.events.bad".into(),
            sequence: 10,
            headers: HeaderMap::new(),
            payload: br#"not-json"#.as_slice().into(),
            time: time::OffsetDateTime::now_utc(),
        })
        .unwrap_err();

        assert!(
            error
                .to_string()
                .contains("failed to decode stored job event")
        );
    }
}
