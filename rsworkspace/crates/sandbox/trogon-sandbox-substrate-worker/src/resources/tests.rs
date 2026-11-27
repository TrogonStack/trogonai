use super::*;

const SHA256: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";

#[test]
fn resources_in_range_are_kept() -> Result<(), ActorResourcesError> {
    let resources = ActorResources::new(1000, 2048)?;
    assert_eq!(resources.cpu_millicores(), 1000);
    assert_eq!(resources.memory_mib(), 2048);
    Ok(())
}

#[test]
fn resources_outside_the_supported_range_are_refused() {
    assert_eq!(ActorResources::new(0, 1), Err(ActorResourcesError));
    assert_eq!(ActorResources::new(1_000_000, 1), Err(ActorResourcesError));
    assert_eq!(ActorResources::new(1, 0), Err(ActorResourcesError));
}

#[test]
fn resources_convert_to_the_control_plane_limits() -> Result<(), ActorResourcesError> {
    let proto = ActorResources::new(500, 2)?.as_proto();
    let limits = proto.limits.ok_or(ActorResourcesError)?;
    assert_eq!(limits.cpu_millicores, 500);
    assert_eq!(limits.memory_bytes, 2 * 1024 * 1024);
    Ok(())
}

#[test]
fn a_digest_pinned_reference_is_accepted() {
    assert_eq!(
        validate_image_digest(&format!("registry.example/agent@sha256:{SHA256}")),
        Ok(())
    );
}

#[test]
fn an_overlong_reference_is_refused() {
    assert_eq!(validate_image_digest(&"a".repeat(513)), Err(ImageDigestError::TooLong));
}

#[test]
fn a_malformed_reference_is_refused() {
    assert_eq!(
        validate_image_digest("Not A Reference!"),
        Err(ImageDigestError::InvalidReference)
    );
}

#[test]
fn a_tag_without_a_digest_is_refused() {
    assert_eq!(
        validate_image_digest("registry.example/agent:latest"),
        Err(ImageDigestError::NotPinned)
    );
}

#[test]
fn only_sha256_digests_are_supported() {
    assert_eq!(
        validate_image_digest(&format!("registry.example/agent@sha512:{SHA256}{SHA256}")),
        Err(ImageDigestError::UnsupportedAlgorithm)
    );
}

#[test]
fn a_short_or_uppercase_digest_is_refused() {
    assert_eq!(
        validate_image_digest("registry.example/agent@sha256:abcd"),
        Err(ImageDigestError::InvalidReference)
    );
    assert_eq!(
        validate_image_digest(&format!(
            "registry.example/agent@sha256:{}",
            SHA256.to_ascii_uppercase()
        )),
        Err(ImageDigestError::MalformedDigest)
    );
}
