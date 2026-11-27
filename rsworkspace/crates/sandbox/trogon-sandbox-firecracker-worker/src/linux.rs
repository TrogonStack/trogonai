use std::num::NonZeroU32;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use anyhow::Context as _;
use async_nats::jetstream;
use clap::Parser;
use tokio_util::sync::CancellationToken;
use tracing::info;
use trogon_sandbox_contract::OciImageRef;
use trogon_sandbox_firecracker_worker::FirecrackerProvider;
use trogon_sandbox_firecracker_worker::config::{
    BaseRootfs, EgressProxyBinary, FirecrackerBinary, FirecrackerConfig, KernelImage, MemoryMib, NetnsHelper,
    NetnsPrefix, NetworkConfig, RelayBinary, SnapshotConfig, SnapshotDir, StateDir, VcpuCount, VsockPort,
};
use trogon_sandbox_firecracker_worker::jailer::{JailerConfig, JailerLauncher};
use trogon_sandbox_firecracker_worker::process::{DirectLauncher, VmLauncher};
use trogon_sandbox_firecracker_worker::worker;

#[derive(Parser, Debug)]
#[command(
    version,
    about = "Sandbox Firecracker worker: serves sandbox lease requests as one Firecracker microVM per lease"
)]
struct Args {
    #[arg(long, env = "SANDBOX_NATS_URL", default_value = "nats://127.0.0.1:4222")]
    nats_url: String,

    #[arg(long, env = "SANDBOX_FIRECRACKER_BINARY", default_value = "firecracker")]
    firecracker_binary: PathBuf,

    /// Jailer executable. The worker refuses to start without it unless `--insecure-dev`
    /// is given.
    #[arg(long, env = "SANDBOX_FIRECRACKER_JAILER")]
    jailer: Option<PathBuf>,

    /// Development only: allow starting without `--jailer`, so the VMM runs as the
    /// worker's own user with no chroot, cgroup limits or UID separation.
    #[arg(long, env = "SANDBOX_INSECURE_DEV", default_value_t = false)]
    insecure_dev: bool,

    /// Start of a reserved UID range, one identity per concurrent guest.
    #[arg(long, env = "SANDBOX_FIRECRACKER_JAILER_UID_BASE", default_value = "200000")]
    jailer_uid_base: NonZeroU32,

    #[arg(long, env = "SANDBOX_FIRECRACKER_JAILER_GID_BASE", default_value = "200000")]
    jailer_gid_base: NonZeroU32,

    #[arg(long, env = "SANDBOX_FIRECRACKER_JAILER_CGROUP_PARENT", default_value = "sandbox-fc")]
    jailer_cgroup_parent: PathBuf,

    /// Host memory above the guest RAM budget for the VMM and its threads.
    #[arg(long, env = "SANDBOX_FIRECRACKER_JAILER_MEMORY_OVERHEAD_MIB", default_value_t = 128)]
    jailer_memory_overhead_mib: u32,

    /// Uncompressed guest kernel (`vmlinux`).
    #[arg(long, env = "SANDBOX_FIRECRACKER_KERNEL")]
    kernel: PathBuf,

    /// ext4 image with the guest agent installed. Copied once per lease.
    #[arg(long, env = "SANDBOX_FIRECRACKER_ROOTFS")]
    rootfs: PathBuf,

    /// One subdirectory per lease lives here. Keep the path short: it holds Unix sockets.
    #[arg(long, env = "SANDBOX_FIRECRACKER_STATE_DIR", default_value = "/var/lib/sandbox-fc")]
    state_dir: PathBuf,

    /// The only image a lease may name. The rootfs is fixed, so this is a label for it.
    #[arg(long, env = "SANDBOX_FIRECRACKER_IMAGE")]
    image: String,

    #[arg(long, env = "SANDBOX_FIRECRACKER_VCPUS", default_value_t = 2)]
    vcpus: u8,

    /// Memory for a lease that asks for none.
    #[arg(long, env = "SANDBOX_FIRECRACKER_DEFAULT_MEMORY_MIB", default_value_t = 512)]
    default_memory_mib: u32,

    /// Port the guest agent listens on over vsock.
    #[arg(long, env = "SANDBOX_FIRECRACKER_AGENT_VSOCK_PORT", default_value_t = 8110)]
    agent_vsock_port: u32,

