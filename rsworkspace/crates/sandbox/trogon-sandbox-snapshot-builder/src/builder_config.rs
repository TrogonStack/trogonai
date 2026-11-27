use std::sync::Arc;

use trogon_sandbox_oci::RegistryHost;
use trogon_sandbox_protocol::DispatchSigner;

/// What the builder runs with.
#[derive(Clone)]
pub struct BuilderConfig {
    pub nats_url: String,
    pub default_registry: String,
    pub max_concurrent: usize,
    pub cosign_kms_uri: Option<String>,
    pub scan_cvss_threshold: f64,
    pub require_scan_pass: bool,
    pub oci_insecure_registries: Vec<RegistryHost>,
    /// Signs the export requests sent to workers. Unset sends them unsigned.
    pub dispatch_signer: Option<Arc<DispatchSigner>>,
}
