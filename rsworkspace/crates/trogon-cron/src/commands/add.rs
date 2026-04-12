use std::{fmt, io::Read};

use trogon_cron::{ConfigStore, JobSpec, NatsConfigStore};

#[derive(Debug)]
pub enum CommandError {
    ReadStdin(std::io::Error),
    ReadFile {
        path: String,
        source: std::io::Error,
    },
    InvalidJobSpec(serde_json::Error),
    PutJob(trogon_cron::CronError),
}

impl fmt::Display for CommandError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::ReadStdin(source) => write!(f, "failed to read stdin: {source}"),
            Self::ReadFile { path, source } => write!(f, "failed to read '{path}': {source}"),
            Self::InvalidJobSpec(source) => write!(f, "invalid job spec: {source}"),
            Self::PutJob(source) => write!(f, "failed to register job: {source}"),
        }
    }
}

impl std::error::Error for CommandError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::ReadStdin(source) => Some(source),
            Self::ReadFile { source, .. } => Some(source),
            Self::InvalidJobSpec(source) => Some(source),
            Self::PutJob(source) => Some(source),
        }
    }
}

pub async fn run(store: &NatsConfigStore, file: &str) -> Result<(), CommandError> {
    let json = if file == "-" {
        let mut buf = String::new();
        std::io::stdin()
            .read_to_string(&mut buf)
            .map_err(CommandError::ReadStdin)?;
        buf
    } else {
        std::fs::read_to_string(file).map_err(|source| CommandError::ReadFile {
            path: file.to_owned(),
            source,
        })?
    };

    let spec: JobSpec = serde_json::from_str(&json).map_err(CommandError::InvalidJobSpec)?;
    let id = spec.id.clone();
    store
        .put_job(&spec, trogon_cron::JobWriteCondition::MustNotExist)
        .await
        .map_err(CommandError::PutJob)?;
    println!("Job '{id}' registered.");

    Ok(())
}
