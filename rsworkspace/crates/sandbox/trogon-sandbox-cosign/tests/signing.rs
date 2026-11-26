mod support;

use std::collections::BTreeMap;

use secrecy::SecretString;
use support::{FakeRegistry, TestResult, unreachable_ref};
use trogon_sandbox_contract::EnvDigest;
use trogon_sandbox_cosign::{
    CosignError, CosignSigner, CosignVerifier, attach_cosign_signature, attach_cosign_signature_with_annotations,
    generate_test_key_pair, verify_if_configured, verify_snapshot_signature,
    verify_snapshot_signature_with_annotations,
};

fn env_annotations(seed: char) -> BTreeMap<String, String> {
    BTreeMap::from([(
        EnvDigest::ANNOTATION_KEY.to_string(),
        format!("sha256:{}", seed.to_string().repeat(64)),
    )])
}

#[tokio::test]
async fn a_signed_snapshot_verifies_with_the_matching_key() -> TestResult {
    let registry = FakeRegistry::start().await?;
    let subject = registry.push_artifact(b"disk").await?;
    let (private, public) = generate_test_key_pair()?;
    let signer = CosignSigner::from_private_key_pem(private, None);
    attach_cosign_signature(&registry.client()?, &subject, &signer, &registry.insecure()).await?;

    let verifier = CosignVerifier::from_pem(public).with_insecure_registries(registry.insecure());
    verify_snapshot_signature(&verifier, &subject).await?;
    verify_if_configured(Some(&verifier), &subject).await?;
    verify_if_configured(None, &subject).await?;
    Ok(())
}

#[tokio::test]
async fn a_signature_from_another_key_is_rejected() -> TestResult {
    let registry = FakeRegistry::start().await?;
    let subject = registry.push_artifact(b"disk").await?;
    let (private, _) = generate_test_key_pair()?;
    let (_, other_public) = generate_test_key_pair()?;
    let signer = CosignSigner::from_private_key_pem(private, None);
    attach_cosign_signature(&registry.client()?, &subject, &signer, &registry.insecure()).await?;

    let verifier = CosignVerifier::from_pem(other_public).with_insecure_registries(registry.insecure());
    let outcome = verify_snapshot_signature(&verifier, &subject).await;
    assert!(
        matches!(outcome, Err(CosignError::SignatureInvalid { .. })),
        "{outcome:?}"
    );
    Ok(())
}

#[tokio::test]
async fn an_unsigned_snapshot_is_rejected() -> TestResult {
    let registry = FakeRegistry::start().await?;
    let subject = registry.push_artifact(b"disk").await?;
    let (_, public) = generate_test_key_pair()?;

    let verifier = CosignVerifier::from_pem(public).with_insecure_registries(registry.insecure());
    let outcome = verify_snapshot_signature(&verifier, &subject).await;
    assert!(
        matches!(
            outcome,
            Err(CosignError::SignatureInvalid { .. } | CosignError::VerifyFailed(_))
        ),
        "{outcome:?}"
    );
    Ok(())
}

#[tokio::test]
async fn signed_annotations_must_match_the_required_ones() -> TestResult {
    let registry = FakeRegistry::start().await?;
    let subject = registry.push_artifact(b"disk").await?;
    let (private, public) = generate_test_key_pair()?;
    let signer = CosignSigner::from_private_key_pem(private, None);
    let signed = env_annotations('b');
    attach_cosign_signature_with_annotations(&registry.client()?, &subject, &signer, &registry.insecure(), &signed)
        .await?;
    let verifier = CosignVerifier::from_pem(public).with_insecure_registries(registry.insecure());

    verify_snapshot_signature_with_annotations(&verifier, &subject, &signed).await?;
    verify_snapshot_signature_with_annotations(&verifier, &subject, &BTreeMap::new()).await?;

    let relabeled = verify_snapshot_signature_with_annotations(&verifier, &subject, &env_annotations('c')).await;
    assert!(
        matches!(relabeled, Err(CosignError::AnnotationMismatch { .. })),
        "{relabeled:?}"
    );
    let missing = BTreeMap::from([("other".to_string(), "value".to_string())]);
    let absent = verify_snapshot_signature_with_annotations(&verifier, &subject, &missing).await;
    assert!(
        matches!(absent, Err(CosignError::AnnotationMismatch { .. })),
        "{absent:?}"
    );
    Ok(())
}

#[tokio::test]
async fn an_unannotated_signature_cannot_satisfy_required_annotations() -> TestResult {
    let registry = FakeRegistry::start().await?;
    let subject = registry.push_artifact(b"disk").await?;
    let (private, public) = generate_test_key_pair()?;
    let signer = CosignSigner::from_private_key_pem(private, None);
    attach_cosign_signature(&registry.client()?, &subject, &signer, &registry.insecure()).await?;
    let verifier = CosignVerifier::from_pem(public).with_insecure_registries(registry.insecure());

    let outcome = verify_snapshot_signature_with_annotations(&verifier, &subject, &env_annotations('b')).await;
    assert!(
        matches!(outcome, Err(CosignError::AnnotationMismatch { .. })),
        "{outcome:?}"
    );
    Ok(())
}

