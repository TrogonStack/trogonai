use std::convert::Infallible;

use serde::{Deserialize, Serialize};
use trogon_eventsourcing::{StateMachine, snapshot::SnapshotSchema};

use crate::events::{JobAdded, JobEvent, JobEventState, JobPaused, JobRemoved, JobResumed};

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
                } else if matches!(job.state, JobEventState::Enabled) {
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
