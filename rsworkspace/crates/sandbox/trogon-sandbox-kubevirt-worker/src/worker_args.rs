use std::path::PathBuf;

use clap::Parser;
use trogon_sandbox_contract::{S3BucketName, WorkerId};
use trogon_sandbox_cosign::{CosignError, CosignVerifier};
use trogon_sandbox_observability::ObservabilityArgs;
use trogon_sandbox_oci::{OciClientConfig, RegistryHost};
use trogon_sandbox_volumes::{VolumeBackend, VolumeClientConfig, VolumeEndpoint, VolumeRegion};

use crate::{KubeVirtConfig, WarmPoolConfig};

/// Command line and environment configuration of the worker.
#[derive(Parser, Debug)]
#[command(
    version,
    about = "Sandbox KubeVirt worker: serves sandbox lease requests by launching KubeVirt VMIs"
)]
pub struct WorkerArgs {
    /// Directory the worker keeps durable custody of the resources it creates in.
    #[arg(
        long,
        env = "SANDBOX_KUBEVIRT_STATE_DIR",
        default_value = "/var/lib/sandbox-kubevirt/state"
    )]
    pub state_dir: PathBuf,

    /// Journal for the tenant and attempt each lease belongs to, so the dispatch fence survives a restart. Unset
    /// keeps it in memory.
    #[arg(long, env = "SANDBOX_DISPATCH_FENCE_FILE")]
    pub dispatch_fence_file: Option<PathBuf>,

    #[arg(long, env = "SANDBOX_NATS_URL", default_value = "nats://127.0.0.1:4222")]
    pub nats_url: String,

    #[arg(long, env = "SANDBOX_K8S_NAMESPACE")]
    pub k8s_namespace: String,

    #[arg(long, env = "SANDBOX_KUBEVIRT_IMAGE")]
    pub image: String,

    #[arg(long, env = "SANDBOX_KUBEVIRT_STORAGE_CLASS")]
    pub storage_class: Option<String>,

    #[arg(long, env = "SANDBOX_SNAPSHOT_REGISTRY", default_value = "localhost:5000")]
    pub snapshot_registry: RegistryHost,

    #[arg(long, env = "SANDBOX_WORKER_ID", default_value = "kubevirt-0")]
    pub worker_id: WorkerId,

    #[arg(long, env = "SANDBOX_MAX_CONCURRENT_LEASES", default_value_t = 4)]
    pub max_concurrent_leases: u32,

    #[arg(long, env = "SANDBOX_KUBEVIRT_DISK_GI", default_value_t = 10)]
    pub disk_size_gi: u32,

    #[arg(long, env = "SANDBOX_KUBEVIRT_BOOTSTRAP_SSH_PUBKEY")]
    pub bootstrap_ssh_pubkey: Option<String>,

    #[arg(long, env = "SANDBOX_KUBEVIRT_S3_SECRET")]
    pub s3_secret_name: Option<String>,

    #[arg(long, env = "SANDBOX_KUBEVIRT_RESTORE_PREFIX", default_value = "kubevirt-restore/")]
    pub restore_bucket_prefix: String,

    #[arg(long, env = "SANDBOX_VOLUME_S3_BUCKET")]
    pub volume_s3_bucket: Option<S3BucketName>,

    #[arg(long, env = "SANDBOX_VOLUME_S3_REGION", default_value = "us-east-1")]
    pub volume_s3_region: VolumeRegion,

    #[arg(long, env = "SANDBOX_VOLUME_S3_ENDPOINT")]
    pub volume_s3_endpoint: Option<VolumeEndpoint>,

    #[arg(long, env = "SANDBOX_OCI_INSECURE_REGISTRIES", value_delimiter = ',')]
    pub oci_insecure_registries: Vec<RegistryHost>,

    #[arg(
        long,
        env = "SANDBOX_COSIGN_PUBLIC_KEY",
        help = "PEM path for Cosign snapshot verification"
    )]
    pub cosign_public_key: Option<PathBuf>,

    /// Guest memory every warm VMI boots with. Leases asking for more cold start, so raise it when most leases
    /// need more than the default.
    #[arg(
        long,
        env = "SANDBOX_KUBEVIRT_WARM_MEMORY_MIB",
        default_value_t = WarmPoolConfig::default().warm_memory_mib,
        value_parser = clap::value_parser!(u32).range(1..)
    )]
    pub warm_memory_mib: u32,

    /// Whether this worker keeps warm VMIs at all. Off, every lease cold starts and no idle VMI holds cluster
    /// capacity.
    #[arg(
        long,
        env = "SANDBOX_KUBEVIRT_WARM_POOL_ENABLED",
        default_value_t = WarmPoolConfig::default().enabled,
        action = clap::ArgAction::Set
    )]
    pub warm_pool_enabled: bool,

    /// Most idle warm VMIs kept for one snapshot digest. To keep none, turn the pool off instead.
    #[arg(
        long,
        env = "SANDBOX_KUBEVIRT_WARM_POOL_MAX_PER_DIGEST",
        default_value_t = WarmPoolConfig::default().max_pool_size_per_digest,
        value_parser = clap::value_parser!(u32).range(1..)
    )]
    pub warm_pool_max_per_digest: u32,

    /// Most warm VMIs this worker keeps across every digest.
    #[arg(
        long,
        env = "SANDBOX_KUBEVIRT_WARM_POOL_MAX_TOTAL",
        default_value_t = WarmPoolConfig::default().max_total_warm,
        value_parser = clap::value_parser!(u32).range(1..)
    )]
    pub warm_pool_max_total: u32,

    /// Seconds an idle warm VMI is kept before it is deleted.
    #[arg(
        long,
        env = "SANDBOX_KUBEVIRT_WARM_POOL_MAX_IDLE_SECS",
        default_value_t = WarmPoolConfig::default().max_idle_seconds,
        value_parser = clap::value_parser!(u64).range(1..)
    )]
    pub warm_pool_max_idle_secs: u64,

    #[command(flatten)]
    pub observability: ObservabilityArgs,
}

