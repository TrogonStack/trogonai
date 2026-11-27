use trogon_sandbox_contract::{ArtifactType, SnapshotCaptureStatus, SnapshotReceipt};

use super::{ReleaseOutcome, capture_status};
use crate::ExportError;

type TestResult = Result<(), Box<dyn std::error::Error>>;

#[test]
fn tag_failure_keeps_capture_status_created() -> TestResult {
    let digest = "sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa".parse()?;
    let outcome = ReleaseOutcome {
        receipt: SnapshotReceipt {
            digest,
            bytes: 1,
            artifact_type: ArtifactType::SandboxRootfsV1,
        },
        tag_failure: Some(ExportError::TagNotMoved("cas".into())),
    };
    assert!(matches!(
        capture_status(&Ok(outcome)),
        SnapshotCaptureStatus::Created { .. }
    ));
    assert!(matches!(
        capture_status(&Err(ExportError::Permanent("x".into()))),
        SnapshotCaptureStatus::Failed { .. }
    ));
    Ok(())
}
