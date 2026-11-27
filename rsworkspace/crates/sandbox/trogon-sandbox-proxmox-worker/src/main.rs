use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use anyhow::Context;
use clap::Parser;
use trogon_sandbox_protocol::{AuthorityConfig, AuthorityMode, nats_connect, register_state_dir};
use trogon_sandbox_proxmox_worker::{
    AntiSpoof, ProxmoxApi, ProxmoxConfig, ProxmoxConfigParts, ProxmoxProvider, ProxmoxWorker, ProxyNetwork,
    ResourcePoolId, VmId,
};
use trogon_std::env::SystemEnv;

#[derive(Parser)]
struct Args {
    /// Leases one VM, checks command execution, file transfer and removal against the host, prints a JSON line per
    /// phase and exits without connecting to NATS.
    #[arg(long)]
    spike: bool,
    #[arg(long, env = "TROGON_SANDBOX_NATS_URL", default_value = "nats://127.0.0.1:4222")]
    nats_url: String,
    #[arg(long, env = "TROGON_SANDBOX_PROXMOX_URL")]
    endpoint: url::Url,
    #[arg(long, env = "TROGON_SANDBOX_PROXMOX_NODE")]
    node: String,
    #[arg(long, env = "TROGON_SANDBOX_PROXMOX_POOL")]
    pool: Option<ResourcePoolId>,
    #[arg(long, env = "TROGON_SANDBOX_PROXMOX_TOKEN_ID")]
    token_id: String,
    #[arg(long, env = "TROGON_SANDBOX_PROXMOX_TOKEN_SECRET", hide_env_values = true)]
    token_secret: String,
    #[arg(long, env = "TROGON_SANDBOX_PROXMOX_TEMPLATE")]
    template: VmId,
    #[arg(long, env = "TROGON_SANDBOX_PROXMOX_IMAGE")]
    image: String,
    #[arg(
        long,
        env = "TROGON_SANDBOX_PROXMOX_STATE_DIR",
        default_value = "/var/lib/trogon-sandbox-proxmox"
    )]
    state_dir: PathBuf,
    #[arg(long, default_value_t = 4)]
    max_leases: usize,
    #[arg(long, default_value_t = 120)]
    boot_timeout_seconds: u64,
    #[arg(long, default_value_t = 120)]
    task_timeout_seconds: u64,
    #[arg(long, env = "TROGON_SANDBOX_WORKER_ID")]
    worker_id: Option<String>,
    #[arg(long, env = "TROGON_SANDBOX_PROXMOX_PROXY_BRIDGE")]
    proxy_bridge: Option<String>,
    #[arg(long, env = "TROGON_SANDBOX_PROXMOX_GUEST_ADDRESSES", value_delimiter = ',')]
    guest_addresses: Vec<ipnet::Ipv4Net>,
    #[arg(long, env = "TROGON_SANDBOX_PROXMOX_GUEST_GATEWAY")]
    guest_gateway: Option<std::net::Ipv4Addr>,
    #[arg(long, env = "TROGON_SANDBOX_PROXMOX_PROXY_BIND")]
    proxy_bind: Option<std::net::Ipv4Addr>,
    #[arg(long, env = "TROGON_SANDBOX_PROXMOX_PROXY_ADDRESS")]
    proxy_address: Option<std::net::Ipv4Addr>,
    /// `require` fails a lease whose guest NIC does not report `ipfilter`, `macfilter` and `firewall=1` as
    /// active; `warn` logs and boots anyway.
    #[arg(long, env = "TROGON_SANDBOX_PROXMOX_ANTISPOOF", value_enum, default_value_t)]
    antispoof: AntiSpoof,
}

fn proxy_network(args: &Args) -> anyhow::Result<Option<ProxyNetwork>> {
    match (
        &args.proxy_bridge,
        args.guest_gateway,
        args.proxy_bind,
        args.proxy_address,
    ) {
        (Some(bridge), Some(gateway), Some(bind_address), Some(advertised_address)) => Ok(Some(ProxyNetwork {
            bridge: bridge.clone(),
            gateway,
            bind_address,
            advertised_address,
            addresses: args.guest_addresses.clone(),
            antispoof: args.antispoof,
        })),
        (None, None, None, None) if args.guest_addresses.is_empty() => Ok(None),
        _ => anyhow::bail!("configure all proxy network settings together, including a guest address pool"),
    }
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .init();
    let args = Args::parse();
    let network = proxy_network(&args)?;
    let api = ProxmoxApi::new(
        args.endpoint,
        args.node,
        &args.token_id,
        &args.token_secret,
        Duration::from_secs(args.task_timeout_seconds),
    )?
    .with_pool(args.pool);
    let config = ProxmoxConfig::new(ProxmoxConfigParts {
        template: args.template,
        image: args.image,
        state_dir: args.state_dir,
        boot_timeout: Duration::from_secs(args.boot_timeout_seconds),
        max_leases: args.max_leases,
        network,
    })?;
    register_state_dir(config.state_dir());
    let provider = Arc::new(ProxmoxProvider::new(api, config)?);
    provider.recover().await?;
    let worker_id = provider.dispatch_ledger().worker_id().to_string();
    anyhow::ensure!(
        args.worker_id.is_none_or(|configured| configured == worker_id),
        "configured worker id differs from its durable identity"
    );
    if args.spike {
        provider.spike(&mut std::io::stdout()).await?;
        return Ok(());
    }
    let client = nats_connect::connect(&args.nats_url)
        .await
        .context("connecting to NATS")?;
    ProxmoxWorker::new(
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
