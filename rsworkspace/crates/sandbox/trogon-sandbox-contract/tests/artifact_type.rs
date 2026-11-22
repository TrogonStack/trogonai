use std::collections::HashSet;
use std::error::Error;

use serde_json::error::Category::{Data, Syntax};
use trogon_sandbox_contract::ArtifactType;

#[test]
fn every_artifact_type_preserves_its_json_and_media_type() -> Result<(), Box<dyn Error>> {
    let fixtures = [
        (
            ArtifactType::SandboxRootfsV1,
            r#""sandbox_rootfs_v1""#,
            "application/vnd.trogon.sandbox.rootfs.v1+tar",
        ),
        (
            ArtifactType::HyperlightSnapshotV1,
            r#""hyperlight_snapshot_v1""#,
            "application/vnd.trogon.hyperlight.snapshot.v1",
        ),
        (
            ArtifactType::FirecrackerSnapshotV1,
            r#""firecracker_snapshot_v1""#,
            "application/vnd.trogon.firecracker.snapshot.v1",
        ),
        (
            ArtifactType::KubevirtDiskV1,
            r#""kubevirt_disk_v1""#,
            "application/vnd.trogon.kubevirt.disk.v1+qcow2",
        ),
        (ArtifactType::SbomV1, r#""sbom_v1""#, "application/vnd.trogon.sbom.v1"),
        (
            ArtifactType::ScanReportV1,
            r#""scan_report_v1""#,
            "application/vnd.trogon.scan-report.v1",
        ),
    ];
    let mut unique = HashSet::new();
    for (artifact_type, json, media_type) in fixtures {
        assert!(unique.insert(artifact_type));
        assert_eq!(artifact_type.media_type(), media_type);
        assert_eq!(serde_json::to_string(&artifact_type)?, json);
        assert_eq!(serde_json::from_str::<ArtifactType>(json)?, artifact_type);
    }
    Ok(())
}

#[test]
fn unsupported_artifact_spellings_and_json_shapes_are_rejected() {
    for (json, category) in [
        (r#""SandboxRootfsV1""#, Data),
        (r#""Sandbox_Rootfs_V1""#, Data),
        (r#""hyperlight_Snapshot_v1""#, Data),
        (r#""FIRECRACKER_SNAPSHOT_V1""#, Data),
        (r#""Kubevirt_disk_v1""#, Data),
        (r#""SBOM_V1""#, Data),
        (r#""Scan_Report_V1""#, Data),
        (r#""sandbox_rootfs_v2""#, Data),
        (r#""unknown""#, Data),
        (r#""application/vnd.trogon.sbom.v1""#, Data),
        ("null", Syntax),
        ("true", Syntax),
        ("0", Syntax),
        ("[]", Syntax),
        ("{}", Syntax),
    ] {
        let decoded = serde_json::from_str::<ArtifactType>(json);
        assert!(decoded.is_err(), "decoded {json}");
        if let Err(error) = decoded {
            assert_eq!(error.classify(), category, "{json}");
        }
    }
}
