use std::sync::Arc;

use clap::Parser;
use trogon_sandbox_observability::ObservabilityArgs;
use trogon_sandbox_oci::{RegistryHost, RegistryHostError};
use trogon_sandbox_protocol::DispatchSigner;

use crate::BuilderConfig;

/// Command line and environment configuration of the snapshot builder.
#[derive(Parser, Debug)]
#[command(
    version,
    about = "Subscribes to lease-released events, requests worker snapshot export, secures the result, and records its digest"
)]
pub struct SnapshotBuilderArgs {
    /// NATS server the builder serves from.
    #[arg(long, env = "SANDBOX_NATS_URL", default_value = "nats://127.0.0.1:4222")]
    pub nats_url: String,

    /// Most snapshots processed at once.
    #[arg(long, env = "SANDBOX_SNAPSHOT_BUILDER_MAX_CONCURRENT", default_value_t = 4)]
    pub max_concurrent: usize,

    #[arg(long, env = "SANDBOX_REGISTRY_URL", default_value = "127.0.0.1:5000")]
    pub registry_url: String,

    /// Cosign KMS key URI the snapshots are signed with. Unset skips signing.
    #[arg(long, env = "SANDBOX_COSIGN_KMS_URI")]
    pub cosign_kms_uri: Option<String>,

    #[arg(long, env = "SANDBOX_SCAN_CVSS_THRESHOLD", default_value_t = 7.0)]
    pub scan_cvss_threshold: f64,

    #[arg(
        long,
        env = "SANDBOX_REQUIRE_SCAN_PASS",
        default_value_t = true,
        action = clap::ArgAction::Set
    )]
    pub require_scan_pass: bool,

    #[arg(long, env = "SANDBOX_OCI_INSECURE_REGISTRIES", value_delimiter = ',')]
    pub oci_insecure_registries: Vec<String>,

    #[command(flatten)]
    pub observability: ObservabilityArgs,
}

impl SnapshotBuilderArgs {
    /// The builder configuration these arguments describe, signing worker requests with `dispatch_signer`.
    pub fn builder_config(
        &self,
        dispatch_signer: Option<Arc<DispatchSigner>>,
    ) -> Result<BuilderConfig, RegistryHostError> {
        let oci_insecure_registries = self
            .oci_insecure_registries
            .iter()
            .filter(|host| !host.is_empty())
            .map(|host| RegistryHost::new(host.as_str()))
            .collect::<Result<Vec<_>, _>>()?;
        Ok(BuilderConfig {
            nats_url: self.nats_url.clone(),
            default_registry: self.registry_url.clone(),
            max_concurrent: self.max_concurrent,
            cosign_kms_uri: self.cosign_kms_uri.clone(),
            scan_cvss_threshold: self.scan_cvss_threshold,
            require_scan_pass: self.require_scan_pass,
            oci_insecure_registries,
            dispatch_signer,
        })
    }
}

#[cfg(test)]
mod tests;
