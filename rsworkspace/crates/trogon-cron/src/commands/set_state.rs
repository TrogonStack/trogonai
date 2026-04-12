use std::fmt;

use trogon_cron::{ConfigStore, JobEnabledState, JobWriteCondition, NatsConfigStore};

use super::job_id::{JobId, JobIdError};

#[derive(Debug)]
pub struct SetStateCommand {
    pub job_id: JobId,
    pub state: JobEnabledState,
}

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

pub async fn run(store: &NatsConfigStore, command: SetStateCommand) -> Result<(), CommandError> {
    let version = current_job_version(store, &command).await?;
    store
        .set_job_state(
            command.job_id.as_str(),
            command.state,
            JobWriteCondition::MustBeAtVersion(version),
        )
        .await
        .map_err(CommandError::SetState)?;
    println!("Job '{}' {}.", command.job_id, command.state.as_str());

    Ok(())
}

async fn current_job_version(
    store: &NatsConfigStore,
    command: &SetStateCommand,
) -> Result<u64, CommandError> {
    store
        .get_job(command.job_id.as_str())
        .await
        .map_err(CommandError::GetJob)?
        .map(|job| job.version)
        .ok_or_else(|| CommandError::JobNotFound(command.job_id.clone()))
}

impl SetStateCommand {
    pub fn new(id: String, state: JobEnabledState) -> Result<Self, CommandError> {
        Ok(Self {
            job_id: JobId::parse(&id).map_err(CommandError::InvalidJobId)?,
            state,
        })
    }
}
