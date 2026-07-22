//! `trogonai-catalog` — NATS-backed model catalog populator (S1 keystone).

use std::time::Duration;

use clap::Parser;
use tracing::info;
use trogon_service_config::{RuntimeConfigArgs, load_config, resolve_nats};
use trogonai_catalog::config::CatalogServiceConfig;
use trogonai_catalog::populator::Populator;
use trogonai_catalog_client::provision;

#[derive(Parser)]
struct Cli {
    #[command(flatten)]
    runtime: RuntimeConfigArgs,
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| "trogonai_catalog=info".into()),
        )
        .init();

    let cli = Cli::parse();
    let cfg: CatalogServiceConfig = load_config(cli.runtime.config.as_deref())?;
    let nats_cfg = resolve_nats(&cfg.nats, &cli.runtime.nats);

    let nats = async_nats::connect(&nats_cfg.servers.join(",")).await?;
    let js = async_nats::jetstream::new(nats.clone());
    let store = provision(&js).await.map_err(|e| e.to_string())?;

    let populator = Populator {
        client: reqwest::Client::new(),
        proxy_url: cfg.proxy_url.clone(),
        store,
        nats,
        nats_prefix: cfg.nats_prefix.clone(),
    };

    let ttl = Duration::from_secs(cfg.catalog_ttl_secs);
    info!(
        service.name = "trogonai-catalog",
        ttl_secs = cfg.catalog_ttl_secs,
        "catalog service started"
    );

    loop {
        if let Err(e) = populator.refresh_all().await {
            tracing::warn!(error = %e, "catalog refresh failed");
        }
        tokio::time::sleep(ttl).await;
    }
}
