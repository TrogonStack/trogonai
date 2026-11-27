#![recursion_limit = "256"]

#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::AtomicU32;
use std::time::Duration;

use anyhow::Context as _;
use async_nats::jetstream;
use clap::Parser;
use tokio_util::sync::CancellationToken;
use tracing::{info, warn};
use trogon_sandbox_contract::{ProviderKind, WorkerId};
use trogon_sandbox_cosign::CosignVerifier;
use trogon_sandbox_docker_worker::heartbeat::{spawn_heartbeat_task, spawn_warm_pool_sweeper};
use trogon_sandbox_docker_worker::{adapter, browser, digest_cache, resources, warm_pool, worker};
use trogon_sandbox_oci::RegistryHost;
use trogon_sandbox_providers::ContainerHardening;
use trogon_sandbox_volumes::{VolumeEndpoint, VolumeRegion};
use trogon_std::env::{ReadEnv, SystemEnv};

#[derive(Parser, Debug)]
#[command(
    version,
    about = "Sandbox Docker worker: serves sandbox lease requests via local Docker"
)]
struct Args {
    #[arg(long, env = "SANDBOX_NATS_URL", default_value = "nats://127.0.0.1:4222")]
    nats_url: String,

    #[arg(
        long,
        env = "SANDBOX_DOCKER_WORKER_OWNER",
        help = "Owner identity labelled onto every container this worker creates. At startup the worker removes all containers carrying this owner, which are leftovers of a previous run. Overrides the owner persisted in --state-dir; strict targeted dispatch still uses that directory. Give each worker sharing a Docker daemon a distinct value. Unset: the owner is loaded from, or created in, --state-dir. Containers without an owner label (older versions) are never swept"
    )]
    owner: Option<String>,

    #[arg(
        long,
        env = "SANDBOX_DOCKER_STATE_DIR",
        default_value = "/var/lib/sandbox-docker",
        help = "Directory holding this worker's persisted owner id and strict targeted-dispatch ledger. Locked while the worker runs; give each worker sharing a Docker daemon its own directory. An explicit --owner overrides only the container owner identity"
    )]
    state_dir: PathBuf,

    #[arg(
        long,
        env = "SANDBOX_DOCKER_PERSIST_ROLLBACK_STATE",
        default_value_t = false,
        help = "Keep each lease's request under --state-dir so an in-place rollback still works after a worker restart. The request can carry env values, so the files are 0600 in a 0700 directory and are removed when the lease is cancelled. Off by default: rollback state stays in memory"
    )]
    persist_rollback_state: bool,

    #[arg(
        long,
        env = "SANDBOX_DOCKER_ROLLBACK_STATE_RETENTION_SECS",
        default_value_t = 604_800,
        help = "Seconds after which a persisted rollback entry that was not rewritten is removed at startup. Only used with --persist-rollback-state"
    )]
    rollback_state_retention_secs: u64,

    #[arg(long, env = "SANDBOX_IMAGE", default_value = "sandbox-agent:dev")]
    image: String,

    #[arg(
        long,
        env = "SANDBOX_DOCKER_COMMAND_SUPERVISOR",
        help = "Static Linux supervisor for command custody. Strong custody requires actual Docker CapDrop=ALL and excludes privileged containers and ALL, SYS_PTRACE or SYS_ADMIN additions. Incompatible with --keep-default-capabilities. This option does not advertise cancellation support"
    )]
    command_supervisor: Option<PathBuf>,

    #[arg(
        long,
        env = "SANDBOX_BROWSER_IMAGE",
        help = "Preloaded registry image with an explicit tag or digest. Browser support is advertised only after an offline Chromium screenshot and Node 22.7+ probe. Browser requests must name this exact image; launches use its verified immutable Docker image ID"
    )]
    browser_image: Option<String>,

    #[arg(long, env = "SANDBOX_EGRESS_PROXY_IMAGE")]
    egress_proxy_image: String,

    #[arg(long, env = "SANDBOX_MAX_CONCURRENT_LEASES", default_value_t = 4)]
    max_concurrent_leases: u32,

    #[arg(long, env = "SANDBOX_SNAPSHOT_REGISTRY", default_value = "127.0.0.1:5000")]
    snapshot_registry: String,

    #[arg(long, env = "SANDBOX_OCI_INSECURE_REGISTRIES", value_delimiter = ',')]
    oci_insecure_registries: Vec<String>,

    #[arg(
        long,
        env = "SANDBOX_COSIGN_PUBLIC_KEY",
        help = "PEM path for Cosign snapshot verification (required when restoring signed snapshots)"
    )]
    cosign_public_key: Option<std::path::PathBuf>,

    #[arg(long, env = "SANDBOX_VOLUME_S3_REGION", default_value = "us-east-1")]
    volume_s3_region: String,

    #[arg(long, env = "SANDBOX_VOLUME_S3_ENDPOINT")]
    volume_s3_endpoint: Option<String>,

    #[arg(long, env = "SANDBOX_DIGEST_CACHE_SIZE", default_value_t = 32)]
    digest_cache_size: usize,

    #[arg(long, env = "SANDBOX_WARM_POOL_ENABLED", default_value_t = true)]
    warm_pool_enabled: bool,

    #[arg(long, env = "SANDBOX_WARM_POOL_MAX_PER_DIGEST", default_value_t = 3)]
    warm_pool_max_per_digest: usize,

    #[arg(long, env = "SANDBOX_WARM_POOL_MAX_TOTAL", default_value_t = 16)]
    warm_pool_max_total: usize,

    #[arg(long, env = "SANDBOX_WARM_POOL_MAX_IDLE_SECONDS", default_value_t = 600)]
    warm_pool_max_idle_seconds: u64,

    /// Process and thread ceiling per sandbox container. 0 disables the limit.
    #[arg(long, env = "SANDBOX_DOCKER_PIDS_LIMIT", default_value_t = 4096)]
    pids_limit: i64,

    /// Keep Docker's default capability set instead of dropping all but a small allowlist.
    #[arg(long, env = "SANDBOX_DOCKER_KEEP_DEFAULT_CAPABILITIES", default_value_t = false)]
    keep_default_capabilities: bool,

    /// Allow setuid binaries to gain privileges inside sandboxes.
    #[arg(long, env = "SANDBOX_DOCKER_ALLOW_NEW_PRIVILEGES", default_value_t = false)]
    allow_new_privileges: bool,

    /// Mount the container root filesystem read-only (writable tmpfs at /tmp and /run).
    #[arg(long, env = "SANDBOX_DOCKER_READONLY_ROOTFS", default_value_t = false)]
    readonly_rootfs: bool,

    /// Allow containers to use swap beyond their memory limit.
    #[arg(long, env = "SANDBOX_DOCKER_ALLOW_SWAP", default_value_t = false)]
    allow_swap: bool,

    /// OCI runtime for sandbox containers, for example `runsc` or `kata`.
    #[arg(long, env = "SANDBOX_DOCKER_RUNTIME")]
    runtime: Option<String>,

    /// Size of /dev/shm in MiB.
    #[arg(long, env = "SANDBOX_DOCKER_SHM_MIB", default_value_t = 256)]
    shm_mib: u64,

    /// Largest vCPU count a lease may ask for. Defaults to the host's core
    /// count, because Docker refuses a container wanting more cores than exist.
    #[arg(long, env = "SANDBOX_DOCKER_MAX_VCPUS")]
    max_vcpus: Option<u32>,

    /// Largest per-lease disk quota in MiB. Unset means the worker claims no
    /// disk quota support. Set it only when the Docker storage driver accepts
    /// `--storage-opt size` (overlay2 on xfs with `pquota`, zfs, btrfs);
    /// otherwise container creation fails for every lease that asks for disk.
    #[arg(long, env = "SANDBOX_DOCKER_DISK_QUOTA_MAX_MIB")]
    disk_quota_max_mib: Option<u32>,

    /// User for workloads whose lease names none (for example `1000:1000`).
    #[arg(long, env = "SANDBOX_DOCKER_DEFAULT_USER")]
    default_user: Option<String>,

    #[command(flatten)]
    observability: trogon_sandbox_observability::ObservabilityArgs,
}

