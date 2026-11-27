use clap::Parser;
use trogon_sandbox_mcp::{Config, RunError, init_logging, run};
use trogon_std::env::SystemEnv;

#[tokio::main]
async fn main() -> Result<(), RunError> {
    let config = Config::parse();
    init_logging(&config.log_level);
    run(config, &SystemEnv, rmcp::transport::stdio()).await
}
