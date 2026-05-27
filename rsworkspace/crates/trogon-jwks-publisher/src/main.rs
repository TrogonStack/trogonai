use std::net::SocketAddr;
use std::path::PathBuf;

use clap::{Parser, ValueEnum};
use tracing::{error, info};
use trogon_jwks_publisher::keys::KeySource;
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

    #[cfg(feature = "kms-aws")]
    #[arg(long, env = "TROGON_JWKS_KMS_KEY_ARN")]
    kms_key_arn: Option<String>,

    #[cfg(feature = "kms-aws")]
    #[arg(long, env = "TROGON_JWKS_KMS_REGION")]
    kms_region: Option<String>,

    #[cfg(feature = "kms-aws")]
    #[arg(long, env = "TROGON_JWKS_KMS_PREVIOUS_KEY_VERSION")]
    kms_previous_key_version: Option<String>,

    #[cfg(feature = "vault")]
    #[arg(long, env = "TROGON_JWKS_VAULT_ADDR", default_value = "http://127.0.0.1:8200")]
    vault_addr: String,

    #[cfg(feature = "vault")]
    #[arg(long, env = "TROGON_JWKS_VAULT_TRANSIT_MOUNT", default_value = "transit")]
    vault_transit_mount: String,

    #[cfg(feature = "vault")]
    #[arg(long, env = "TROGON_JWKS_VAULT_TRANSIT_KEY")]
    vault_transit_key: Option<String>,

    #[cfg(feature = "vault")]
    #[arg(long, env = "TROGON_JWKS_VAULT_TOKEN")]
    vault_token: Option<String>,

    #[arg(long, env = "MCP_STS_MESH_ISSUER")]
    mesh_issuer: Option<String>,

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
        KeySourceKind::Kms => run_kms_source(&args).await?,
        KeySourceKind::Vault => run_vault_source(&args).await?,
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

async fn run_kms_source(args: &Args) -> Result<tokio::sync::watch::Receiver<trogon_jwks_publisher::Jwks>, Box<dyn std::error::Error + Send + Sync>> {
    #[cfg(feature = "kms-aws")]
    {
        use trogon_jwks_publisher::keys::kms::KmsKeySource;

        let key_arn = args
            .kms_key_arn
            .clone()
            .ok_or("kms key source requires TROGON_JWKS_KMS_KEY_ARN or --kms-key-arn")?;
        let source = KmsKeySource::new(
            key_arn,
            args.kms_region.clone(),
            args.kms_previous_key_version.clone(),
            args.mesh_issuer.clone(),
        )
        .await?;
        let initial = source.current().await?;
        info!(%initial, "loaded initial mesh JWKS from AWS KMS");
        let (state, watch_rx) = JwksState::new(initial);
        let source = Arc::new(source);
        spawn_kms_refresh(source, state);
        Ok(watch_rx)
    }
    #[cfg(not(feature = "kms-aws"))]
    {
        let _ = args;
        Err("kms key source requires building with --features kms-aws".into())
    }
}

#[cfg(feature = "kms-aws")]
fn spawn_kms_refresh(source: Arc<trogon_jwks_publisher::keys::kms::KmsKeySource>, state: JwksState) {
    tokio::spawn(async move {
        let mut interval = tokio::time::interval(std::time::Duration::from_secs(60));
        loop {
            interval.tick().await;
            match source.current().await {
                Ok(jwks) => state.update(jwks),
                Err(err) => error!(error = %err, "failed to refresh JWKS from KMS"),
            }
        }
    });
}

async fn run_vault_source(args: &Args) -> Result<tokio::sync::watch::Receiver<trogon_jwks_publisher::Jwks>, Box<dyn std::error::Error + Send + Sync>> {
    #[cfg(feature = "vault")]
    {
        use trogon_jwks_publisher::keys::vault::VaultKeySource;

        let key_name = args
            .vault_transit_key
            .clone()
            .ok_or("vault key source requires TROGON_JWKS_VAULT_TRANSIT_KEY or --vault-transit-key")?;
        let token = args
            .vault_token
            .clone()
            .ok_or("vault key source requires TROGON_JWKS_VAULT_TOKEN or --vault-token")?;
        let source = VaultKeySource::new(
            args.vault_addr.clone(),
            args.vault_transit_mount.clone(),
            key_name,
            token,
            args.mesh_issuer.clone(),
        )
        .await?;
        let initial = source.current().await?;
        info!(%initial, "loaded initial mesh JWKS from Vault Transit");
        let (state, watch_rx) = JwksState::new(initial);
        let source = Arc::new(source);
        spawn_vault_refresh(source, state);
        Ok(watch_rx)
    }
    #[cfg(not(feature = "vault"))]
    {
        let _ = args;
        Err("vault key source requires building with --features vault".into())
    }
}

#[cfg(feature = "vault")]
fn spawn_vault_refresh(source: Arc<trogon_jwks_publisher::keys::vault::VaultKeySource>, state: JwksState) {
    tokio::spawn(async move {
        let mut interval = tokio::time::interval(std::time::Duration::from_secs(60));
        loop {
            interval.tick().await;
            match source.current().await {
                Ok(jwks) => state.update(jwks),
                Err(err) => error!(error = %err, "failed to refresh JWKS from Vault"),
            }
        }
    });
}
