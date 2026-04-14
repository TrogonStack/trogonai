#[cfg(not(coverage))]
mod cli;
#[cfg(not(coverage))]
mod runtime_config;

#[cfg(not(coverage))]
use std::time::Duration;

#[cfg(not(coverage))]
use tracing::{error, info};
#[cfg(not(coverage))]
use trogon_cron::{
    ChangeJobStateCommand, CronController, GetJobCommand, JobEnabledState, JobId, ListJobsCommand,
    RemoveJobCommand, ScheduleSpec, change_job_state, connect_store, get_job, list_jobs,
    read_register_job_from_stdin, register_job, remove_job,
};
#[cfg(not(coverage))]
use trogon_eventsourcing::StreamCommand;
#[cfg(not(coverage))]
use trogon_nats::connect;
#[cfg(not(coverage))]
use trogon_std::args::{CliArgs, ParseArgs};
#[cfg(not(coverage))]
use trogon_std::env::SystemEnv;
#[cfg(not(coverage))]
use trogon_std::fs::SystemFs;

#[cfg(not(coverage))]
const NATS_CONNECT_TIMEOUT: Duration = Duration::from_secs(10);

#[cfg(not(coverage))]
#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let cli = CliArgs::<cli::Cli>::new().parse_args();

    acp_telemetry::init_logger(
        acp_telemetry::ServiceName::TrogonCron,
        "cron",
        &SystemEnv,
        &SystemFs,
    );

    info!("trogon-cron starting");

    let result = run(cli).await;

    match &result {
        Ok(()) => info!("trogon-cron stopped"),
        Err(e) => error!(error = %e, "trogon-cron stopped with error"),
    }

    if let Err(e) = acp_telemetry::shutdown_otel() {
        error!(error = %e, "OpenTelemetry shutdown failed");
    }

    result
}

#[cfg(coverage)]
fn main() {}

#[cfg(not(coverage))]
async fn run(cli: cli::Cli) -> Result<(), Box<dyn std::error::Error>> {
    let resolved =
        runtime_config::load_with_overrides(cli.runtime.config.as_deref(), &cli.runtime.nats)?;
    let nats = connect(&resolved.nats, NATS_CONNECT_TIMEOUT).await?;

    match cli.command.unwrap_or(cli::Command::Serve) {
        cli::Command::Serve => run_controller(nats).await,
        cli::Command::Job { action } => handle_job(store_from_nats(nats).await?, action).await,
    }
}

#[cfg(not(coverage))]
async fn handle_job(
    js: async_nats::jetstream::Context,
    action: cli::JobAction,
) -> Result<(), Box<dyn std::error::Error>> {
    match action {
        cli::JobAction::List => {
            let jobs = list_jobs(&js, ListJobsCommand).await?;
            if jobs.is_empty() {
                println!("No jobs registered.");
                return Ok(());
            }

            println!("{:<30} {:<10} SCHEDULE", "ID", "STATUS");
            println!("{}", "-".repeat(72));
            for job in jobs {
                let schedule = match &job.payload.schedule {
                    ScheduleSpec::At { at } => format!("at {}", at.to_rfc3339()),
                    ScheduleSpec::Every { every_sec } => format!("@every {every_sec}s"),
                    ScheduleSpec::Cron { expr, timezone } => match timezone {
                        Some(timezone) => format!("{expr} [{timezone}]"),
                        None => expr.clone(),
                    },
                };
                println!(
                    "{:<30} {:<10} {} (v{})",
                    job.payload.id,
                    job.payload.state.as_str(),
                    schedule,
                    job.version
                );
            }
            Ok(())
        }
        cli::JobAction::Get { id } => {
            let id = JobId::parse(&id)?;
            match get_job(&js, GetJobCommand { id: id.clone() }).await? {
                Some(job) => println!("{}", serde_json::to_string_pretty(&job)?),
                None => {
                    return Err(trogon_cron::CronError::JobNotFound { id: id.to_string() }.into());
                }
            }
            Ok(())
        }
        cli::JobAction::Add => {
            let command = read_register_job_from_stdin()?;
            let id = command.stream_id().to_string();
            register_job(&js, command).await?;
            println!("Job '{id}' registered.");
            Ok(())
        }
        cli::JobAction::Remove { id } => {
            let command = RemoveJobCommand::new(JobId::parse(&id)?);
            let stream_id = command.stream_id().to_string();
            remove_job(&js, command).await?;
            println!("Job '{stream_id}' removed.");
            Ok(())
        }
        cli::JobAction::Enable { id } => {
            let command = ChangeJobStateCommand::new(JobId::parse(&id)?, JobEnabledState::Enabled);
            let stream_id = command.stream_id().to_string();
            change_job_state(&js, command).await?;
            println!("Job '{stream_id}' enabled.");
            Ok(())
        }
        cli::JobAction::Disable { id } => {
            let command = ChangeJobStateCommand::new(JobId::parse(&id)?, JobEnabledState::Disabled);
            let stream_id = command.stream_id().to_string();
            change_job_state(&js, command).await?;
            println!("Job '{stream_id}' disabled.");
            Ok(())
        }
    }
}

#[cfg(not(coverage))]
async fn store_from_nats(
    nats: async_nats::Client,
) -> Result<async_nats::jetstream::Context, Box<dyn std::error::Error>> {
    Ok(connect_store(nats).await?)
}

#[cfg(not(coverage))]
async fn run_controller(nats: async_nats::Client) -> Result<(), Box<dyn std::error::Error>> {
    let controller = CronController::from_nats(nats).await?;
    controller.run().await?;
    Ok(())
}

#[cfg(all(coverage, test))]
mod tests {
    #[test]
    fn coverage_stub() {
        super::main();
    }
}