    /// Budget for the API socket to appear and, separately, for the agent to answer.
    #[arg(long, env = "SANDBOX_FIRECRACKER_BOOT_TIMEOUT", default_value_t = 30)]
    boot_timeout_seconds: u64,

    #[arg(long, env = "SANDBOX_FIRECRACKER_MAX_CONCURRENT_LEASES", default_value_t = 8)]
    max_concurrent_leases: u32,

    /// Root helper (`scripts/fc-netns`) the worker may run through `sudo -n`. Setting it
    /// lets leases whose network policy allows egress boot with a network device; without
    /// it every guest boots with none and no host privilege is needed.
    #[arg(long, env = "SANDBOX_FIRECRACKER_NETNS_HELPER", group = "networking", conflicts_with_all = ["host_links", "jailer"])]
    netns_helper: Option<PathBuf>,

    /// The egress proxy each networked lease gets.
    #[arg(
        long,
        env = "SANDBOX_FIRECRACKER_EGRESS_PROXY_BINARY",
        default_value = "sandbox-egress-proxyd"
    )]
    egress_proxy_binary: PathBuf,

    /// Must match the prefix the helper was installed with.
    #[arg(long, env = "SANDBOX_FIRECRACKER_NETNS_PREFIX", default_value = "sbxfc-")]
    netns_prefix: String,

    /// Where checkpoints of running leases are kept, outside the state dir. Setting it
    /// lets leases be forked and restored on this host; each snapshot holds a copy of the
    /// guest's memory and root filesystem.
    #[arg(long, env = "SANDBOX_FIRECRACKER_SNAPSHOT_DIR")]
    snapshot_dir: Option<PathBuf>,

    /// How many snapshots to keep before the oldest is evicted.
    #[arg(long, env = "SANDBOX_FIRECRACKER_MAX_SNAPSHOTS", default_value_t = SnapshotConfig::DEFAULT_KEEP)]
    max_snapshots: std::num::NonZeroUsize,
    #[arg(long, env = "SANDBOX_FIRECRACKER_HOST_LINKS", group = "networking")]
    host_links: Option<ipnet::Ipv4Net>,

    /// Lets leases inject broker-issued credentials through the egress proxy's TLS
    /// interception (policy `Transform` rules). The guest trusts the proxy's CA before the
    /// lease is ready, and a lease whose CA install fails does not start. Needs
    /// `--host-links` or `--netns-helper`; with the helper, every lease's proxy runs
    /// inside the worker instead of as a `sandbox-egress-proxyd` process.
    #[arg(
        long,
        env = "SANDBOX_FIRECRACKER_CREDENTIAL_BROKER_NATS_URL",
        requires = "networking"
    )]
    credential_broker_nats_url: Option<String>,

    /// Lets leases forward matching requests to another service under their OIDC identity
    /// (policy `Forward` rules). The egress proxy on the host signs the tokens with this
    /// PKCS#8 PEM EC key, which never reaches the guest; a receiver trusts its public half.
    /// Needs `--host-links`.
    #[arg(long, env = "SANDBOX_FIRECRACKER_OIDC_SIGNING_KEY_PEM_FILE", requires = "host_links")]
    oidc_signing_key_pem_file: Option<PathBuf>,

    /// Refuse a lease that needs credentials when the control plane has not committed a
    /// credential policy for it. Off, such a lease runs with unfenced credential requests.
    /// Needs `--credential-broker-nats-url`.
    #[arg(
        long,
        env = "SANDBOX_REQUIRE_COMMITTED_POLICY",
        default_value_t = false,
        requires = "credential_broker_nats_url"
    )]
    require_committed_policy: bool,

    #[arg(long, env = "SANDBOX_OCI_INSECURE_REGISTRIES", value_delimiter = ',')]
    insecure_registries: Vec<trogon_sandbox_oci::RegistryHost>,

    #[arg(long, env = "SANDBOX_FIRECRACKER_WARM_POOL_MAX_SNAPSHOTS", default_value_t = 0)]
    warm_pool_max_snapshots: usize,

    #[arg(long, env = "SANDBOX_FIRECRACKER_WARM_POOL_MAX_BYTES", default_value_t = 4 * 1024 * 1024 * 1024)]
    warm_pool_max_bytes: u64,

    #[arg(long, env = "SANDBOX_FIRECRACKER_WARM_POOL_MAX_IDLE_SECONDS", default_value_t = 300)]
    warm_pool_max_idle_seconds: u64,

    #[arg(long, env = "SANDBOX_FIRECRACKER_RELEASED_MAX_BYTES", default_value_t = 4 * 1024 * 1024 * 1024)]
    released_max_bytes: u64,

    #[arg(long, env = "SANDBOX_FIRECRACKER_RELEASED_MAX_RECORDS", default_value_t = 256)]
    released_max_records: usize,

    #[arg(long, env = "SANDBOX_FIRECRACKER_RELEASED_TTL_SECONDS", default_value_t = 600)]
    released_ttl_seconds: u64,

    #[arg(long, env = "SANDBOX_FIRECRACKER_SNAPSHOT_MAX_ROOTFS_BYTES", default_value_t = 4 * 1024 * 1024 * 1024)]
    snapshot_max_rootfs_bytes: u64,

    #[arg(long, env = "SANDBOX_FIRECRACKER_OCI_GUEST_AGENT", requires = "oci_guest_init")]
    oci_guest_agent: Option<PathBuf>,

    #[arg(long, env = "SANDBOX_FIRECRACKER_OCI_GUEST_INIT", requires = "oci_guest_agent")]
    oci_guest_init: Option<PathBuf>,

    #[arg(
        long,
        env = "SANDBOX_FIRECRACKER_OCI_CACHE_DIR",
        default_value = "/var/cache/sandbox-fc-images"
    )]
    oci_cache_dir: PathBuf,

    #[arg(long, env = "SANDBOX_FIRECRACKER_UMOCI", default_value = "umoci")]
    umoci: PathBuf,

    #[arg(long, env = "SANDBOX_FIRECRACKER_MKFS_EXT4", default_value = "/usr/sbin/mkfs.ext4")]
    mkfs_ext4: PathBuf,

    #[arg(long, env = "SANDBOX_FIRECRACKER_OCI_ROOTFS_SIZE_MIB", default_value_t = 2048)]
    oci_rootfs_size_mib: u64,

    #[arg(long, env = "SANDBOX_FIRECRACKER_OCI_CACHE_MAX_BYTES", default_value_t = 8 * 1024 * 1024 * 1024)]
    oci_cache_max_bytes: u64,

    #[arg(long, env = "SANDBOX_FIRECRACKER_OCI_MAX_DOWNLOAD_BYTES", default_value_t = 1024 * 1024 * 1024)]
    oci_max_download_bytes: u64,

    /// Static CPU template for guests (`C3`, `T2`, ...). Part of the environment digest.
    #[arg(long, env = "SANDBOX_FIRECRACKER_CPU_TEMPLATE")]
    cpu_template: Option<String>,

    /// Environment digests this node may run (node execution policy). The worker refuses
    /// to start when its own digest is not listed. Empty approves its own digest only.
    #[arg(long, env = "SANDBOX_FIRECRACKER_APPROVED_ENV_DIGESTS", value_delimiter = ',')]
    approved_env_digests: Vec<trogon_sandbox_contract::EnvDigest>,

    /// Refuse to restore snapshots that record no environment digest. Off by default so
    /// snapshots taken before the digest existed keep restoring, with a warning.
    #[arg(long, env = "SANDBOX_FIRECRACKER_STRICT_SNAPSHOT_ENV")]
    strict_snapshot_env: bool,

    /// PEM public key that verifies the cosign signature, and the environment digest it
    /// signs, of a snapshot pulled from the registry.
    #[arg(long, env = "SANDBOX_COSIGN_PUBLIC_KEY")]
    cosign_public_key: Option<std::path::PathBuf>,

    /// What a restore does when that verification fails: `off`, `warn` (log and restore
    /// anyway) or `require` (refuse). `require` needs a public key.
    #[arg(long, env = "SANDBOX_COSIGN_VERIFY", default_value = "warn")]
    cosign_verify: trogon_sandbox_cosign::VerifyMode,

    #[command(flatten)]
    observability: trogon_sandbox_observability::ObservabilityArgs,
}

