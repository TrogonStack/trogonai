use std::fmt;

use trogon_cron::{ConfigStore, NatsConfigStore};

use super::job_id::{JobId, JobIdError};

#[derive(Debug)]
pub enum CommandError {
    GetJob(trogon_cron::CronError),
    SerializeJob(serde_json::Error),
    InvalidJobId(JobIdError),
    JobNotFound(JobId),
}

impl fmt::Display for CommandError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::GetJob(source) => write!(f, "failed to load job: {source}"),
            Self::SerializeJob(source) => write!(f, "failed to serialize job: {source}"),
            Self::InvalidJobId(source) => write!(f, "{source}"),
            Self::JobNotFound(id) => write!(f, "job '{id}' not found"),
        }
    }
}

impl std::error::Error for CommandError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::GetJob(source) => Some(source),
            Self::SerializeJob(source) => Some(source),
            Self::InvalidJobId(source) => Some(source),
            Self::JobNotFound(_) => None,
        }
    }
}

pub async fn run(store: &NatsConfigStore, id: &str) -> Result<(), CommandError> {
    let job_id = JobId::parse(id).map_err(CommandError::InvalidJobId)?;

    match store
        .get_job(job_id.as_str())
        .await
        .map_err(CommandError::GetJob)?
    {
        Some(job) => println!(
            "{}",
            serde_json::to_string_pretty(&job).map_err(CommandError::SerializeJob)?
        ),
        None => return Err(CommandError::JobNotFound(job_id)),
    }

    Ok(())
}
