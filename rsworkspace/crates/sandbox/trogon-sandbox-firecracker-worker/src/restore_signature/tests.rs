use trogon_sandbox_contract::{ArtifactType, Digest};

use super::*;

type TestResult = Result<(), Box<dyn std::error::Error>>;

fn reference() -> Result<OciArtifactRef, Box<dyn std::error::Error>> {
    Ok(OciArtifactRef {
        registry: "registry.test".into(),
        repository: "snap".into(),
        digest: Digest::new(format!("sha256:{}", "a".repeat(64)))?,
        artifact_type: ArtifactType::FirecrackerSnapshotV1,
    })
}

#[test]
fn require_without_a_key_is_a_startup_error() {
    let policy = RestoreSignature {
        verifier: None,
        mode: VerifyMode::Require,
    };
    assert!(policy.validate().is_err());
    assert!(RestoreSignature::default().validate().is_ok());
}

#[tokio::test]
async fn warn_restores_and_require_refuses_an_unverifiable_snapshot() -> TestResult {
    let reference = reference()?;
    let warn = RestoreSignature::default();
    assert!(warn.check(&reference, None).await.is_ok());
    let off = RestoreSignature {
        verifier: None,
        mode: VerifyMode::Off,
    };
    assert!(off.check(&reference, None).await.is_ok());
    let require = RestoreSignature {
        verifier: None,
        mode: VerifyMode::Require,
    };
    assert!(require.check(&reference, None).await.is_err());
    Ok(())
}
