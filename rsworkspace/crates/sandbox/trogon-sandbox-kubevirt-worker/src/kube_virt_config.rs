use trogon_sandbox_contract::WorkerId;
use trogon_sandbox_cosign::CosignVerifier;
use trogon_sandbox_oci::OciClientConfig;
use trogon_sandbox_volumes::VolumeClientConfig;

use crate::WarmPoolConfig;

/// Runtime configuration of the KubeVirt worker.
#[derive(Debug, Clone)]
pub struct KubeVirtConfig {
    pub namespace: String,
    pub default_image: String,
    pub storage_class: Option<String>,
    pub snapshot_registry: String,
    pub worker_id: WorkerId,
    pub disk_size_gi: u32,
    pub bootstrap_ssh_pubkey: Option<String>,
    pub s3_secret_name: Option<String>,
    pub restore_bucket_prefix: String,
    pub oci: OciClientConfig,
    pub cosign_verifier: Option<CosignVerifier>,
    pub volume: Option<VolumeClientConfig>,
    pub warm_pool: WarmPoolConfig,
}

impl KubeVirtConfig {
    /// The root disk size as a Kubernetes quantity, never below one gibibyte.
    pub fn disk_size_quantity(&self) -> String {
        format!("{}Gi", self.disk_size_gi.max(1))
    }
}
