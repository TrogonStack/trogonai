use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use trogon_eventsourcing::{AggregateEvent, EventData, EventType, RecordedEvent, SubjectEvent};

use crate::{
    config::{JobEnabledState, JobSpec, VersionedJobSpec},
    error::CronError,
    kv::EVENTS_SUBJECT_PREFIX,
};

mod job_registered;
mod job_removed;
mod job_state_changed;

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

impl AggregateEvent for JobEvent {
    fn aggregate_id(&self) -> &str {
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
        job_registered::new(spec)
    }

    pub fn job_state_changed(id: impl Into<String>, state: JobEnabledState) -> Self {
        job_state_changed::new(id, state)
    }

    pub fn job_removed(id: impl Into<String>) -> Self {
        job_removed::new(id)
    }

    pub fn job_id(&self) -> &str {
        match self {
            Self::JobRegistered { spec } => job_registered::job_id(spec),
            Self::JobStateChanged { id, .. } => job_state_changed::job_id(id),
            Self::JobRemoved { id } => job_removed::job_id(id),
        }
    }

    pub fn apply_to_state(
        &self,
        jobs: &mut BTreeMap<String, JobSpec>,
    ) -> Result<ProjectionChange, CronError> {
        match self {
            Self::JobRegistered { spec } => job_registered::apply_to_state(spec, jobs),
            Self::JobStateChanged { id, state } => {
                job_state_changed::apply_to_state(id, *state, jobs)
            }
            Self::JobRemoved { id } => job_removed::apply_to_state(id, jobs),
        }
    }
}

pub fn apply_event_to_versioned_state(
    jobs: &mut BTreeMap<String, VersionedJobSpec>,
    event: &JobEvent,
    version: u64,
) -> Result<(), CronError> {
    match event {
        JobEvent::JobRegistered { spec } => {
            job_registered::apply_to_versioned_state(spec, jobs, version)
        }
        JobEvent::JobStateChanged { id, state } => {
            job_state_changed::apply_to_versioned_state(id, *state, jobs, version)
        }
        JobEvent::JobRemoved { id } => job_removed::apply_to_versioned_state(id, jobs),
    }
}

pub(super) fn projection_invariant_error(context: &'static str, id: &str) -> CronError {
    CronError::event_source(context, std::io::Error::other(format!("job '{id}'")))
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use super::*;
    use crate::config::{DeliverySpec, ScheduleSpec};

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
    fn event_data_and_recorded_event_helpers_work() {
        let event = JobEventData::new(JobEvent::job_removed("cleanup"));
        assert_eq!(event.aggregate_id(), "cleanup");
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
        assert_eq!(recorded.aggregate_id(), "cleanup");
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
