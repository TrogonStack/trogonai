use std::net::SocketAddr;
use std::path::PathBuf;

use clap::{Parser, ValueEnum};
use tracing::{error, info};
use trogon_jwks_publisher::JwksState;
use trogon_jwks_publisher::keys::KeySource;
use trogon_jwks_publisher::keys::cloud::{KmsKeySource, VaultKeySource};
use trogon_jwks_publisher::keys::file::FileKeySource;
use trogon_jwks_publisher::publishers::https;
use trogon_jwks_publisher::publishers::kv::{self, ENV_CREATE_KV_BUCKET};
use trogon_jwks_publisher::publishers::reqrep;

#[derive(Debug, Clone, ValueEnum)]
enum KeySourceKind {
    File,
    Kms,
    Vault,
}

#[derive(Parser, Debug)]
#[command(name = "trogon-jwks-publisher")]
#[command(about = "Publish mesh-token JWKS to NATS KV, HTTPS, and request/reply")]
struct Args {
    #[arg(long, env = "NATS_URL", default_value = "nats://127.0.0.1:4222")]
    nats_url: String,

    #[arg(long, value_enum, default_value = "file")]
    key_source: KeySourceKind,

    #[arg(long, default_value = "/etc/trogon-mesh-keys")]
    key_dir: PathBuf,

    #[arg(long, default_value = "127.0.0.1:8080")]
    https_listen: SocketAddr,

    #[arg(long)]
    tls_cert: Option<PathBuf>,

    #[arg(long)]
    tls_key: Option<PathBuf>,
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .init();

    let args = Args::parse();
    let create_kv_bucket = std::env::var(ENV_CREATE_KV_BUCKET)
        .ok()
        .is_some_and(|v| matches!(v.as_str(), "1" | "true" | "TRUE" | "yes" | "YES"));

    let watch_rx = match args.key_source {
        KeySourceKind::File => {
            let (source, watch_rx) = FileKeySource::spawn(args.key_dir.clone()).await?;
            let initial = source.current().await?;
            info!(%initial, "loaded initial mesh JWKS");
            watch_rx
        }
        KeySourceKind::Kms => {
            let kms = KmsKeySource::new();
            let initial = kms.current().await?;
            info!(%initial, "loaded initial mesh JWKS from KMS");
            let (_state, watch_rx) = JwksState::new(initial);
            watch_rx
        }
        KeySourceKind::Vault => {
            let vault = VaultKeySource::new();
            let initial = vault.current().await?;
            info!(%initial, "loaded initial mesh JWKS from Vault");
            let (_state, watch_rx) = JwksState::new(initial);
            watch_rx
        }
    };

    let nats_url = args.nats_url.clone();
    let kv_watch = watch_rx.clone();
    tokio::spawn(async move {
        if let Err(err) = kv::run_kv_publisher(&nats_url, kv_watch, create_kv_bucket).await {
            error!(error = %err, "KV publisher stopped");
        }
    });

    let nats_url = args.nats_url;
    let reqrep_watch = watch_rx.clone();
    tokio::spawn(async move {
        if let Err(err) = reqrep::run_reqrep_publisher(&nats_url, reqrep_watch).await {
            error!(error = %err, "request/reply publisher stopped");
        }
    });

    https::run_https_publisher(args.https_listen, watch_rx, args.tls_cert, args.tls_key).await?;

    Ok(())
}
