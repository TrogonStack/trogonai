use std::fmt;

use async_nats::jetstream::kv;
use trogon_cron::{
    CronError, SNAPSHOT_STORE_CONFIG, ScheduleSpec, VersionedJobSpec, open_snapshot_bucket,
};
use trogon_eventsourcing::list_snapshots;
use trogon_nats::jetstream::JetStreamGetKeyValue;

#[derive(Debug, Default)]
pub struct ListCommand;

#[derive(Debug)]
pub enum CommandError {
    ListJobs(CronError),
}

impl fmt::Display for CommandError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::ListJobs(source) => write!(f, "failed to list jobs: {source}"),
        }
    }
}

impl std::error::Error for CommandError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::ListJobs(source) => Some(source),
        }
    }
}

pub async fn run<J>(js: &J, _command: ListCommand) -> Result<(), CommandError>
where
    J: JetStreamGetKeyValue<Store = kv::Store>,
{
    let bucket = open_snapshot_bucket(js)
        .await
        .map_err(CommandError::ListJobs)?;
    let jobs = list_snapshots::<VersionedJobSpec>(&bucket, SNAPSHOT_STORE_CONFIG)
        .await
        .map_err(CronError::from)
        .map_err(CommandError::ListJobs)?;
    if jobs.is_empty() {
        println!("No jobs registered.");
        return Ok(());
    }
    println!("{:<30} {:<10} SCHEDULE", "ID", "STATUS");
    println!("{}", "-".repeat(72));
    for job in jobs {
        let schedule = match &job.spec.schedule {
            ScheduleSpec::At { at } => format!("at {}", at.to_rfc3339()),
            ScheduleSpec::Every { every_sec } => format!("@every {every_sec}s"),
            ScheduleSpec::Cron { expr, timezone } => match timezone {
                Some(timezone) => format!("{expr} [{timezone}]"),
                None => expr.clone(),
            },
        };
        println!(
            "{:<30} {:<10} {} (v{})",
            job.spec.id,
            job.spec.state.as_str(),
            schedule,
            job.version
        );
    }

    Ok(())
}