impl WorkerArgs {
    /// The warm pool policy the flags describe.
    pub fn warm_pool_config(&self) -> WarmPoolConfig {
        WarmPoolConfig {
            enabled: self.warm_pool_enabled,
            max_pool_size_per_digest: self.warm_pool_max_per_digest,
            max_total_warm: self.warm_pool_max_total,
            max_idle_seconds: self.warm_pool_max_idle_secs,
            warm_memory_mib: self.warm_memory_mib,
        }
    }

    /// The registries reached over plain HTTP: the configured ones and the snapshot registry itself.
    pub fn insecure_registries(&self) -> Vec<RegistryHost> {
        let mut insecure = self.oci_insecure_registries.clone();
        if !insecure.contains(&self.snapshot_registry) {
            insecure.push(self.snapshot_registry.clone());
        }
        insecure
    }

    /// The S3 volume store, when a bucket is configured.
    pub fn volume_config(&self) -> Option<VolumeClientConfig> {
        self.volume_s3_bucket.as_ref().map(|bucket| VolumeClientConfig {
            backend: VolumeBackend::S3 {
                bucket: bucket.clone(),
                region: self.volume_s3_region.clone(),
                endpoint: self.volume_s3_endpoint.clone(),
            },
            kms_key_id: None,
        })
    }

    /// The runtime configuration of the worker. Fails when the Cosign public key cannot be loaded.
    pub fn kube_virt_config(&self) -> Result<KubeVirtConfig, CosignError> {
        let insecure = self.insecure_registries();
        let cosign_verifier = self
            .cosign_public_key
            .as_ref()
            .map(|path| {
                CosignVerifier::from_pem_path(path).map(|verifier| {
                    verifier.with_insecure_registries(insecure.iter().map(ToString::to_string).collect())
                })
            })
            .transpose()?;
        Ok(KubeVirtConfig {
            namespace: self.k8s_namespace.clone(),
            default_image: self.image.clone(),
            storage_class: self.storage_class.clone(),
            snapshot_registry: self.snapshot_registry.to_string(),
            worker_id: self.worker_id.clone(),
            disk_size_gi: self.disk_size_gi,
            bootstrap_ssh_pubkey: self.bootstrap_ssh_pubkey.clone(),
            s3_secret_name: self.s3_secret_name.clone(),
            restore_bucket_prefix: self.restore_bucket_prefix.clone(),
            oci: OciClientConfig {
                insecure_registries: insecure,
                ..Default::default()
            },
            cosign_verifier,
            volume: self.volume_config(),
            warm_pool: self.warm_pool_config(),
        })
    }
}
