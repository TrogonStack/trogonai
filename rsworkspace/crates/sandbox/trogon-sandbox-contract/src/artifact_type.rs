use serde::{Deserialize, Serialize};

/// An artifact's wire kind and media type; runtime support is defined by its consumer.
#[derive(Debug, Clone, Copy, Eq, PartialEq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ArtifactType {
    SandboxRootfsV1,
    HyperlightSnapshotV1,
    FirecrackerSnapshotV1,
    KubevirtDiskV1,
    SbomV1,
    ScanReportV1,
}

impl ArtifactType {
    pub fn media_type(&self) -> &'static str {
        match self {
            Self::SandboxRootfsV1 => "application/vnd.trogon.sandbox.rootfs.v1+tar",
            Self::HyperlightSnapshotV1 => "application/vnd.trogon.hyperlight.snapshot.v1",
            Self::FirecrackerSnapshotV1 => "application/vnd.trogon.firecracker.snapshot.v1",
            Self::KubevirtDiskV1 => "application/vnd.trogon.kubevirt.disk.v1+qcow2",
            Self::SbomV1 => "application/vnd.trogon.sbom.v1",
            Self::ScanReportV1 => "application/vnd.trogon.scan-report.v1",
        }
    }
}
