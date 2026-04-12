use trogon_service_config::RuntimeConfigArgs;

#[derive(clap::Parser, Clone)]
#[command(name = "trogon-cron", about = "Distributed CRON scheduler over NATS")]
pub struct Cli {
    #[command(flatten)]
    pub runtime: RuntimeConfigArgs,

    #[command(subcommand)]
    pub command: Option<Command>,
}

#[derive(clap::Subcommand, Clone)]
pub enum Command {
    Serve,
    Job {
        #[command(subcommand)]
        action: JobAction,
    },
}

#[derive(clap::Subcommand, Clone)]
pub enum JobAction {
    List,
    Get { id: String },
    Add,
    Remove { id: String },
    Enable { id: String },
    Disable { id: String },
}
