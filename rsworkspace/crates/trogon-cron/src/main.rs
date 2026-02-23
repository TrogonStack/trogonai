use std::io::Read;

use clap::{Parser, Subcommand};
use trogon_cron::{CronClient, JobConfig, Schedule, Scheduler};
use trogon_nats::{NatsConfig, connect as nats_connect};
use trogon_std::env::SystemEnv;

/// Distributed CRON scheduler over NATS.
#[derive(Parser)]
#[command(name = "trogon-cron", version)]
struct Cli {
    /// NATS server URL
    #[arg(long, env = "NATS_URL", default_value = "nats://localhost:4222", global = true)]
    nats_url: String,

    #[command(subcommand)]
    command: Option<Command>,
}

#[derive(Subcommand)]
enum Command {
    /// Run the CRON scheduler (default when no subcommand is given)
    Serve,
    /// Manage scheduled jobs
    Job {
        #[command(subcommand)]
        action: JobAction,
    },
}

#[derive(Subcommand)]
enum JobAction {
    /// List all registered jobs
    List,
    /// Show a specific job config
    Get { id: String },
    /// Register or update a job from a JSON file (use - for stdin)
    Add { file: String },
    /// Remove a job by id
    Remove { id: String },
    /// Enable a job
    Enable { id: String },
    /// Disable a job without removing it
    Disable { id: String },
}

#[tokio::main]
async fn main() {
    let cli = Cli::parse();

    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .init();

    let nats = connect(&cli.nats_url).await;

    match cli.command {
        None | Some(Command::Serve) => {
            tracing::info!("Starting CRON scheduler");
            if let Err(e) = Scheduler::new(nats).run().await {
                tracing::error!(error = %e, "Scheduler exited with error");
                std::process::exit(1);
            }
        }
        Some(Command::Job { action }) => {
            let client = CronClient::new(nats).await.unwrap_or_else(die);
            handle_job(client, action).await;
        }
    }
}

async fn connect(url: &str) -> async_nats::Client {
    let config = NatsConfig {
        servers: vec![url.to_string()],
        auth: NatsConfig::from_env(&SystemEnv).auth,
    };
    nats_connect(&config).await.unwrap_or_else(|e| {
        eprintln!("Failed to connect to NATS: {e}");
        std::process::exit(1);
    })
}

async fn handle_job(client: CronClient, action: JobAction) {
    match action {
        JobAction::List => cmd_list(client).await,
        JobAction::Get { id } => cmd_get(client, &id).await,
        JobAction::Add { file } => cmd_add(client, &file).await,
        JobAction::Remove { id } => cmd_remove(client, &id).await,
        JobAction::Enable { id } => cmd_set_enabled(client, &id, true).await,
        JobAction::Disable { id } => cmd_set_enabled(client, &id, false).await,
    }
}

async fn cmd_list(client: CronClient) {
    let jobs = client.list_jobs().await.unwrap_or_else(die);
    if jobs.is_empty() {
        println!("No jobs registered.");
        return;
    }
    println!("{:<30} {:<10} SCHEDULE", "ID", "STATUS");
    println!("{}", "-".repeat(60));
    for job in jobs {
        let status = if job.enabled { "enabled" } else { "disabled" };
        let schedule = match &job.schedule {
            Schedule::Interval { interval_sec } => format!("every {interval_sec}s"),
            Schedule::Cron { expr } => expr.clone(),
        };
        println!("{:<30} {:<10} {}", job.id, status, schedule);
    }
}

async fn cmd_get(client: CronClient, id: &str) {
    match client.get_job(id).await.unwrap_or_else(die) {
        Some(job) => println!("{}", serde_json::to_string_pretty(&job).unwrap()),
        None => {
            eprintln!("Job '{id}' not found");
            std::process::exit(1);
        }
    }
}

async fn cmd_add(client: CronClient, file: &str) {
    let json = if file == "-" {
        let mut buf = String::new();
        std::io::stdin()
            .read_to_string(&mut buf)
            .unwrap_or_else(die);
        buf
    } else {
        std::fs::read_to_string(file).unwrap_or_else(|e| {
            eprintln!("Failed to read '{file}': {e}");
            std::process::exit(1);
        })
    };

    let config: JobConfig = serde_json::from_str(&json).unwrap_or_else(|e| {
        eprintln!("Invalid job config: {e}");
        std::process::exit(1);
    });
    let id = config.id.clone();
    client.register_job(&config).await.unwrap_or_else(die);
    println!("Job '{id}' registered.");
}

async fn cmd_remove(client: CronClient, id: &str) {
    client.remove_job(id).await.unwrap_or_else(die);
    println!("Job '{id}' removed.");
}

async fn cmd_set_enabled(client: CronClient, id: &str, enabled: bool) {
    client
        .set_enabled(id, enabled)
        .await
        .unwrap_or_else(die);
    let state = if enabled { "enabled" } else { "disabled" };
    println!("Job '{id}' {state}.");
}

fn die<T>(e: impl std::fmt::Display) -> T {
    eprintln!("Error: {e}");
    std::process::exit(1);
}
