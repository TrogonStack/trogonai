#![cfg_attr(coverage_nightly, feature(coverage_attribute))]

use mcp_postgres::{
    Authenticator, ConfigInput, HttpTransportConfig, QueryService, RuntimeConfig, SpiceDb, http_router,
};
use std::{process::ExitCode, sync::Arc};
use tracing_subscriber::{Layer, layer::SubscriberExt, util::SubscriberInitExt};
use trogon_std::env::{ReadEnv, SystemEnv};

#[tokio::main]
#[cfg_attr(coverage_nightly, coverage(off))]
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
    let mut input: ConfigInput = toml::from_str(&std::fs::read_to_string(path)?)?;
    let http = HttpTransportConfig::new(input.http.take().ok_or(mcp_postgres::Error::Configuration)?)?;
    let authentication =
        Arc::new(Authenticator::new(input.authentication.take().ok_or(mcp_postgres::Error::Configuration)?).await?);
    let address = http.bind();
    let config = RuntimeConfig::from_input(input, |key| SystemEnv.var(key).ok())?;
    let permissions =
        Arc::new(SpiceDb::connect(config.spicedb_endpoint().into(), config.spicedb_token().into()).await?);
    let service = QueryService::connect(config, permissions).await?;
    let app = http_router(service, authentication, http);
    #[cfg(unix)]
    let mut terminate = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())?;
    #[cfg(unix)]
    let shutdown = async move {
        tokio::select! {
            _ = tokio::signal::ctrl_c() => {},
            _ = terminate.recv() => {},
        }
    };
    #[cfg(not(unix))]
    let shutdown = async {
        let _ = tokio::signal::ctrl_c().await;
    };
    let listener = tokio::net::TcpListener::bind(address).await?;
    axum::serve(listener, app).with_graceful_shutdown(shutdown).await?;
    Ok(())
}