#[tokio::test]
async fn an_unparseable_public_key_is_reported() -> TestResult {
    let registry = FakeRegistry::start().await?;
    let subject = registry.push_artifact(b"disk").await?;
    let (private, _) = generate_test_key_pair()?;
    let signer = CosignSigner::from_private_key_pem(private, None);
    attach_cosign_signature(&registry.client()?, &subject, &signer, &registry.insecure()).await?;

    let verifier = CosignVerifier::from_pem(b"not a key".to_vec()).with_insecure_registries(registry.insecure());
    let outcome = verify_snapshot_signature(&verifier, &subject).await;
    assert!(matches!(outcome, Err(CosignError::PublicKey(_))), "{outcome:?}");
    Ok(())
}

#[tokio::test]
async fn verification_fails_for_a_bad_reference_or_unreachable_registry() -> TestResult {
    let (_, public) = generate_test_key_pair()?;
    let verifier = CosignVerifier::from_pem(public);

    let bad = unreachable_ref("not a registry!")?;
    let outcome = verify_snapshot_signature(&verifier, &bad).await;
    assert!(matches!(outcome, Err(CosignError::VerifyFailed(_))), "{outcome:?}");

    let registry = FakeRegistry::start().await?;
    let missing = unreachable_ref(&registry.address)?;
    let verifier = verifier.with_insecure_registries(registry.insecure());
    let outcome = verify_snapshot_signature(&verifier, &missing).await;
    assert!(matches!(outcome, Err(CosignError::VerifyFailed(_))), "{outcome:?}");
    Ok(())
}

#[tokio::test]
async fn local_key_files_sign_and_report_their_failures() -> TestResult {
    let registry = FakeRegistry::start().await?;
    let subject = registry.push_artifact(b"disk").await?;
    let (private, public) = generate_test_key_pair()?;
    let dir = tempfile::tempdir()?;
    let key_path = dir.path().join("cosign.key");
    std::fs::write(&key_path, &private)?;
    let public_path = dir.path().join("cosign.pub");
    std::fs::write(&public_path, &public)?;

    let signer = CosignSigner::from_private_key_path(&key_path, None);
    attach_cosign_signature(&registry.client()?, &subject, &signer, &registry.insecure()).await?;
    let verifier = CosignVerifier::from_pem_path(&public_path)?.with_insecure_registries(registry.insecure());
    assert_eq!(verifier.public_key_pem(), public.as_slice());
    verify_snapshot_signature(&verifier, &subject).await?;

    let missing = CosignSigner::from_private_key_path(dir.path().join("absent.key"), None);
    let outcome = attach_cosign_signature(&registry.client()?, &subject, &missing, &registry.insecure()).await;
    assert!(matches!(outcome, Err(CosignError::SignFailed(_))), "{outcome:?}");

    let outcome = CosignVerifier::from_pem_path(dir.path().join("absent.pub"));
    assert!(matches!(outcome, Err(CosignError::PublicKey(_))), "{outcome:?}");
    Ok(())
}

#[tokio::test]
async fn signing_reports_a_bad_key_password_reference_and_registry() -> TestResult {
    let registry = FakeRegistry::start().await?;
    let subject = registry.push_artifact(b"disk").await?;
    let client = registry.client()?;

    let signer = CosignSigner::from_private_key_pem(b"not a key".to_vec(), None);
    let outcome = attach_cosign_signature(&client, &subject, &signer, &registry.insecure()).await;
    assert!(matches!(outcome, Err(CosignError::SignFailed(_))), "{outcome:?}");

    let (private, _) = generate_test_key_pair()?;
    let password = Some(SecretString::from("hunter2"));
    let signer = CosignSigner::from_private_key_pem(private.clone(), password);
    let outcome = attach_cosign_signature(&client, &subject, &signer, &registry.insecure()).await;
    assert!(matches!(outcome, Err(CosignError::SignFailed(_))), "{outcome:?}");

    let signer = CosignSigner::from_private_key_pem(private, None);
    let bad = unreachable_ref("not a registry!")?;
    let outcome = attach_cosign_signature(&client, &bad, &signer, &[]).await;
    assert!(matches!(outcome, Err(CosignError::SignFailed(_))), "{outcome:?}");

    let missing = unreachable_ref(&registry.address)?;
    let outcome = attach_cosign_signature(&client, &missing, &signer, &registry.insecure()).await;
    assert!(matches!(outcome, Err(CosignError::SignFailed(_))), "{outcome:?}");
    Ok(())
}

#[tokio::test]
async fn a_registry_that_refuses_the_signature_manifest_is_reported() -> TestResult {
    let registry = FakeRegistry::start().await?;
    let subject = registry.push_artifact(b"disk").await?;
    let (private, _) = generate_test_key_pair()?;
    let signer = CosignSigner::from_private_key_pem(private, None);
    registry.reject_manifest_puts();

    let outcome = attach_cosign_signature(&registry.client()?, &subject, &signer, &registry.insecure()).await;
    assert!(matches!(outcome, Err(CosignError::Registry(_))), "{outcome:?}");
    Ok(())
}
