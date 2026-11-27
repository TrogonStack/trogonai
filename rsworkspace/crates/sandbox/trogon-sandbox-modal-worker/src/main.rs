use std::path::PathBuf;
use std::sync::Arc;

use anyhow::Context;
use clap::Parser;
use trogon_sandbox_contract::{PullSecretId, WorkerId};
use trogon_sandbox_modal_worker::{
    BrokeredVolumeStores, CpuCores, DEFAULT_MAX_VCPUS, ModalAppName, ModalConfig, ModalProvider, ModalWorker,
    PythonBridgeModalClient, VolumeSettings,
};
use trogon_sandbox_observability::{MODAL_WORKER, ObservabilityArgs, bootstrap};
use trogon_sandbox_protocol::{AuthorityConfig, AuthorityMode, nats_connect, register_fence_file};
use trogon_sandbox_volumes::{VolumeEndpoint, VolumeRegion};
use trogon_std::env::SystemEnv;

#[derive(Parser)]
struct Args {
    #[arg(long, env = "TROGON_SANDBOX_NATS_URL", default_value = "nats://127.0.0.1:4222")]
    nats_url: String,
    /// Where the lease fence is journaled; kept in memory when unset.
    #[arg(long, env = "TROGON_SANDBOX_DISPATCH_FENCE_FILE")]
    dispatch_fence_file: Option<PathBuf>,
    #[arg(long, env = "TROGON_SANDBOX_MODAL_APP_NAME", default_value = "trogon-sandboxes")]
    app_name: ModalAppName,
    #[arg(long, env = "TROGON_SANDBOX_MODAL_ENVIRONMENT")]
    environment: Option<String>,
    /// Cores a sandbox gets when its lease names none.
    #[arg(long, env = "TROGON_SANDBOX_MODAL_CPU_CORES")]
    cpu_cores: Option<CpuCores>,
    #[arg(long, env = "TROGON_SANDBOX_MAX_CONCURRENT_LEASES", default_value_t = 4)]
    max_concurrent_leases: u32,
    #[arg(long, env = "TROGON_SANDBOX_MODAL_REGIONS", value_delimiter = ',')]
    regions: Vec<String>,
    #[arg(long, env = "TROGON_SANDBOX_MODAL_MAX_VCPUS", default_value_t = DEFAULT_MAX_VCPUS)]
    max_vcpus: u32,
    /// The interpreter that has the `modal` package installed.
    #[arg(long, env = "TROGON_SANDBOX_MODAL_PYTHON", default_value = "python3")]
    python: String,
    #[arg(
        long,
        env = "TROGON_SANDBOX_MODAL_VOLUME_CREDENTIAL_ID",
        default_value = "modal-volume"
    )]
    volume_credential_id: String,
    #[arg(long, env = "TROGON_SANDBOX_MODAL_VOLUME_REGION", default_value = "us-east-1")]
    volume_region: String,
    #[arg(long, env = "TROGON_SANDBOX_MODAL_VOLUME_ENDPOINT")]
    volume_endpoint: Option<String>,
    #[command(flatten)]
    observability: ObservabilityArgs,
}

fn config(args: &Args) -> anyhow::Result<ModalConfig> {
    let endpoint = args
        .volume_endpoint
        .as_deref()
        .filter(|endpoint| !endpoint.is_empty())
        .map(VolumeEndpoint::new)
        .transpose()?;
    Ok(ModalConfig {
        app_name: args.app_name.clone(),
        environment: args.environment.clone().filter(|environment| !environment.is_empty()),
        default_cpu_cores: args.cpu_cores,
        max_vcpus: args.max_vcpus,
        regions: args.regions.clone(),
        max_concurrent_leases: args.max_concurrent_leases,
        volume: VolumeSettings {
            region: VolumeRegion::new(&args.volume_region)?,
            endpoint,
            credential_id: PullSecretId::new(args.volume_credential_id.as_str()),
        },
    })
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let args = Args::parse();
    register_fence_file(args.dispatch_fence_file.as_deref());
    let _guards = bootstrap("trogon-sandbox-modal-worker", MODAL_WORKER, &args.observability)?;
    let config = config(&args)?;
    let client = nats_connect::connect(&args.nats_url)
        .await
        .context("connecting to NATS")?;
    let api = Arc::new(PythonBridgeModalClient::new(args.python.as_str())?);
    let stores = Arc::new(BrokeredVolumeStores::new(client.clone(), config.volume.clone()));
    let provider = Arc::new(ModalProvider::new(api, stores, config));
    let worker_id = WorkerId::new(uuid::Uuid::new_v4().to_string())?;
    let health = args.observability.health().clone();
    health.started_with_nats(&client);
    ModalWorker::new(
        client,
        provider,
        worker_id,
        AuthorityConfig::new(AuthorityMode::from_env(&SystemEnv)?),
    )
    .run(async {
        let _ = tokio::signal::ctrl_c().await;
        health.begin_shutdown();
    })
    .await?;
    Ok(())
}
