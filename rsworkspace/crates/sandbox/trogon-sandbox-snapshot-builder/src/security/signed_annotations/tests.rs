use super::*;

type TestResult = Result<(), Box<dyn std::error::Error>>;

fn digest() -> String {
    format!("sha256:{}", "a".repeat(64))
}

#[test]
fn annotations_that_are_not_the_environment_digest_are_not_signed() -> TestResult {
    let mut manifest = Annotations::new();
    manifest.insert("org.example.other".into(), "x".into());
    assert!(signed_annotations(&manifest)?.is_empty());
    assert!(manifest_env_digest(&manifest).is_none());
    Ok(())
}

#[test]
fn a_well_formed_environment_digest_is_signed() -> TestResult {
    let mut manifest = Annotations::new();
    manifest.insert(EnvDigest::ANNOTATION_KEY.into(), digest());
    let signed = signed_annotations(&manifest)?;
    assert_eq!(signed.len(), 1);
    assert_eq!(signed.get(EnvDigest::ANNOTATION_KEY), Some(&digest()));
    assert_eq!(manifest_env_digest(&manifest), Some(EnvDigest::new(digest())?));
    Ok(())
}

#[test]
fn a_malformed_environment_digest_is_refused() {
    let mut manifest = Annotations::new();
    manifest.insert(EnvDigest::ANNOTATION_KEY.into(), "not-a-digest".into());
    assert!(matches!(
        signed_annotations(&manifest),
        Err(SecurityError::MalformedEnvDigest { .. })
    ));
    assert!(manifest_env_digest(&manifest).is_none());
}
