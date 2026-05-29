use std::net::SocketAddr;

use clap::Parser;
use tonic::transport::Server;
use tracing_subscriber::EnvFilter;

use trogon_gateway_xds::proto::AggregatedDiscoveryServiceServer;
use trogon_gateway_xds::state::{self, ConfigStore, NodeSnapshot, DEFAULT_KV_BUCKET, DEFAULT_KV_KEY_PREFIX};
use trogon_gateway_xds::{AdsServer, AdsServerOpts};

#[derive(Debug, Parser)]
#[command(name = "trogon-gateway-xds", about = "Envoy xDS v3 interop bridge for Trogon gateway config")]
struct Cli {
    #[arg(long, env = "TROGON_XDS_LISTEN", default_value = "0.0.0.0:18000")]
    listen: SocketAddr,

    #[arg(long, env = "NATS_URL", default_value = "nats://127.0.0.1:4222")]
    nats_url: String,

    #[arg(long, env = "TROGON_XDS_KV_BUCKET", default_value = DEFAULT_KV_BUCKET)]
    kv_bucket: String,

    #[arg(long, env = "TROGON_XDS_KV_PREFIX", default_value = DEFAULT_KV_KEY_PREFIX)]
    kv_prefix: String,

    #[arg(long, env = "TROGON_XDS_DEFAULT_NODE_ID", default_value = "trogon-gateway")]
    default_node_id: String,
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::from_default_env().add_directive("trogon_gateway_xds=info".parse()?))
        .init();

    let cli = Cli::parse();
    let store = init_store(&cli).await?;
    let ads = AdsServer::new(
        store,
        AdsServerOpts {
            default_node_id: cli.default_node_id,
        },
    );

    tracing::info!(listen = %cli.listen, "starting xDS ADS server");
    Server::builder()
        .add_service(AggregatedDiscoveryServiceServer::new(ads))
        .serve(cli.listen)
        .await?;
    Ok(())
}

async fn init_store(cli: &Cli) -> Result<ConfigStore, Box<dyn std::error::Error>> {
    let client = async_nats::connect(&cli.nats_url).await?;
    let jetstream = async_nats::jetstream::new(client);
    match jetstream.get_key_value(&cli.kv_bucket).await {
        Ok(kv) => {
            let watcher = state::watch_kv(kv, cli.kv_prefix.clone()).await?;
            Ok(watcher.store())
        }
        Err(error) => {
            tracing::warn!(%error, "KV bucket unavailable; starting with empty in-memory store");
            let (store, _rx) = ConfigStore::new(std::collections::HashMap::new());
            store.upsert(NodeSnapshot::from_config(
                cli.default_node_id.clone(),
                trogon_gateway_xds::config::GatewayConfigSnapshot {
                    version: "bootstrap-0".into(),
                    ..Default::default()
                },
            ));
            Ok(store)
        }
    }
}
