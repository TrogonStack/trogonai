use std::time::Duration;

use tempfile::TempDir;
use tracing::{info, warn};
use trogon_sandbox_contract::{ArtifactType, EnvDigest, OciArtifactRef, SnapshotPolicy, SnapshotReceipt};
use trogon_sandbox_cosign::{CosignSigner, attach_cosign_signature_with_annotations};
use trogon_sandbox_oci::{ArtifactPullLimits, OciClient, OciClientConfig};

use super::attach::{attach_sbom, attach_scan_report};
use super::prepare_scan_input::prepare_scan_input_async;
use super::signed_annotations::{manifest_env_digest, signed_annotations};
use super::syft::generate_sbom;
use super::trivy::scan_path;
use super::{SecurityConfig, SecurityError};
use crate::constants::FIRECRACKER_LAYER_COUNT;

/// Pulls the exported snapshot back, gates it on a scan, attaches a bill of materials, and signs it. Returns the
/// environment digest the worker stamped on the manifest, when it is well formed.
pub async fn run_security_pipeline(
    config: &SecurityConfig,
    policy: &SnapshotPolicy,
    receipt: &SnapshotReceipt,
) -> Result<Option<EnvDigest>, SecurityError> {
    let subject = OciArtifactRef {
        registry: config.registry.clone(),
        repository: policy.target_repository.clone(),
        digest: receipt.digest.clone(),
        artifact_type: receipt.artifact_type,
    };

    let oci = OciClient::new(OciClientConfig {
        insecure_registries: config.insecure_registries.clone(),
        ..Default::default()
    });

    let pulled = if subject.artifact_type == ArtifactType::FirecrackerSnapshotV1 {
        let limit = config.preparation.limits.max_compressed_bytes;
        oci.pull_artifact_bounded(
            &subject,
            ArtifactPullLimits {
                max_layers: FIRECRACKER_LAYER_COUNT,
                max_layer_bytes: limit,
                max_total_bytes: limit,
                timeout: Duration::from_millis(config.preparation.limits.timeout_ms),
            },
        )
        .await?
    } else {
        oci.pull_artifact(&subject).await?
    };
    let temp = TempDir::new()?;
    let scan_input =
        prepare_scan_input_async(temp.path(), subject.artifact_type, &pulled.layers, &config.preparation).await?;

    if config.require_scan_pass {
        let report = scan_path(&scan_input, config.scan_cvss_threshold, config.require_scan_pass).await?;

        attach_scan_report(&oci, &subject, &report).await?;
        info!(digest = %subject.digest, "trivy scan passed");
    }

    if let Ok(sbom) = generate_sbom(&scan_input).await {
        attach_sbom(&oci, &subject, &sbom).await?;
        info!(digest = %subject.digest, bytes = sbom.len(), "sbom attached");
    } else if config.require_scan_pass {
        warn!(digest = %subject.digest, "syft unavailable; skipping sbom attachment");
    }

    if let Some(kms_uri) = &config.cosign_kms_uri {
        let signer = CosignSigner::from_kms_uri(kms_uri.clone());
        let insecure: Vec<String> = config
            .insecure_registries
            .iter()
            .map(|host| host.as_str().to_string())
            .collect();
        attach_cosign_signature_with_annotations(
            &oci,
            &subject,
            &signer,
            &insecure,
            &signed_annotations(&pulled.annotations)?,
        )
        .await?;
        info!(digest = %subject.digest, "cosign signature attached via kms");
    }

    Ok(manifest_env_digest(&pulled.annotations))
}
