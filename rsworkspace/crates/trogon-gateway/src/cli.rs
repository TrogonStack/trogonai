use std::path::PathBuf;

#[derive(clap::Parser, Clone)]
#[command(name = "trogon-gateway", about = "Unified gateway ingestion binary")]
pub struct Cli {
    #[command(subcommand)]
    pub command: Command,
}

#[derive(clap::Subcommand, Clone)]
pub enum Command {
    Serve {
        #[arg(long, short)]
        config: Option<PathBuf>,
    },
}