/// Whether `name` is set to `require` in the environment.
fn requires(env: &impl ReadEnv, name: &str) -> bool {
    env.var(name)
        .is_ok_and(|mode| mode.trim().eq_ignore_ascii_case("require"))
}

fn hardening_from_args(args: &Args) -> ContainerHardening {
    let mut hardening = ContainerHardening::secure();
    hardening.pids_limit = (args.pids_limit > 0).then_some(args.pids_limit);
    hardening.drop_all_capabilities = !args.keep_default_capabilities;
    hardening.no_new_privileges = !args.allow_new_privileges;
    hardening.readonly_rootfs = args.readonly_rootfs;
    hardening.disable_swap = !args.allow_swap;
    hardening.runtime = args.runtime.clone();
    hardening.shm_size_mib = (args.shm_mib > 0).then_some(args.shm_mib);
    if args.readonly_rootfs {
        hardening.tmpfs = ["/tmp", "/run"]
            .map(|path| (path.to_string(), "rw,nosuid,size=256m".to_string()))
            .to_vec();
    }
    hardening
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let args = Args::parse();
    let (_tracing, _registry) = trogon_sandbox_observability::bootstrap(
        "trogon-sandbox-docker-worker",
        trogon_sandbox_observability::DOCKER_WORKER,
        &args.observability,
    )?;

    let nats = trogon_sandbox_protocol::nats_connect::connect(&args.nats_url)
        .await
        .with_context(|| format!("failed to connect to NATS at {}", args.nats_url))?;
    let js = jetstream::new(nats.clone());

    if requires(&SystemEnv, trogon_sandbox_protocol::ENV_VERIFY_MODE)
        && requires(&SystemEnv, trogon_sandbox_protocol::ENV_FENCE_EPOCH)
    {
        std::fs::create_dir_all(&args.state_dir)?;
        #[cfg(unix)]
        {
            std::fs::set_permissions(&args.state_dir, std::fs::Permissions::from_mode(0o700))?;
        }
    }
    trogon_sandbox_protocol::register_state_dir(&args.state_dir);
    let (owner, _owner_lock) = match args.owner.clone() {
        Some(owner) => (owner, None),
        None => {
            let persisted = trogon_sandbox_docker_worker::owner_store::load_or_create(&args.state_dir)
                .with_context(|| format!("load worker owner from {}", args.state_dir.display()))?;
            (persisted.owner.clone(), Some(persisted))
        }
    };
    let dispatch_ledger = if trogon_sandbox_protocol::global_verifier()
        .fresh_command_launch_fence()
        .is_ok()
    {
        Some(Arc::new(trogon_sandbox_protocol::AttemptLedger::open(
            args.state_dir.join("dispatch.sqlite"),
            ProviderKind::LocalDocker,
            None,
        )?))
    } else {
        None
    };
    let inner = trogon_sandbox_providers::LocalDockerProvider::with_default_engine(args.egress_proxy_image.clone())?
        .with_owner(owner.clone())
        .with_violation_nats_url(args.nats_url.clone())
        .with_hardening(hardening_from_args(&args))
        .with_default_user(args.default_user.clone());

    let removed = inner
        .sweep_owned()
        .await
        .context("remove containers left by a previous run")?;
    info!(removed, "removed containers left by a previous run");
    let runtime_docker = bollard::Docker::connect_with_defaults()?;
    let runtime_removed = adapter::command_runtime::CommandRuntime::sweep_owned(&runtime_docker, &owner).await?;
    info!(
        runtime_removed,
        "removed unused command runtime artifacts from a previous run"
    );
    let command_runtime = args
        .command_supervisor
        .as_ref()
        .map(|path| adapter::command_runtime::CommandRuntime::load(path, owner.clone()))
        .transpose()?;

    let browser_support = if let Some(image) = args.browser_image.as_deref() {
        let runtime = browser::runtime::DockerBrowserProbeRuntime::new(&inner)?;
        Some(
            browser::verify_browser(
                &runtime,
                trogon_sandbox_contract::OciImageRef::parse_unqualified(image),
                inner.hardening(),
            )
            .await
            .context("verify configured browser image")?,
        )
    } else {
        None
    };

    let mut insecure = args.oci_insecure_registries;
    if !insecure.iter().any(|r| r == &args.snapshot_registry) {
        insecure.push(args.snapshot_registry.clone());
    }
    let oci_client = adapter::build_oci_client(
        insecure
            .iter()
            .map(RegistryHost::new)
            .collect::<Result<Vec<_>, _>>()
            .context("parse insecure registry host")?,
    );
    let cosign_verifier = args
        .cosign_public_key
        .as_ref()
        .map(|path| CosignVerifier::from_pem_path(path).map(|v| v.with_insecure_registries(insecure.clone())))
        .transpose()
        .context("load cosign public key")?;
    let digest_cache = Arc::new(digest_cache::DigestCache::new(args.digest_cache_size));
    let worker_id = match dispatch_ledger.as_ref() {
        Some(ledger) => ledger.worker_id().clone(),
        None => WorkerId::new(uuid::Uuid::new_v4().simple().to_string())?,
    };
    let current_leases = Arc::new(AtomicU32::new(0));

    let warm_pool_config = warm_pool::WarmPoolConfig {
        enabled: args.warm_pool_enabled,
        max_pool_size_per_digest: args.warm_pool_max_per_digest,
        max_total_warm: args.warm_pool_max_total,
        max_idle_seconds: args.warm_pool_max_idle_seconds,
    };

    let mut provider = adapter::DockerWorkerProvider::new(
        inner,
        args.image,
        js.clone(),
        args.nats_url.clone(),
        worker_id.to_string(),
        oci_client,
        args.snapshot_registry,
        cosign_verifier,
        Arc::clone(&digest_cache),
        Some(VolumeRegion::new(&args.volume_s3_region).context("parse the S3 volume region")?),
        args.volume_s3_endpoint
            .as_deref()
            .map(VolumeEndpoint::new)
            .transpose()
            .context("parse the S3 volume endpoint")?,
        warm_pool_config,
    )?
    .with_browser_support(browser_support);
    if let Some(ledger) = dispatch_ledger {
        provider = provider.with_dispatch_ledger(ledger)?;
    }
    if let Some(runtime) = command_runtime {
        provider = provider.with_command_runtime(runtime);
    }
    if args.persist_rollback_state {
        let store = trogon_sandbox_docker_worker::rollback_state::RollbackStateStore::open(
            &args.state_dir,
            Duration::from_secs(args.rollback_state_retention_secs),
        )
        .with_context(|| format!("open rollback state in {}", args.state_dir.display()))?;
        provider = provider.with_rollback_state(store);
    }
    let provider = Arc::new(provider);

    let provider_trait: Arc<dyn trogon_sandbox_protocol::Provider> =
        Arc::clone(&provider) as Arc<dyn trogon_sandbox_protocol::Provider>;

    let cancel = CancellationToken::new();
    let cancel_clone = cancel.clone();
    tokio::spawn(async move {
        if tokio::signal::ctrl_c().await.is_ok() {
            cancel_clone.cancel();
        }
    });

    let machine_resources = resources(
        args.max_vcpus
            .unwrap_or_else(|| std::thread::available_parallelism().map_or(1, |n| n.get() as u32)),
        args.disk_quota_max_mib,
    );

    spawn_heartbeat_task(
        nats.clone(),
        worker_id.clone(),
        machine_resources,
        args.max_concurrent_leases,
        Arc::clone(&current_leases),
        Arc::clone(&provider),
        cancel.clone(),
    );

    spawn_warm_pool_sweeper(Arc::clone(&provider), cancel.clone());

    info!("trogon-sandbox-docker-worker starting");
    let cleanup_provider = Arc::clone(&provider);
    let result = worker::run(
        nats,
        js,
        provider_trait,
        provider,
        worker_id.to_string(),
        cancel,
        current_leases,
    )
    .await;
    if let Err(error) = cleanup_provider.shutdown_command_runtime().await {
        warn!(error = %error, "command runtime cleanup requires the next owned-container sweep");
    }
    result
}
