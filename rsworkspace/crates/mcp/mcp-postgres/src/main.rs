use mcp_postgres::{ConfigInput, PostgresMcp, QueryService, RuntimeConfig, SpiceDb};
use rmcp::ServiceExt;
use std::{process::ExitCode, sync::Arc};
use tracing_subscriber::{Layer, layer::SubscriberExt, util::SubscriberInitExt};
use trogon_std::env::{ReadEnv, SystemEnv};

#[tokio::main]
async fn main() -> ExitCode {
    let audit = tracing_subscriber::fmt::layer()
        .json()
        .with_ansi(false)
        .with_writer(std::io::stderr)
        .with_filter(tracing_subscriber::filter::Targets::new().with_target("mcp_postgres", tracing::Level::INFO));
    if tracing_subscriber::registry().with(audit).try_init().is_err() {
        return ExitCode::FAILURE;
    }
    if run().await.is_err() {
        tracing::error!("mcp-postgres startup or transport failed");
        return ExitCode::FAILURE;
    }
    ExitCode::SUCCESS
}
async fn run() -> Result<(), Box<dyn std::error::Error>> {
    let mut args = std::env::args().skip(1);
    let path = args.next().ok_or(mcp_postgres::Error::Configuration)?;
    if args.next().is_some() {
        return Err(mcp_postgres::Error::Configuration.into());
    }
    let input: ConfigInput = toml::from_str(&std::fs::read_to_string(path)?)?;
    let config = RuntimeConfig::from_input(input, |key| SystemEnv.var(key).ok())?;
    let permissions =
        Arc::new(SpiceDb::connect(config.spicedb_endpoint().into(), config.spicedb_token().into()).await?);
    let service = QueryService::connect(config, permissions).await?;
    let server = PostgresMcp::new(service).serve(rmcp::transport::stdio()).await?;
    server.waiting().await?;
    Ok(())
}
