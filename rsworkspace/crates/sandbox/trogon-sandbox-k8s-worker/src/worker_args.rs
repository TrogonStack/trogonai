use std::path::PathBuf;

use clap::Parser;
use trogon_sandbox_contract::IsolationTier;
use trogon_sandbox_observability::ObservabilityArgs;
use trogon_sandbox_providers::{K8sMicroVmConfig, K8sResourceNaming, ORPHAN_RECLAIM_GRACE_SECONDS};

use crate::{WarmPoolConfig, WorkerIdentity};

/// Command line and environment configuration of the worker.
#[derive(Parser, Debug)]
#[command(
    version,
    about = "Sandbox Kubernetes worker: serves lease requests over NATS and runs each lease in a pod"
)]
pub struct WorkerArgs {
    /// Journal for the tenant and attempt each lease belongs to, so the dispatch fence survives a
    /// restart. Unset keeps it in memory.
    #[arg(long, env = "SANDBOX_DISPATCH_FENCE_FILE")]
    pub dispatch_fence_file: Option<PathBuf>,

    /// NATS server the worker serves leases from.
    #[arg(long, env = "SANDBOX_NATS_URL", default_value = "nats://127.0.0.1:4222")]
    pub nats_url: String,

    /// Namespace the lease Jobs and pods live in.
    #[arg(long, env = "SANDBOX_K8S_NAMESPACE")]
    pub k8s_namespace: String,

    /// The image a lease boots when its own reference names none.
    #[arg(long, env = "SANDBOX_IMAGE")]
    pub image: String,

    #[arg(long, env = "SANDBOX_POD_RUNTIME_CLASS", default_value = "kata-fc")]
    pub runtime_class: String,

    /// RuntimeClass for `IsolationTier::MicroVm` leases. Defaults to `--runtime-class` when unset.
    #[arg(long, env = "SANDBOX_POD_RUNTIME_CLASS_MICRO_VM")]
    pub runtime_class_micro_vm: Option<String>,

    /// RuntimeClass for `IsolationTier::UserSpaceKernel` leases, typically `gvisor`. Optional.
    #[arg(long, env = "SANDBOX_POD_RUNTIME_CLASS_USER_SPACE_KERNEL")]
    pub runtime_class_user_space_kernel: Option<String>,

    #[arg(long, env = "SANDBOX_SERVICE_ACCOUNT", default_value = "sandbox-agent")]
    pub service_account: String,

    #[arg(long, env = "SANDBOX_K8S_RESOURCE_NAMING", default_value = "sandbox")]
    pub resource_naming: K8sResourceNaming,

    #[arg(long, env = "SANDBOX_IMAGE_PULL_SECRETS", value_delimiter = ',')]
    pub image_pull_secrets: Vec<String>,

    /// How many leases the worker advertises it can hold at once.
    #[arg(long, env = "SANDBOX_WORKER_MAX_CONCURRENT", default_value_t = 4)]
    pub max_concurrent_leases: u32,

    #[arg(long, env = "SANDBOX_SNAPSHOT_REGISTRY", default_value = "localhost:5000")]
    pub snapshot_registry: String,

    #[arg(long, env = "SANDBOX_OCI_INSECURE_REGISTRIES", value_delimiter = ',')]
    pub oci_insecure_registries: Vec<String>,

    /// PEM path of the public key snapshots are verified against.
    #[arg(long, env = "SANDBOX_COSIGN_PUBLIC_KEY")]
    pub cosign_public_key: Option<PathBuf>,

    #[arg(long, env = "SANDBOX_DIGEST_CACHE_SIZE", default_value_t = 32)]
    pub digest_cache_size: usize,

    #[arg(long, env = "SANDBOX_SNAPSHOT_REPOSITORY", default_value = "sandbox/rootfs")]
    pub snapshot_repository: String,

    #[arg(long, env = "SANDBOX_WARM_POOL_MAX_PER_DIGEST", default_value_t = 3)]
    pub warm_pool_max_per_digest: u32,

    #[arg(long, env = "SANDBOX_WARM_POOL_MAX_TOTAL", default_value_t = 16)]
    pub warm_pool_max_total: u32,

    #[arg(long, env = "SANDBOX_WARM_POOL_MAX_IDLE_SECS", default_value_t = 600)]
    pub warm_pool_max_idle_secs: u64,

    /// Most warm pods one reconcile pass (every 30s) may create.
    #[arg(long, env = "SANDBOX_WARM_POOL_REFILL_PER_PASS", default_value_t = 2)]
    pub warm_pool_refill_per_pass: u32,

