use serde::{Deserialize, Serialize};
use trogon_cron_jobs_proto::v1;

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
