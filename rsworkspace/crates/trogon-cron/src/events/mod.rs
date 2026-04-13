use serde::{Deserialize, Serialize};
use trogon_eventsourcing::{EventData, EventType, RecordedEvent, StreamEvent, SubjectEvent};

use crate::{
    config::{JobEnabledState, JobSpec},
    kv::EVENTS_SUBJECT_PREFIX,
};

mod state;

pub use state::{JobStreamState, JobTransitionError, apply, initial_state, projection_change};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum JobEvent {
    JobRegistered { spec: JobSpec },
    JobStateChanged { id: String, state: JobEnabledState },
    JobRemoved { id: String },
}

pub type JobEventData = EventData<JobEvent>;
pub type RecordedJobEvent = RecordedEvent<JobEvent>;

#[derive(Debug, Clone, PartialEq)]
pub enum ProjectionChange {
    Upsert(JobSpec),
    Delete(String),
}

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
    use std::collections::BTreeMap;

    use super::*;
    use crate::{
        JobId,
        config::{DeliverySpec, ScheduleSpec},
    };

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
        let mut state = initial_state(JobId::parse("backup").unwrap());

        for event in events {
            state = apply(state, event).unwrap();
        }

        assert_eq!(state, JobStreamState::Present(job("backup")));
    }

    #[test]
    fn state_change_requires_existing_job() {
        let error = apply(
            initial_state(JobId::parse("missing").unwrap()),
            JobEvent::job_state_changed("missing", JobEnabledState::Disabled),
        )
        .unwrap_err();

        assert!(matches!(
            error,
            JobTransitionError::MissingJobForStateChange { .. }
        ));
    }

    #[test]
    fn projection_change_tracks_latest_state() {
        let before = initial_state(JobId::parse("backup").unwrap());
        let after = apply(before.clone(), JobEvent::job_registered(job("backup"))).unwrap();
        assert_eq!(
            projection_change(&before, &after),
            Some(ProjectionChange::Upsert(job("backup")))
        );

        let updated = apply(
            after.clone(),
            JobEvent::job_state_changed("backup", JobEnabledState::Disabled),
        )
        .unwrap();
        match projection_change(&after, &updated).unwrap() {
            ProjectionChange::Upsert(job) => assert_eq!(job.state, JobEnabledState::Disabled),
            ProjectionChange::Delete(_) => panic!("expected upsert change"),
        }
    }

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

    #[test]
    fn initial_state_rejects_registering_existing_job() {
        let error = apply(
            JobStreamState::Present(job("backup")),
            JobEvent::job_registered(job("backup")),
        )
        .unwrap_err();
        assert!(matches!(
            error,
            JobTransitionError::CannotRegisterExistingJob { .. }
        ));
    }

    #[test]
    fn initial_state_rejects_missing_removal() {
        let error = apply(
            initial_state(JobId::parse("backup").unwrap()),
            JobEvent::job_removed("backup"),
        )
        .unwrap_err();
        assert!(matches!(
            error,
            JobTransitionError::MissingJobForRemoval { .. }
        ));
    }

    #[test]
    fn reducer_rejects_stream_id_mismatch() {
        let error = apply(
            initial_state(JobId::parse("backup").unwrap()),
            JobEvent::job_removed("other"),
        )
        .unwrap_err();
        assert!(matches!(error, JobTransitionError::StreamIdMismatch { .. }));
    }
}