    /// Most warm pods allowed to be starting at once.
    #[arg(long, env = "SANDBOX_WARM_POOL_MAX_WARMING", default_value_t = 4)]
    pub warm_pool_max_warming: u32,

    /// Stable identity of this replica, such as its StatefulSet pod name from the downward API.
    /// Recovery requires this identity to match the resource's owner label. Unset generates a new
    /// identity on startup, which cannot recover resources from a previous process.
    #[arg(long, env = "SANDBOX_K8S_WORKER_IDENTITY")]
    pub worker_identity: Option<WorkerIdentity>,

    /// Deprecated compatibility flag. Unlabeled leases require explicit operator reconciliation;
    /// this flag does not authorize their recovery.
    #[arg(
        long,
        env = "SANDBOX_K8S_RECOVER_UNLABELED_LEASES",
        default_value_t = true,
        action = clap::ArgAction::Set
    )]
    pub recover_unlabeled_leases: bool,

    /// Deprecated compatibility setting. Other worker identities remain untouched even after this
    /// grace period.
    #[arg(
        long,
        env = "SANDBOX_K8S_ORPHAN_RECLAIM_GRACE_SECS",
        default_value_t = ORPHAN_RECLAIM_GRACE_SECONDS
    )]
    pub orphan_reclaim_grace_secs: i64,

    /// Keep an image's own non-root `USER` for leases that name no user, instead of forcing uid
    /// 65532. An image whose `USER` is root, unset or a name rather than a numeric id falls back
    /// to uid 65532 with a warning, so the agent never runs as root.
    #[arg(
        long,
        env = "SANDBOX_K8S_HONOR_IMAGE_USER",
        default_value_t = false,
        action = clap::ArgAction::Set
    )]
    pub honor_image_user: bool,

    /// Keep each lease's creation request in a Secret owned by its Job or warm pod, so an in-place
    /// rollback still works after this worker restarts. Off by default because the request can
    /// carry env values.
    #[arg(
        long,
        env = "SANDBOX_K8S_PERSIST_ROLLBACK_STATE",
        default_value_t = false,
        action = clap::ArgAction::Set
    )]
    pub persist_rollback_state: bool,

    #[command(flatten)]
    pub observability: ObservabilityArgs,
}

impl WorkerArgs {
    /// The cluster side of the configuration, for the replica named `identity`.
    pub fn k8s_config(&self, identity: &WorkerIdentity) -> K8sMicroVmConfig {
        let image_pull_secrets = self
            .image_pull_secrets
            .iter()
            .filter(|secret| !secret.is_empty())
            .cloned()
            .collect();
        let mut config = K8sMicroVmConfig::new(&self.k8s_namespace)
            .with_resource_naming(self.resource_naming)
            .with_runtime_class(&self.runtime_class)
            .with_service_account(&self.service_account)
            .with_image_pull_secrets(image_pull_secrets)
            .with_recover_unlabeled_leases(self.recover_unlabeled_leases)
            .with_honor_image_user(self.honor_image_user)
            .with_orphan_reclaim_grace_seconds(self.orphan_reclaim_grace_secs)
            .with_worker_identity(identity.as_str());
        if let Some(class) = self.runtime_class_micro_vm.as_deref() {
            config = config.with_runtime_class_for_tier(IsolationTier::MicroVm, class);
        }
        if let Some(class) = self.runtime_class_user_space_kernel.as_deref() {
            config = config.with_runtime_class_for_tier(IsolationTier::UserSpaceKernel, class);
        }
        config
    }

    /// The bounds of the warm pod pool.
    pub fn warm_pool_config(&self) -> WarmPoolConfig {
        WarmPoolConfig {
            max_pool_size_per_digest: self.warm_pool_max_per_digest,
            max_total_warm: self.warm_pool_max_total,
            max_idle_seconds: self.warm_pool_max_idle_secs,
            refill_per_pass: self.warm_pool_refill_per_pass,
            max_warming: self.warm_pool_max_warming,
        }
    }

    /// The registries reached over plain HTTP: the configured ones plus the snapshot registry.
    pub fn insecure_registries(&self) -> Vec<String> {
        let mut registries: Vec<String> = self
            .oci_insecure_registries
            .iter()
            .filter(|host| !host.is_empty())
            .cloned()
            .collect();
        if !registries.contains(&self.snapshot_registry) {
            registries.push(self.snapshot_registry.clone());
        }
        registries
    }
}

#[cfg(test)]
mod tests;