/// The jailer is what confines a compromised VMM, so a worker without one only starts
/// when the operator opted in with `--insecure-dev`.
fn require_jailer(jailer: Option<&PathBuf>, insecure_dev: bool) -> anyhow::Result<()> {
    match (jailer, insecure_dev) {
        (Some(_), _) => Ok(()),
        (None, true) => {
            tracing::warn!(
                "INSECURE DEV MODE: no --jailer, so the Firecracker VMM runs directly as the \
                 worker user without a chroot, cgroup limits or a per-guest UID; a guest \
                 escape lands in the worker. Never use --insecure-dev outside development"
            );
            Ok(())
        }
        (None, false) => anyhow::bail!(
            "--jailer / SANDBOX_FIRECRACKER_JAILER is required; pass --insecure-dev to run \
             the VMM directly as the worker user for local development"
        ),
    }
}

pub async fn main() -> anyhow::Result<()> {
    let args = Args::parse();
    let (_tracing, registry) = trogon_sandbox_observability::bootstrap(
        "trogon-sandbox-firecracker-worker",
        trogon_sandbox_observability::FIRECRACKER_WORKER,
        &args.observability,
    )?;

    require_jailer(args.jailer.as_ref(), args.insecure_dev)?;

    trogon_sandbox_protocol::register_state_dir(&args.state_dir);
    let mut config = FirecrackerConfig::new(
        FirecrackerBinary::new(args.firecracker_binary),
        // The VMM runs inside each lease directory, so the paths it is handed must not
        // depend on the worker's working directory.
        KernelImage::new(std::path::absolute(&args.kernel).context("resolving --kernel")?),
        BaseRootfs::new(args.rootfs),
        StateDir::new(args.state_dir),
    );
    config.default_vcpus = VcpuCount::new(args.vcpus)?;
    config.default_memory = MemoryMib::new(args.default_memory_mib)?;
    config.agent_port = VsockPort::new(args.agent_vsock_port)?;
    config.boot_timeout = Duration::from_secs(args.boot_timeout_seconds);
    config.default_image = OciImageRef::parse_unqualified(&args.image);
    config.max_concurrent_leases = args.max_concurrent_leases;
    trogon_sandbox_firecracker_worker::spoof_metrics::register(&registry)?;
    if let Some(helper) = args.netns_helper {
        trogon_sandbox_firecracker_worker::verify_helper(&NetnsHelper::new(helper.clone())).await?;
        config.legacy_network = Some(NetworkConfig {
            helper: NetnsHelper::new(helper),
            proxy_binary: EgressProxyBinary::new(args.egress_proxy_binary),
            relay_binary: RelayBinary::new(std::env::current_exe().context("locating the worker executable")?),
            prefix: NetnsPrefix::new(args.netns_prefix)?,
        });
    }

    if let Some(dir) = args.snapshot_dir {
        let dir = std::path::absolute(&dir).context("resolving --snapshot-dir")?;
        let state = std::path::absolute(config.state_dir.as_path()).context("resolving --state-dir")?;
        anyhow::ensure!(
            !dir.starts_with(&state),
            "--snapshot-dir must be outside --state-dir, whose every subdirectory is reclaimed on startup"
        );
        config.snapshots = Some(SnapshotConfig {
            dir: SnapshotDir::new(dir),
            keep: args.max_snapshots,
        });
    }
    config.network = args
        .host_links
        .map(|host_links| trogon_sandbox_firecracker_worker::network::NetworkConfig { host_links });
    config.credential_broker_nats_url = args.credential_broker_nats_url;
    config.oidc_signing_key_pem_file = args.oidc_signing_key_pem_file;
    if let Some(path) = &config.oidc_signing_key_pem_file {
        let pem = std::fs::read_to_string(path).context("reading --oidc-signing-key-pem-file")?;
        trogon_sandbox_egress_proxy::oidc_signer::OidcSigner::from_ec_pkcs8_pem(&pem)
            .context("loading --oidc-signing-key-pem-file")?;
    }
    if let Some(network) = &config.network {
        network.validate(config.max_concurrent_leases)?;
    }
    config.warm_pool = trogon_sandbox_firecracker_worker::warm_pool::WarmPoolConfig {
        max_snapshots: args.warm_pool_max_snapshots,
        max_bytes: args.warm_pool_max_bytes,
        max_idle: Duration::from_secs(args.warm_pool_max_idle_seconds),
    };
    config.released_snapshots = trogon_sandbox_firecracker_worker::retained::RetentionConfig {
        max_bytes: args.released_max_bytes,
        max_records: args.released_max_records,
        export_ttl: Duration::from_secs(args.released_ttl_seconds),
    };
    config.snapshot_rootfs_max_bytes = args.snapshot_max_rootfs_bytes;
    config.oci_rootfs = match (args.oci_guest_agent, args.oci_guest_init) {
        (Some(guest_agent), Some(guest_init)) => Some(trogon_sandbox_firecracker_worker::rootfs::OciRootfsConfig {
            cache_dir: args.oci_cache_dir,
            guest_agent,
            guest_init,
            umoci: args.umoci,
            mkfs_ext4: args.mkfs_ext4,
            disk_size_bytes: args
                .oci_rootfs_size_mib
                .checked_mul(1024 * 1024)
                .context("OCI root disk size overflows u64")?,
            max_cache_bytes: args.oci_cache_max_bytes,
            max_download_bytes: args.oci_max_download_bytes,
        }),
        (None, None) => None,
        _ => anyhow::bail!("both OCI guest agent and guest init must be configured"),
    };

    config.cpu_template = args.cpu_template;
    config.restore_signature = trogon_sandbox_firecracker_worker::restore_signature::RestoreSignature {
        verifier: args
            .cosign_public_key
            .as_deref()
            .map(trogon_sandbox_cosign::CosignVerifier::from_pem_path)
            .transpose()
            .context("load cosign public key")?
            .map(|verifier| {
                verifier.with_insecure_registries(args.insecure_registries.iter().map(ToString::to_string).collect())
            }),
        mode: args.cosign_verify,
    };
    config.restore_signature.validate()?;
    let env_digest = trogon_sandbox_firecracker_worker::environment::install(
        &mut config,
        args.jailer.as_deref(),
        args.approved_env_digests,
        if args.strict_snapshot_env {
            trogon_sandbox_contract::LegacySnapshotPolicy::Refuse
        } else {
            trogon_sandbox_contract::LegacySnapshotPolicy::AllowWithWarning
        },
        &trogon_std::env::SystemEnv,
    )
    .await?;
    info!(%env_digest, "execution environment approved");

    let nats = async_nats::connect(&args.nats_url)
        .await
        .with_context(|| format!("failed to connect to NATS at {}", args.nats_url))?;
    let js = jetstream::new(nats.clone());

    let launcher: Arc<dyn VmLauncher> = match args.jailer {
        Some(binary) => Arc::new(JailerLauncher::new(
            JailerConfig {
                binary,
                uid_base: args.jailer_uid_base,
                gid_base: args.jailer_gid_base,
                cgroup_parent: args.jailer_cgroup_parent,
                memory_overhead: MemoryMib::new(args.jailer_memory_overhead_mib)?,
            },
            &config,
        )?),
        None => Arc::new(DirectLauncher::new(config.binary.clone())),
    };
    let committed_fence = if config.injects_credentials() {
        Some(trogon_sandbox_credential_broker::CommittedFence::new(
            Arc::new(trogon_sandbox_credential_broker::NatsKvCommittedPolicies::new(
                trogon_sandbox_credential_broker::NatsKvCommittedPolicies::open_bucket(&js).await?,
            )),
            args.require_committed_policy,
        ))
    } else {
        None
    };
    let mut provider = FirecrackerProvider::new(config, launcher);
    if let Some(fence) = committed_fence {
        provider = provider.with_committed_fence(fence);
    }
    let provider = Arc::new(
        provider
            .with_oci_client(Arc::new(trogon_sandbox_oci::OciClient::new(
                trogon_sandbox_oci::OciClientConfig {
                    insecure_registries: args.insecure_registries,
                    ..Default::default()
                },
            )))
            .with_lifecycle_stream(js.clone())
            .with_loss_sink(Arc::new(worker::NatsLeaseLoss::new(nats.clone(), js.clone()))),
    );

    let cleaned = provider.recover_stale().await?;
    info!(cleaned, "stale lease directories removed");

    let worker_id = provider.dispatch_ledger().await?.worker_id().clone();
    let cancel = CancellationToken::new();
    let cancel_clone = cancel.clone();
    tokio::spawn(async move {
        if tokio::signal::ctrl_c().await.is_ok() {
            cancel_clone.cancel();
        }
    });

    info!(%worker_id, "trogon-sandbox-firecracker-worker starting");
    let result = worker::run(
        nats,
        js,
        Arc::clone(&provider),
        worker_id,
        cancel,
        &trogon_std::env::SystemEnv,
    )
    .await;
    // VMs do not outlive the worker: nothing adopts them on restart.
    provider.shutdown().await;
    provider.clear_warm_pool().await;
    result
}

#[cfg(test)]
mod tests;
