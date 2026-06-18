use trogon_service_config::RuntimeConfigArgs;

#[derive(clap::Parser, Clone)]
#[command(name = "trogon-gateway", about = "Unified gateway ingestion binary")]
pub struct Cli {
    #[command(flatten)]
    pub runtime: RuntimeConfigArgs,

    #[command(subcommand)]
    pub command: Command,
}

#[derive(clap::Subcommand, Clone)]
pub enum Command {
    Serve,
    Source {
        #[command(subcommand)]
        source: SourceCommand,
    },
}

#[derive(clap::Subcommand, Clone)]
pub enum SourceCommand {
    Notion {
        #[command(subcommand)]
        command: NotionCommand,
    },
}

#[derive(clap::Subcommand, Clone)]
pub enum NotionCommand {
    VerificationToken {
        #[arg(long, default_value = "primary")]
        integration: String,
        #[arg(long)]
        watch: bool,
    },
}
