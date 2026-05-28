use trogon_service_config::RuntimeConfigArgs;

#[derive(clap::Parser, Clone)]
#[command(name = "trogon-scheduler", about = "Distributed schedule runner over NATS")]
pub struct Cli {
    #[command(flatten)]
    pub runtime: RuntimeConfigArgs,
}
