use crate::{
    JobId, JobIdError,
    config::{JobSpec, VersionedJobSpec},
};

use super::{JobEvent, ProjectionChange};

#[allow(clippy::large_enum_variant)]
#[derive(Debug, Clone, PartialEq)]
pub enum JobStreamState {
    Initial { id: JobId },
    Present(JobSpec),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum JobTransitionError {
    StreamIdMismatch { state_id: JobId, event_id: String },
    CannotRegisterExistingJob { id: JobId },
    MissingJobForStateChange { id: JobId },
    MissingJobForRemoval { id: JobId },
}

pub fn initial_state(id: JobId) -> JobStreamState {
    JobStreamState::Initial { id }
}

pub fn apply(state: JobStreamState, event: JobEvent) -> Result<JobStreamState, JobTransitionError> {
    let state_id = state.stream_id();
    let event_id = event.job_id().to_string();
    if state_id.as_str() != event_id {
        return Err(JobTransitionError::StreamIdMismatch { state_id, event_id });
    }

    match (state, event) {
        (JobStreamState::Initial { .. }, JobEvent::JobRegistered { spec }) => {
            Ok(JobStreamState::Present(spec))
        }
        (JobStreamState::Initial { .. }, JobEvent::JobStateChanged { .. }) => {
            Err(JobTransitionError::MissingJobForStateChange { id: state_id })
        }
        (JobStreamState::Initial { .. }, JobEvent::JobRemoved { .. }) => {
            Err(JobTransitionError::MissingJobForRemoval { id: state_id })
        }
        (JobStreamState::Present(_), JobEvent::JobRegistered { .. }) => {
            Err(JobTransitionError::CannotRegisterExistingJob { id: state_id })
        }
        (JobStreamState::Present(mut spec), JobEvent::JobStateChanged { state, .. }) => {
            spec.state = state;
            Ok(JobStreamState::Present(spec))
        }
        (JobStreamState::Present(_), JobEvent::JobRemoved { .. }) => Ok(initial_state(state_id)),
    }
}

pub fn projection_change(
    before: &JobStreamState,
    after: &JobStreamState,
) -> Option<ProjectionChange> {
    match (before, after) {
        (JobStreamState::Initial { .. }, JobStreamState::Initial { .. }) => None,
        (_, JobStreamState::Present(spec)) => Some(ProjectionChange::Upsert(spec.clone())),
        (JobStreamState::Present(spec), JobStreamState::Initial { .. }) => {
            Some(ProjectionChange::Delete(spec.id.clone()))
        }
    }
}

impl JobStreamState {
    pub fn stream_id(&self) -> JobId {
        match self {
            Self::Initial { id } => id.clone(),
            Self::Present(spec) => {
                JobId::parse(&spec.id).expect("present job state must carry a valid job id")
            }
        }
    }

    pub fn into_spec(self) -> Option<JobSpec> {
        match self {
            Self::Initial { .. } => None,
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
            Self::StreamIdMismatch { state_id, event_id } => write!(
                f,
                "event stream id '{event_id}' does not match state stream id '{state_id}'"
            ),
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

impl std::error::Error for JobTransitionError {}
