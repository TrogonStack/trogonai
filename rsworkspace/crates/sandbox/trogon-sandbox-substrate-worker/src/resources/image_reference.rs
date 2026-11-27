use super::ImageDigestError;
use super::constants::MAX_IMAGE_REFERENCE_LEN;

pub fn validate_image_digest(reference: &str) -> Result<(), ImageDigestError> {
    if reference.len() > MAX_IMAGE_REFERENCE_LEN {
        return Err(ImageDigestError::TooLong);
    }
    let parsed: oci_client::Reference = reference.parse().map_err(|_| ImageDigestError::InvalidReference)?;
    let digest = parsed.digest().ok_or(ImageDigestError::NotPinned)?;
    let value = digest
        .strip_prefix("sha256:")
        .ok_or(ImageDigestError::UnsupportedAlgorithm)?;
    if value.len() != 64
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
    {
        return Err(ImageDigestError::MalformedDigest);
    }
    Ok(())
}
