use trogon_sandbox_contract::ArtifactType;

use crate::AdminError;

/// Parses an artifact type given as its wire name or its type name.
pub fn parse_artifact_type(raw: &str) -> Result<ArtifactType, AdminError> {
    match raw {
        "sandbox_rootfs_v1" | "SandboxRootfsV1" => Ok(ArtifactType::SandboxRootfsV1),
        "hyperlight_snapshot_v1" | "HyperlightSnapshotV1" => Ok(ArtifactType::HyperlightSnapshotV1),
        "firecracker_snapshot_v1" | "FirecrackerSnapshotV1" => Ok(ArtifactType::FirecrackerSnapshotV1),
        "kubevirt_disk_v1" | "KubevirtDiskV1" => Ok(ArtifactType::KubevirtDiskV1),
        "sbom_v1" | "SbomV1" => Ok(ArtifactType::SbomV1),
        "scan_report_v1" | "ScanReportV1" => Ok(ArtifactType::ScanReportV1),
        other => Err(AdminError::UnknownArtifactType {
            value: other.to_string(),
        }),
    }
}
