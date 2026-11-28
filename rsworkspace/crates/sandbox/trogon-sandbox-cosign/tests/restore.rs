mod support;

use support::{FakeRegistry, TestResult, digest, unreachable_ref};
use trogon_sandbox_contract::{EnvDigest, LeaseError};
use trogon_sandbox_cosign::{
    CosignError, CosignSigner, CosignVerifier, EnvDigestBinding, SnapshotPullError, VerifyMode, apply_mode,
    attach_cosign_signature, attach_cosign_signature_with_annotations, enforce_restore_signature,
    generate_test_key_pair, into_pull_error, snapshot_signature_lease_error,
};

#[test]
fn modes_parse_and_display() -> TestResult {
    for (text, mode) in [
        ("off", VerifyMode::Off),
        ("warn", VerifyMode::Warn),
        ("require", VerifyMode::Require),
    ] {
        assert_eq!(text.parse::<VerifyMode>()?, mode);
        assert_eq!(mode.to_string(), text);
    }
    let error = "strict".parse::<VerifyMode>().err().ok_or("accepted")?;
    assert_eq!(
        error.to_string(),
        "unknown cosign verify mode 'strict' (expected off, warn or require)"
    );
    Ok(())
}

#[test]
fn only_require_refuses_a_failure() -> TestResult {
    let reference = unreachable_ref("registry.test")?;
    let failure = || CosignError::AnnotationMismatch {
        digest: reference.digest.clone(),
    };
    assert!(apply_mode(VerifyMode::Off, Err(failure()), &reference).is_ok());
    assert!(apply_mode(VerifyMode::Warn, Err(failure()), &reference).is_ok());
    assert!(apply_mode(VerifyMode::Require, Err(failure()), &reference).is_err());
    assert!(apply_mode(VerifyMode::Require, Ok(()), &reference).is_ok());
    Ok(())
}

#[tokio::test]
async fn a_missing_key_refuses_only_under_require() -> TestResult {
    let reference = unreachable_ref("registry.test")?;
    assert!(
        enforce_restore_signature(VerifyMode::Off, None, &reference, EnvDigestBinding::Unbound)
            .await
            .is_ok()
    );
    assert!(
        enforce_restore_signature(VerifyMode::Warn, None, &reference, EnvDigestBinding::Unbound)
            .await
            .is_ok()
    );
    assert!(matches!(
        enforce_restore_signature(VerifyMode::Require, None, &reference, EnvDigestBinding::Unbound).await,
        Err(CosignError::PublicKey(_))
    ));
    Ok(())
}

#[tokio::test]
async fn restores_bind_the_environment_digest_to_the_signature() -> TestResult {
    let registry = FakeRegistry::start().await?;
    let subject = registry.push_artifact(b"disk").await?;
    let (private, public) = generate_test_key_pair()?;
    let signer = CosignSigner::from_private_key_pem(private, None);
    let signed = EnvDigest::from(digest('b')?);
    let annotations =
        std::collections::BTreeMap::from([(EnvDigest::ANNOTATION_KEY.to_string(), signed.as_str().to_string())]);
    attach_cosign_signature_with_annotations(
        &registry.client()?,
        &subject,
        &signer,
        &registry.insecure(),
        &annotations,
    )
    .await?;
    let verifier = CosignVerifier::from_pem(public).with_insecure_registries(registry.insecure());

    enforce_restore_signature(
        VerifyMode::Require,
        Some(&verifier),
        &subject,
        EnvDigestBinding::Bound(&signed),
    )
    .await?;
    enforce_restore_signature(
        VerifyMode::Require,
        Some(&verifier),
        &subject,
        EnvDigestBinding::Unbound,
    )
    .await?;

    let other = EnvDigest::from(digest('c')?);
    let outcome = enforce_restore_signature(
        VerifyMode::Require,
        Some(&verifier),
        &subject,
        EnvDigestBinding::Bound(&other),
    )
    .await;
    assert!(
        matches!(outcome, Err(CosignError::AnnotationMismatch { .. })),
        "{outcome:?}"
    );
    enforce_restore_signature(
        VerifyMode::Warn,
        Some(&verifier),
        &subject,
        EnvDigestBinding::Bound(&other),
    )
    .await?;
    Ok(())
}

#[tokio::test]
async fn an_unannotated_snapshot_fails_a_required_environment_digest() -> TestResult {
    let registry = FakeRegistry::start().await?;
    let subject = registry.push_artifact(b"disk").await?;
    let (private, public) = generate_test_key_pair()?;
    let signer = CosignSigner::from_private_key_pem(private, None);
    attach_cosign_signature(&registry.client()?, &subject, &signer, &registry.insecure()).await?;
    let verifier = CosignVerifier::from_pem(public).with_insecure_registries(registry.insecure());

    let expected = EnvDigest::from(digest('b')?);
    let outcome = enforce_restore_signature(
        VerifyMode::Require,
        Some(&verifier),
        &subject,
        EnvDigestBinding::Bound(&expected),
    )
    .await;
    assert!(
        matches!(outcome, Err(CosignError::AnnotationMismatch { .. })),
        "{outcome:?}"
    );
    Ok(())
}

#[test]
fn rejections_become_the_broker_visible_lease_error() -> TestResult {
    let digest = digest('a')?;
    assert!(matches!(
        snapshot_signature_lease_error(digest.clone()),
        LeaseError::SnapshotSignatureInvalid { .. }
    ));
    for error in [
        CosignError::SignatureInvalid { digest: digest.clone() },
        CosignError::AnnotationMismatch { digest: digest.clone() },
    ] {
        assert!(matches!(
            into_pull_error(error),
            SnapshotPullError::SignatureRejected(LeaseError::SnapshotSignatureInvalid { .. })
        ));
    }
    let other = into_pull_error(CosignError::Registry("down".into()));
    assert!(matches!(other, SnapshotPullError::Cosign(CosignError::Registry(_))));
    assert_eq!(other.to_string(), "registry: down");
    Ok(())
}
