use trogon_sandbox_oci::RegistryHost;

use super::ScanPreparation;

/// What the post-export pipeline runs with.
#[derive(Debug, Clone)]
pub struct SecurityConfig {
    pub registry: String,
    pub insecure_registries: Vec<RegistryHost>,
    pub cosign_kms_uri: Option<String>,
    pub scan_cvss_threshold: f64,
    pub require_scan_pass: bool,
    pub preparation: ScanPreparation,
}
