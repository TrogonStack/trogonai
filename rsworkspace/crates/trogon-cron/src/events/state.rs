use crate::{
    JobId, JobIdError,
    config::{JobSpec, VersionedJobSpec},
};

use super::{JobEvent, ProjectionChange};

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

    pub fn into_versioned_spec(self, version: u64) -> Option<VersionedJobSpec> {
        self.into_spec()
            .map(|spec| VersionedJobSpec { version, spec })
    }
}

impl TryFrom<JobSpec> for JobStreamState {
    type Error = JobIdError;

    fn try_from(spec: JobSpec) -> Result<Self, Self::Error> {
        JobId::parse(&spec.id)?;
        Ok(Self::Present(spec))
    }
}

impl TryFrom<VersionedJobSpec> for JobStreamState {
    type Error = JobIdError;

    fn try_from(value: VersionedJobSpec) -> Result<Self, Self::Error> {
        Self::try_from(value.spec)
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
