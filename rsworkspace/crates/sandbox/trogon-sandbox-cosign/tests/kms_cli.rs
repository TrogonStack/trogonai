#![cfg(unix)]

mod support;

use std::collections::BTreeMap;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};

use support::{FakeRegistry, TestResult, unreachable_ref};
use trogon_sandbox_contract::EnvDigest;
use trogon_sandbox_cosign::{
    CosignError, CosignSigner, attach_cosign_signature, attach_cosign_signature_with_annotations,
};

const KMS_URI: &str = "awskms:///arn:aws:kms:us-east-1:123456789012:key/abc";

/// An executable that appends its arguments, one per line, to `args.txt` and exits with `status`.
fn fake_cosign(dir: &Path, status: i32, stderr: &str) -> Result<PathBuf, Box<dyn std::error::Error>> {
    let args = dir.join("args.txt");
    let script = dir.join("cosign");
    std::fs::write(
        &script,
        format!(
            "#!/bin/sh\nfor arg in \"$@\"; do echo \"$arg\" >> '{}'; done\necho '{stderr}' >&2\nexit {status}\n",
            args.display()
        ),
    )?;
    std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755))?;
    Ok(script)
}

fn recorded(dir: &Path) -> Result<Vec<String>, Box<dyn std::error::Error>> {
    Ok(std::fs::read_to_string(dir.join("args.txt"))?
        .lines()
        .map(str::to_string)
        .collect())
}

#[tokio::test]
async fn kms_signing_runs_cosign_with_the_key_annotations_and_reference() -> TestResult {
    let registry = FakeRegistry::start().await?;
    let dir = tempfile::tempdir()?;
    let signer = CosignSigner::from_kms_uri_with_program(KMS_URI, fake_cosign(dir.path(), 0, "")?);
    let subject = unreachable_ref(&registry.address)?;
    let annotations = BTreeMap::from([(EnvDigest::ANNOTATION_KEY.to_string(), "sha256:b".to_string())]);

    attach_cosign_signature_with_annotations(&registry.client()?, &subject, &signer, &[], &annotations).await?;

    let args = recorded(dir.path())?;
    assert_eq!(
        args,
        vec![
            "sign".to_string(),
            "--key".into(),
            KMS_URI.into(),
            "--yes".into(),
            "--tlog-upload=false".into(),
            "--upload=true".into(),
            "-a".into(),
            format!("{}=sha256:b", EnvDigest::ANNOTATION_KEY),
            subject.allowlist_string(),
        ]
    );
    Ok(())
}

#[tokio::test]
async fn insecure_mode_applies_only_to_the_exact_registry() -> TestResult {
    let registry = FakeRegistry::start().await?;
    let subject = unreachable_ref(&registry.address)?;

    let exact = tempfile::tempdir()?;
    let signer = CosignSigner::from_kms_uri_with_program(KMS_URI, fake_cosign(exact.path(), 0, "")?);
    attach_cosign_signature(
        &registry.client()?,
        &subject,
        &signer,
        std::slice::from_ref(&registry.address),
    )
    .await?;
    assert!(recorded(exact.path())?.contains(&"--allow-insecure-registry".to_string()));

    let prefix = tempfile::tempdir()?;
    let signer = CosignSigner::from_kms_uri_with_program(KMS_URI, fake_cosign(prefix.path(), 0, "")?);
    let shorter = registry.address.split(':').next().unwrap_or_default().to_string();
    attach_cosign_signature(&registry.client()?, &subject, &signer, &[shorter]).await?;
    assert!(!recorded(prefix.path())?.contains(&"--allow-insecure-registry".to_string()));
    Ok(())
}

#[tokio::test]
async fn a_failing_cosign_reports_its_stderr() -> TestResult {
    let registry = FakeRegistry::start().await?;
    let dir = tempfile::tempdir()?;
    let signer = CosignSigner::from_kms_uri_with_program(KMS_URI, fake_cosign(dir.path(), 3, "kms denied")?);
    let subject = unreachable_ref(&registry.address)?;

    let outcome = attach_cosign_signature(&registry.client()?, &subject, &signer, &[]).await;
    assert!(
        matches!(&outcome, Err(CosignError::SignFailed(message)) if message.contains("kms denied")),
        "{outcome:?}"
    );
    Ok(())
}

#[tokio::test]
async fn a_missing_cosign_program_is_a_sign_failure() -> TestResult {
    let registry = FakeRegistry::start().await?;
    let dir = tempfile::tempdir()?;
    let signer = CosignSigner::from_kms_uri_with_program(KMS_URI, dir.path().join("absent"));
    let subject = unreachable_ref(&registry.address)?;

    let outcome = attach_cosign_signature(&registry.client()?, &subject, &signer, &[]).await;
    assert!(matches!(outcome, Err(CosignError::SignFailed(_))), "{outcome:?}");
    assert!(matches!(
        CosignSigner::from_kms_uri(KMS_URI),
        CosignSigner::AwsKms { .. }
    ));
    Ok(())
}
