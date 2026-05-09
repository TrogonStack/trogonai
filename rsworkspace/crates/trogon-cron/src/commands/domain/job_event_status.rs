use trogon_cron_jobs_proto::v1;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum JobEventStatus {
    #[default]
    Enabled,
    Disabled,
}

impl From<JobEventStatus> for v1::JobStatus {
    fn from(value: JobEventStatus) -> Self {
        match value {
            JobEventStatus::Enabled => Self::JOB_STATUS_ENABLED,
            JobEventStatus::Disabled => Self::JOB_STATUS_DISABLED,
        }
    }
}
