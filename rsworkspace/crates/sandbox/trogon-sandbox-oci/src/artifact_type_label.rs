use trogon_sandbox_contract::ArtifactType;

/// The metric label value for an artifact type.
pub fn artifact_type_label(artifact_type: ArtifactType) -> &'static str {
    match artifact_type {
        ArtifactType::SandboxRootfsV1 => "sandbox_rootfs_v1",
        ArtifactType::HyperlightSnapshotV1 => "hyperlight_snapshot_v1",
        ArtifactType::FirecrackerSnapshotV1 => "firecracker_snapshot_v1",
        ArtifactType::KubevirtDiskV1 => "kubevirt_disk_v1",
        ArtifactType::SbomV1 => "sbom_v1",
        ArtifactType::ScanReportV1 => "scan_report_v1",
    }
}
