use trogon_service_config::RuntimeConfigArgs;

#[derive(clap::Parser, Clone)]
#[command(name = "trogon-cron", about = "Distributed CRON scheduler over NATS")]
pub struct Cli {
    #[command(flatten)]
    pub runtime: RuntimeConfigArgs,
}
