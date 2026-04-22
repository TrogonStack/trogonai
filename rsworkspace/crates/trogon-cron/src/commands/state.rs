use std::convert::Infallible;

use serde::{Deserialize, Serialize};
use trogon_eventsourcing::snapshot::SnapshotSchema;

use crate::{
    JobEnabledState,
    events::{JobAdded, JobEvent, JobPaused, JobRemoved, JobResumed},
};

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub enum JobCommandState {
    Missing,
    Present { current: JobEnabledState },
    Deleted,
}

impl SnapshotSchema for JobCommandState {
    const SNAPSHOT_STREAM_PREFIX: &'static str = "cron.command.job.v1.";
}

impl JobCommandState {
    pub const fn initial() -> Self {
        Self::Missing
    }

    pub fn evolve(self, event: JobEvent) -> Result<Self, Infallible> {
        Ok(match event {
            JobEvent::JobAdded(JobAdded { job, .. }) => {
                if matches!(self, Self::Deleted) {
                    Self::Deleted
                } else {
                    Self::Present {
                        current: job.state.into(),
                    }
                }
            }
            JobEvent::JobPaused(JobPaused { .. }) => match self {
                Self::Deleted => Self::Deleted,
                Self::Missing | Self::Present { .. } => Self::Present {
                    current: JobEnabledState::Disabled,
                },
            },
            JobEvent::JobResumed(JobResumed { .. }) => match self {
                Self::Deleted => Self::Deleted,
                Self::Missing | Self::Present { .. } => Self::Present {
                    current: JobEnabledState::Enabled,
                },
            },
            JobEvent::JobRemoved(JobRemoved { .. }) => Self::Deleted,
        })
    }
}
