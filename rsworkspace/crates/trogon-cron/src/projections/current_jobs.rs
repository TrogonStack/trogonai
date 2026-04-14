use crate::{JobId, JobIdError, config::JobSpec, events::JobEvent};
use trogon_eventsourcing::Snapshot;

#[derive(Debug, Clone, PartialEq)]
pub enum ProjectionChange {
    Upsert(JobSpec),
    Delete(String),
}

#[allow(clippy::large_enum_variant)]
#[derive(Debug, Clone, PartialEq)]
pub enum JobStreamState {
    Initial,
    Present(JobSpec),
}

#[derive(Debug)]
pub enum JobTransitionError {
    InvalidEventId { id: String, source: JobIdError },
    CannotRegisterExistingJob { id: JobId },
    MissingJobForStateChange { id: JobId },
    MissingJobForRemoval { id: JobId },
}

pub const fn initial_state() -> JobStreamState {
    JobStreamState::Initial
}

pub fn apply(state: JobStreamState, event: JobEvent) -> Result<JobStreamState, JobTransitionError> {
    match (state, event) {
        (JobStreamState::Initial, JobEvent::JobRegistered { spec }) => {
            Ok(JobStreamState::Present(spec))
        }
        (JobStreamState::Initial, event @ JobEvent::JobStateChanged { .. }) => {
            Err(JobTransitionError::MissingJobForStateChange {
                id: parse_event_job_id(&event)?,
            })
        }
        (JobStreamState::Initial, event @ JobEvent::JobRemoved { .. }) => {
            Err(JobTransitionError::MissingJobForRemoval {
                id: parse_event_job_id(&event)?,
            })
        }
        (JobStreamState::Present(spec), JobEvent::JobRegistered { .. }) => {
            Err(JobTransitionError::CannotRegisterExistingJob {
                id: JobId::parse(&spec.id).expect("present job state must carry a valid job id"),
            })
        }
        (JobStreamState::Present(mut spec), JobEvent::JobStateChanged { state, .. }) => {
            spec.state = state;
            Ok(JobStreamState::Present(spec))
        }
        (JobStreamState::Present(_), JobEvent::JobRemoved { .. }) => Ok(initial_state()),
    }
}

pub fn projection_change(
    before: &JobStreamState,
    after: &JobStreamState,
) -> Option<ProjectionChange> {
    match (before, after) {
        (JobStreamState::Initial, JobStreamState::Initial) => None,
        (_, JobStreamState::Present(spec)) => Some(ProjectionChange::Upsert(spec.clone())),
        (JobStreamState::Present(spec), JobStreamState::Initial) => {
            Some(ProjectionChange::Delete(spec.id.clone()))
        }
    }
}

impl JobStreamState {
    pub fn into_spec(self) -> Option<JobSpec> {
        match self {
            Self::Initial => None,
            Self::Present(spec) => Some(spec),
        }
    }

    pub fn into_snapshot(self, version: u64) -> Option<Snapshot<JobSpec>> {
        self.into_spec().map(|spec| Snapshot::new(version, spec))
    }
}

impl TryFrom<JobSpec> for JobStreamState {
    type Error = JobIdError;

    fn try_from(spec: JobSpec) -> Result<Self, Self::Error> {
        JobId::parse(&spec.id)?;
        Ok(Self::Present(spec))
    }
}

impl TryFrom<Snapshot<JobSpec>> for JobStreamState {
    type Error = JobIdError;

    fn try_from(value: Snapshot<JobSpec>) -> Result<Self, Self::Error> {
        Self::try_from(value.payload)
    }
}

impl std::fmt::Display for JobTransitionError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::InvalidEventId { id, .. } => write!(f, "job event id '{id}' is invalid"),
            Self::CannotRegisterExistingJob { id } => {
                write!(f, "job '{id}' is already registered")
            }
            Self::MissingJobForStateChange { id } => {
                write!(f, "missing job for state change '{id}'")
            }
            Self::MissingJobForRemoval { id } => {
                write!(f, "missing job for removal '{id}'")
            }
        }
    }
}

impl std::error::Error for JobTransitionError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::InvalidEventId { source, .. } => Some(source),
            _ => None,
        }
    }
}

fn parse_event_job_id(event: &JobEvent) -> Result<JobId, JobTransitionError> {
    let id = event.job_id().to_string();
    JobId::parse(&id).map_err(|source| JobTransitionError::InvalidEventId { id, source })
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use super::*;
    use crate::{
        config::{DeliverySpec, JobEnabledState, ScheduleSpec},
        events::JobEvent,
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
        let mut state = initial_state();

        for event in events {
            state = apply(state, event).unwrap();
        }

        assert_eq!(state, JobStreamState::Present(job("backup")));
    }

    #[test]
    fn state_change_requires_existing_job() {
        let error = apply(
            initial_state(),
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
        let before = initial_state();
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
        let error = apply(initial_state(), JobEvent::job_removed("backup")).unwrap_err();
        assert!(matches!(
            error,
            JobTransitionError::MissingJobForRemoval { .. }
        ));
    }

    #[test]
    fn reducer_rejects_stream_id_mismatch() {
        let error = apply(initial_state(), JobEvent::job_removed("other")).unwrap_err();
        assert!(matches!(
            error,
            JobTransitionError::MissingJobForRemoval { .. }
        ));
    }
}
