use serde::{Deserialize, Serialize};

use super::{JobAdded, JobPaused, JobRemoved, JobResumed};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum JobEvent {
    JobAdded(JobAdded),
    JobPaused(JobPaused),
    JobResumed(JobResumed),
    JobRemoved(JobRemoved),
}

impl JobEvent {
    pub fn stream_id(&self) -> &str {
        match self {
            Self::JobAdded(event) => &event.id,
            Self::JobPaused(event) => &event.id,
            Self::JobResumed(event) => &event.id,
            Self::JobRemoved(event) => &event.id,
        }
    }
}

impl From<JobAdded> for JobEvent {
    fn from(value: JobAdded) -> Self {
        Self::JobAdded(value)
    }
}

impl From<JobPaused> for JobEvent {
    fn from(value: JobPaused) -> Self {
        Self::JobPaused(value)
    }
}

impl From<JobResumed> for JobEvent {
    fn from(value: JobResumed) -> Self {
        Self::JobResumed(value)
    }
}

impl From<JobRemoved> for JobEvent {
    fn from(value: JobRemoved) -> Self {
        Self::JobRemoved(value)
    }
}
