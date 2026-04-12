#[cfg(not(coverage))]
mod cli;
#[cfg(not(coverage))]
mod runtime_config;

#[cfg(not(coverage))]
use std::io::Read;
#[cfg(not(coverage))]
use std::time::Duration;
#[cfg(not(coverage))]
use std::{error::Error, fmt};

#[cfg(not(coverage))]
use tracing::{error, info};
#[cfg(not(coverage))]
use trogon_cron::{
    ConfigStore, CronController, JobEnabledState, JobSpec, JobWriteCondition, NatsConfigStore,
    ScheduleSpec,
};
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
#[derive(Debug)]
enum CliError {
    ReadFile {
        path: String,
        source: std::io::Error,
    },
    InvalidJobSpec {
        source: serde_json::Error,
    },
}

#[cfg(not(coverage))]
impl fmt::Display for CliError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::ReadFile { path, source } => write!(f, "failed to read '{path}': {source}"),
            Self::InvalidJobSpec { source } => write!(f, "invalid job spec: {source}"),
        }
    }
}

#[cfg(not(coverage))]
impl Error for CliError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::ReadFile { source, .. } => Some(source),
            Self::InvalidJobSpec { source } => Some(source),
        }
    }
}

#[cfg(not(coverage))]
#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let cli = CliArgs::<cli::Cli>::new().parse_args();

    trogon_telemetry::init_logger(
        trogon_telemetry::ServiceName::TrogonCron,
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

    if let Err(e) = trogon_telemetry::shutdown_otel() {
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
async fn store_from_nats(
    nats: async_nats::Client,
) -> Result<NatsConfigStore, Box<dyn std::error::Error>> {
    Ok(NatsConfigStore::new(nats).await?)
}

#[cfg(not(coverage))]
async fn run_controller(nats: async_nats::Client) -> Result<(), Box<dyn std::error::Error>> {
    let controller = CronController::from_nats(nats).await?;
    controller.run().await?;
    Ok(())
}

#[cfg(not(coverage))]
async fn handle_job(
    store: NatsConfigStore,
    action: cli::JobAction,
) -> Result<(), Box<dyn std::error::Error>> {
    match action {
        cli::JobAction::List => cmd_list(&store).await,
        cli::JobAction::Get { id } => cmd_get(&store, &id).await,
        cli::JobAction::Add { file } => cmd_add(&store, &file).await,
        cli::JobAction::Remove { id } => cmd_remove(&store, &id).await,
        cli::JobAction::Enable { id } => cmd_set_state(&store, &id, JobEnabledState::Enabled).await,
        cli::JobAction::Disable { id } => {
            cmd_set_state(&store, &id, JobEnabledState::Disabled).await
        }
    }
}

#[cfg(not(coverage))]
async fn cmd_list(store: &NatsConfigStore) -> Result<(), Box<dyn std::error::Error>> {
    let jobs = store.list_jobs().await?;
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

#[cfg(not(coverage))]
async fn cmd_get(store: &NatsConfigStore, id: &str) -> Result<(), Box<dyn std::error::Error>> {
    match store.get_job(id).await? {
        Some(job) => println!("{}", serde_json::to_string_pretty(&job).unwrap()),
        None => {
            return Err(std::io::Error::new(
                std::io::ErrorKind::NotFound,
                format!("job '{id}' not found"),
            )
            .into());
        }
    }

    Ok(())
}

#[cfg(not(coverage))]
async fn cmd_add(store: &NatsConfigStore, file: &str) -> Result<(), Box<dyn std::error::Error>> {
    let json = if file == "-" {
        let mut buf = String::new();
        std::io::stdin().read_to_string(&mut buf)?;
        buf
    } else {
        std::fs::read_to_string(file).map_err(|source| CliError::ReadFile {
            path: file.to_owned(),
            source,
        })?
    };

    let spec: JobSpec =
        serde_json::from_str(&json).map_err(|source| CliError::InvalidJobSpec { source })?;
    let id = spec.id.clone();
    store
        .put_job(&spec, JobWriteCondition::MustNotExist)
        .await?;
    println!("Job '{id}' registered.");

    Ok(())
}

#[cfg(not(coverage))]
async fn cmd_remove(store: &NatsConfigStore, id: &str) -> Result<(), Box<dyn std::error::Error>> {
    let version = current_job_version(store, id).await?;
    store
        .delete_job(id, JobWriteCondition::MustBeAtVersion(version))
        .await?;
    println!("Job '{id}' removed.");

    Ok(())
}

#[cfg(not(coverage))]
async fn cmd_set_state(
    store: &NatsConfigStore,
    id: &str,
    state: JobEnabledState,
) -> Result<(), Box<dyn std::error::Error>> {
    let version = current_job_version(store, id).await?;
    store
        .set_job_state(id, state, JobWriteCondition::MustBeAtVersion(version))
        .await?;
    println!("Job '{id}' {}.", state.as_str());

    Ok(())
}

#[cfg(not(coverage))]
async fn current_job_version(
    store: &NatsConfigStore,
    id: &str,
) -> Result<u64, Box<dyn std::error::Error>> {
    store
        .get_job(id)
        .await?
        .map(|job| job.version)
        .ok_or_else(|| {
            std::io::Error::new(
                std::io::ErrorKind::NotFound,
                format!("job '{id}' not found"),
            )
            .into()
        })
}

#[cfg(all(coverage, test))]
mod tests {
    #[test]
    fn coverage_stub() {
        super::main();
    }
}
