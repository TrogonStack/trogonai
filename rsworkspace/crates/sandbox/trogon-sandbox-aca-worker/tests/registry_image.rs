use trogon_sandbox_aca_worker::{AcaError, image_env_vars, resolve_registry_image};
use trogon_sandbox_contract::{Digest, OciImageRef, SandboxImage};

type TestResult = Result<(), Box<dyn std::error::Error>>;

fn registry(reference: OciImageRef) -> SandboxImage {
    SandboxImage::Registry {
        reference,
        pull_secret: None,
    }
}

#[test]
fn an_unnamed_image_boots_the_default_image() -> TestResult {
    let default = OciImageRef::parse_unqualified("registry.example/default:2");

    let resolved = resolve_registry_image(&registry(OciImageRef::parse_unqualified("")), Some(&default))?;

    assert_eq!(resolved, default);
    Ok(())
}

#[test]
fn an_unnamed_image_without_a_default_is_refused() {
    let result = resolve_registry_image(&registry(OciImageRef::parse_unqualified("")), None);

    assert!(matches!(result, Err(AcaError::InvalidSpec(_))));
}

#[test]
fn a_named_image_is_used_as_given() -> TestResult {
    let named = OciImageRef::parse_unqualified("registry.example/app:1");

    let resolved = resolve_registry_image(&registry(named.clone()), None)?;

    assert_eq!(resolved, named);
    Ok(())
}

#[test]
fn env_vars_describe_the_image_tag_and_digest() -> TestResult {
    let digest = Digest::new(format!("sha256:{}", "a".repeat(64)))?;
    let mut reference = OciImageRef::parse_unqualified("registry.example/app:1");
    reference.digest = Some(digest.clone());

    let vars = image_env_vars(&reference);

    assert_eq!(vars.len(), 3);
    assert_eq!(vars[1], ("SANDBOX_ACA_IMAGE_TAG", "1".to_owned()));
    assert_eq!(vars[2], ("SANDBOX_ACA_IMAGE_DIGEST", digest.as_str().to_owned()));
    Ok(())
}

#[test]
fn env_vars_of_a_bare_image_name_only_the_image() {
    let vars = image_env_vars(&OciImageRef::parse_unqualified("registry.example/app"));

    assert_eq!(vars.len(), 1);
}
