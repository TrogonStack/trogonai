use std::num::NonZeroU32;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use anyhow::Context;
use clap::Parser;
use trogon_sandbox_contract::{DriveId, DriveName, MountPath, TenantId};
use trogon_sandbox_fly_worker::{
    FlyApi, FlyApiToken, FlyAppName, FlyConfig, FlyConfigParts, FlyEndpoint, FlyPreviewNetwork, FlyProvider, FlyRegion,
    FlyVolumeBinding, FlyVolumeId, FlyWorker, FlyWorkspaceVolume,
};
use trogon_sandbox_protocol::{AuthorityConfig, AuthorityMode, nats_connect, register_state_dir};
use trogon_std::env::SystemEnv;

#[derive(Parser)]
struct Args {
    #[arg(long, env = "TROGON_SANDBOX_NATS_URL", default_value = "nats://127.0.0.1:4222")]
    nats_url: String,
    #[arg(long, env = "TROGON_SANDBOX_FLY_API_URL", default_value = "https://api.machines.dev")]
    endpoint: String,
    #[arg(long, env = "TROGON_SANDBOX_FLY_APP")]
    app: String,
    #[arg(long, env = "FLY_API_TOKEN", hide_env_values = true)]
    token: String,
    #[arg(long, env = "TROGON_SANDBOX_FLY_IMAGE")]
    image: String,
    #[arg(long, env = "TROGON_SANDBOX_FLY_REGION")]
    region: String,
    #[arg(
        long,
        env = "TROGON_SANDBOX_FLY_STATE_DIR",
        default_value = "/var/lib/trogon-sandbox-fly"
    )]
    state_dir: PathBuf,
    #[arg(long, default_value_t = 4)]
    max_leases: usize,
    #[arg(long, default_value_t = NonZeroU32::MIN.saturating_add(511))]
    memory_mib: NonZeroU32,
    #[arg(long, default_value_t = NonZeroU32::MIN)]
    cpus: NonZeroU32,
    #[arg(long, default_value_t = 120)]
    boot_timeout_seconds: u64,
    #[arg(long, default_value_t = 0)]
    warm_pool_size: usize,
    #[arg(long, env = "TROGON_SANDBOX_FLY_VOLUME")]
    volume: Option<String>,
    #[arg(long, env = "TROGON_SANDBOX_FLY_VOLUME_TENANT", requires = "volume")]
    volume_tenant: Option<String>,
    #[arg(long, env = "TROGON_SANDBOX_FLY_VOLUME_DRIVE", requires = "volume")]
    volume_drive: Option<String>,
    #[arg(long, default_value = "/data")]
    volume_path: String,
    /// Gives every lease its own Fly volume of this many GB, mounted at the workspace path and deleted with the
    /// lease. Enables native snapshots. Off by default; incompatible with `--volume` and `--warm-pool-size`.
    #[arg(long, env = "TROGON_SANDBOX_FLY_WORKSPACE_VOLUME_GB")]
    workspace_volume_size_gb: Option<NonZeroU32>,
    /// Seconds between checks for a volume snapshot to finish.
    #[arg(long, env = "TROGON_SANDBOX_FLY_SNAPSHOT_POLL_INTERVAL_SECS", default_value_t = 2)]
    snapshot_poll_interval_secs: u64,
    /// Seconds a volume snapshot may take to finish before the checkpoint fails as transient.
    #[arg(long, env = "TROGON_SANDBOX_FLY_SNAPSHOT_TIMEOUT_SECS", default_value_t = 300)]
    snapshot_timeout_secs: u64,
    #[arg(long, env = "TROGON_SANDBOX_EGRESS_PROXY")]
    egress_proxy: Option<String>,
    #[arg(long, env = "TROGON_SANDBOX_WORKER_ID")]
    worker_id: Option<String>,
    /// Hostname exposed ports are published on, when the app uses a custom domain instead of `<app>.fly.dev`.
    #[arg(long, env = "TROGON_SANDBOX_FLY_PUBLIC_HOST")]
    public_host: Option<String>,
    /// `flycast` publishes exposed ports only on the app's private Flycast address; `public` publishes them on
    /// the internet-facing hostname, where anyone who learns a Machine id can reach them.
    #[arg(long, env = "TROGON_SANDBOX_FLY_PREVIEW_NETWORK", value_enum, default_value_t)]
    preview_network: FlyPreviewNetwork,
}

fn volume_binding(args: &Args) -> anyhow::Result<Option<FlyVolumeBinding>> {
    let (volume, tenant, drive) = match (&args.volume, &args.volume_tenant, &args.volume_drive) {
        (None, None, None) => return Ok(None),
        (Some(volume), Some(tenant), Some(drive)) => (volume, tenant, drive),
        _ => anyhow::bail!("volume, volume tenant and volume drive must be configured together"),
    };
    let drive_id = DriveId {
        tenant: TenantId::new(tenant.as_str())?,
        name: DriveName::new(drive.as_str())?,
    };
    let volume_id = FlyVolumeId::new(volume.as_str())?;
    let mount_path = MountPath::new(args.volume_path.as_str())?;
    Ok(Some(FlyVolumeBinding::new(volume_id, drive_id, mount_path)?))
}

fn config(args: Args) -> anyhow::Result<FlyConfig> {
    let volume = volume_binding(&args)?;
    let workspace_volume = args
        .workspace_volume_size_gb
        .map(|size_gb| {
            FlyWorkspaceVolume::new(
                size_gb,
                Duration::from_secs(args.snapshot_poll_interval_secs),
                Duration::from_secs(args.snapshot_timeout_secs),
            )
        })
        .transpose()?;
    Ok(FlyConfig::new(FlyConfigParts {
        image: args.image,
        region: FlyRegion::new(args.region)?,
        state_dir: args.state_dir,
        boot_timeout: Duration::from_secs(args.boot_timeout_seconds),
        max_leases: args.max_leases,
        memory_mib: args.memory_mib,
        cpus: args.cpus,
        warm_pool_size: args.warm_pool_size,
        volume,
        workspace_volume,
        egress_proxy: args.egress_proxy,
        public_host: args.public_host,
        preview_network: args.preview_network,
    })?)
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .init();
    let args = Args::parse();
    let endpoint = FlyEndpoint::parse(&args.endpoint)?;
    let app = FlyAppName::new(args.app.as_str())?;
    let token = FlyApiToken::new(&args.token)?;
    let api = FlyApi::new(endpoint, app, token)?;
    let nats_url = args.nats_url.clone();
    let configured_worker = args.worker_id.clone();
    let config = config(args)?;
    register_state_dir(config.state_dir());
    let provider = Arc::new(FlyProvider::new(api, config)?);
    provider.recover().await?;
    provider.maintain_pool().await?;
    let worker_id = provider.dispatch_ledger().worker_id().to_string();
    anyhow::ensure!(
        configured_worker.is_none_or(|configured| configured == worker_id),
        "configured worker id differs from its durable identity"
    );
    let client = nats_connect::connect(&nats_url).await.context("connecting to NATS")?;
    FlyWorker::new(
        client,
        provider,
        AuthorityConfig::new(AuthorityMode::from_env(&SystemEnv)?),
    )
    .run(async {
        let _ = tokio::signal::ctrl_c().await;
    })
    .await?;
    Ok(())
}
