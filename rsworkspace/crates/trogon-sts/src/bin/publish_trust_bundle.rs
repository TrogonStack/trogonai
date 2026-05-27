use std::path::PathBuf;

use clap::Parser;
use tracing::info;

use trogon_sts::TRUST_BUNDLES_KV_BUCKET;

#[derive(Debug, Parser)]
#[command(
    name = "trogon-sts-publish-trust-bundle",
    about = "Publish a SPIFFE trust bundle PEM to NATS KV mcp-trust-bundles/<trust-domain>"
)]
struct Args {
    #[arg(long, env = "NATS_URL")]
    nats_url: String,

    #[arg(long, env = "MCP_STS_TRUST_BUNDLE_PATH")]
    trust_bundle_path: PathBuf,

    #[arg(long, env = "MCP_STS_TRUST_DOMAIN")]
    trust_domain: String,
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .init();

    let args = Args::parse();
    let pem = tokio::fs::read_to_string(&args.trust_bundle_path).await?;
    let nats = async_nats::ConnectOptions::new().connect(&args.nats_url).await?;
    let js = async_nats::jetstream::new(nats);
    let store = match js.get_key_value(TRUST_BUNDLES_KV_BUCKET).await {
        Ok(store) => store,
        Err(_) => {
            js.create_key_value(async_nats::jetstream::kv::Config {
                bucket: TRUST_BUNDLES_KV_BUCKET.to_string(),
                ..Default::default()
            })
            .await?
        }
    };
    store.put(&args.trust_domain, pem.into()).await?;
    info!(
        bucket = TRUST_BUNDLES_KV_BUCKET,
        key = %args.trust_domain,
        "published trust bundle"
    );
    Ok(())
}
