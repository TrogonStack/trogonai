use std::fmt;

use trogon_cron::{ConfigStore, JobEnabledState, JobWriteCondition, NatsConfigStore};

use super::job_id::{JobId, JobIdError};

#[derive(Debug)]
pub enum CommandError {
    InvalidJobId(JobIdError),
    GetJob(trogon_cron::CronError),
    JobNotFound(JobId),
    SetState(trogon_cron::CronError),
}

impl fmt::Display for CommandError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidJobId(source) => write!(f, "{source}"),
            Self::GetJob(source) => write!(f, "failed to load job: {source}"),
            Self::JobNotFound(id) => write!(f, "job '{id}' not found"),
            Self::SetState(source) => write!(f, "failed to update job state: {source}"),
        }
    }
}

impl std::error::Error for CommandError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::InvalidJobId(source) => Some(source),
            Self::GetJob(source) => Some(source),
            Self::JobNotFound(_) => None,
            Self::SetState(source) => Some(source),
        }
    }
}

pub async fn run(
    store: &NatsConfigStore,
    id: &str,
    state: JobEnabledState,
) -> Result<(), CommandError> {
    let job_id = JobId::parse(id).map_err(CommandError::InvalidJobId)?;
    let version = current_job_version(store, &job_id).await?;
    store
        .set_job_state(
            job_id.as_str(),
            state,
            JobWriteCondition::MustBeAtVersion(version),
        )
        .await
        .map_err(CommandError::SetState)?;
    println!("Job '{job_id}' {}.", state.as_str());

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
