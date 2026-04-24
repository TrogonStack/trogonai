use std::convert::Infallible;

use serde::{Deserialize, Serialize};
use trogon_cron_jobs_proto::state_v1;
use trogon_eventsourcing::{StateMachine, snapshot::SnapshotSchema};

use crate::events::{JobAdded, JobEvent, JobEventStatus, JobPaused, JobRemoved, JobResumed};

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub enum JobState {
    Missing,
    PresentEnabled,
    PresentDisabled,
    Deleted,
}

impl SnapshotSchema for JobState {
    const SNAPSHOT_STREAM_PREFIX: &'static str = "cron.command.job.v2.";
}

#[derive(Debug)]
pub enum JobStateProtoError {
    UnknownStateValue { value: i32 },
}

impl std::fmt::Display for JobStateProtoError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::UnknownStateValue { value } => {
                write!(f, "protobuf state '{value}' is unknown")
            }
        }
    }
}

impl std::error::Error for JobStateProtoError {}

impl From<JobState> for state_v1::State {
    fn from(value: JobState) -> Self {
        Self::from(&value)
    }
}

impl From<&JobState> for state_v1::State {
    fn from(value: &JobState) -> Self {
        let mut state = state_v1::State::new();
        state.set_state(state_v1::StateValue::from(*value));
        state
    }
}

impl TryFrom<state_v1::State> for JobState {
    type Error = JobStateProtoError;

    fn try_from(value: state_v1::State) -> Result<Self, Self::Error> {
        value.state().try_into()
    }
}

impl From<JobState> for state_v1::StateValue {
    fn from(value: JobState) -> Self {
        match value {
            JobState::Missing => Self::Missing,
            JobState::PresentEnabled => Self::PresentEnabled,
            JobState::PresentDisabled => Self::PresentDisabled,
            JobState::Deleted => Self::Deleted,
        }
    }
}

impl TryFrom<state_v1::StateValue> for JobState {
    type Error = JobStateProtoError;

    fn try_from(value: state_v1::StateValue) -> Result<Self, Self::Error> {
        match i32::from(value) {
            1 => Ok(Self::Missing),
            2 => Ok(Self::PresentEnabled),
            3 => Ok(Self::PresentDisabled),
            4 => Ok(Self::Deleted),
            other => Err(JobStateProtoError::UnknownStateValue { value: other }),
        }
    }
}

impl StateMachine<JobEvent> for JobState {
    type EvolveError = Infallible;

    fn initial_state() -> Self {
        Self::Missing
    }

    fn evolve(self, event: JobEvent) -> Result<Self, Self::EvolveError> {
        Ok(match event {
            JobEvent::JobAdded(JobAdded { job, .. }) => {
                if matches!(self, Self::Deleted) {
                    Self::Deleted
                } else if matches!(job.status, JobEventStatus::Enabled) {
                    Self::PresentEnabled
                } else {
                    Self::PresentDisabled
                }
            }
            JobEvent::JobPaused(JobPaused { .. }) => match self {
                Self::Deleted => Self::Deleted,
                Self::Missing | Self::PresentEnabled | Self::PresentDisabled => Self::PresentDisabled,
            },
            JobEvent::JobResumed(JobResumed { .. }) => match self {
                Self::Deleted => Self::Deleted,
                Self::Missing | Self::PresentEnabled | Self::PresentDisabled => Self::PresentEnabled,
            },
            JobEvent::JobRemoved(JobRemoved { .. }) => Self::Deleted,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn state_round_trips_through_proto() {
        let state = JobState::PresentEnabled;

        let proto = state_v1::State::from(state);
        let decoded = JobState::try_from(proto).unwrap();

        assert_eq!(decoded, state);
    }

    #[test]
    fn unspecified_state_is_rejected() {
        let error = JobState::try_from(state_v1::State::new()).unwrap_err();

        assert!(matches!(error, JobStateProtoError::UnknownStateValue { value: 0 }));
    }
}
