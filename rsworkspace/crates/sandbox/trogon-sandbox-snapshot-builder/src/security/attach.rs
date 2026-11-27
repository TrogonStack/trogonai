use trogon_sandbox_contract::{ArtifactType, OciArtifactRef};
use trogon_sandbox_oci::{Layer, OciClient, build_push_target};

use super::SecurityError;
use crate::constants::SBOM_MEDIA_TYPE;

async fn attach_referrer(
    oci: &OciClient,
    subject: &OciArtifactRef,
    artifact_type: ArtifactType,
    layer: Layer,
) -> Result<(), SecurityError> {
    let layers = vec![layer];
    let target = build_push_target(
        &subject.registry,
        &subject.repository,
        artifact_type,
        &layers,
        &Default::default(),
    )?;
    oci.push_artifact(&target, layers, Default::default()).await?;
    oci.attach_referrer(subject, &target).await?;
    Ok(())
}

pub(super) async fn attach_scan_report(
    oci: &OciClient,
    subject: &OciArtifactRef,
    report: &[u8],
) -> Result<(), SecurityError> {
    let layer = Layer {
        data: report.to_vec(),
        media_type: ArtifactType::ScanReportV1.media_type().to_string(),
        annotations: Default::default(),
    };
    attach_referrer(oci, subject, ArtifactType::ScanReportV1, layer).await
}

pub(super) async fn attach_sbom(oci: &OciClient, subject: &OciArtifactRef, sbom: &[u8]) -> Result<(), SecurityError> {
    let layer = Layer {
        data: sbom.to_vec(),
        media_type: SBOM_MEDIA_TYPE.to_string(),
        annotations: Default::default(),
    };
    attach_referrer(oci, subject, ArtifactType::SbomV1, layer).await
}
