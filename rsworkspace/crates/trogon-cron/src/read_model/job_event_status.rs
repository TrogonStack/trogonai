use serde::{Deserialize, Serialize};
use trogon_cron_jobs_proto::v1;

use crate::proto::JobEventProtoError;

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "snake_case")]
pub enum JobEventStatus {
    #[default]
    Enabled,
    Disabled,
}

impl From<JobEventStatus> for v1::JobStatus {
    fn from(value: JobEventStatus) -> Self {
        match value {
            JobEventStatus::Enabled => Self::Enabled,
            JobEventStatus::Disabled => Self::Disabled,
        }
    }
}

impl TryFrom<v1::JobStatus> for JobEventStatus {
    type Error = JobEventProtoError;

    fn try_from(value: v1::JobStatus) -> Result<Self, Self::Error> {
        match i32::from(value) {
            1 => Ok(Self::Enabled),
            2 => Ok(Self::Disabled),
            other => Err(JobEventProtoError::UnknownJobStatus { value: other }),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unknown_proto_status_is_rejected() {
        let error = JobEventStatus::try_from(v1::JobStatus::from(99)).unwrap_err();

        assert!(matches!(error, JobEventProtoError::UnknownJobStatus { value: 99 }));
    }
}
