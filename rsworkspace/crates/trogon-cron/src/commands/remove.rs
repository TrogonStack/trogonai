use std::fmt;

use trogon_cron::{ConfigStore, JobWriteCondition, NatsConfigStore};

use super::job_id::{JobId, JobIdError};

#[derive(Debug)]
pub enum CommandError {
    InvalidJobId(JobIdError),
    GetJob(trogon_cron::CronError),
    JobNotFound(JobId),
    DeleteJob(trogon_cron::CronError),
}

impl fmt::Display for CommandError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidJobId(source) => write!(f, "{source}"),
            Self::GetJob(source) => write!(f, "failed to load job: {source}"),
            Self::JobNotFound(id) => write!(f, "job '{id}' not found"),
            Self::DeleteJob(source) => write!(f, "failed to remove job: {source}"),
        }
    }
}

impl std::error::Error for CommandError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::InvalidJobId(source) => Some(source),
            Self::GetJob(source) => Some(source),
            Self::JobNotFound(_) => None,
            Self::DeleteJob(source) => Some(source),
        }
    }
}

pub async fn run(store: &NatsConfigStore, id: &str) -> Result<(), CommandError> {
    let job_id = JobId::parse(id).map_err(CommandError::InvalidJobId)?;
    let version = current_job_version(store, &job_id).await?;
    store
        .delete_job(job_id.as_str(), JobWriteCondition::MustBeAtVersion(version))
        .await
        .map_err(CommandError::DeleteJob)?;
    println!("Job '{job_id}' removed.");

    Ok(())
}

async fn current_job_version(store: &NatsConfigStore, id: &JobId) -> Result<u64, CommandError> {
    store
        .get_job(id.as_str())
        .await
        .map_err(CommandError::GetJob)?
        .map(|job| job.version)
        .ok_or_else(|| CommandError::JobNotFound(id.clone()))
}
