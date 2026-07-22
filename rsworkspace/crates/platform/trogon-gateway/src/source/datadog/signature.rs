use sha2::{Digest, Sha256};

#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum SignatureError {
    #[error("missing webhook token header")]
    Missing,
    #[error("webhook token mismatch")]
    Mismatch,
}

/// Verifies the Datadog webhook shared secret token using constant-time
/// comparison.
///
/// Datadog does not sign webhook requests, so authentication relies on a shared
/// secret the operator delivers in a custom header. Both sides are hashed with
/// SHA-256 before comparing, ensuring equal-length slices regardless of input
/// length. This prevents leaking the secret's length via timing.
pub fn verify(secret: &str, token_header: Option<&str>) -> Result<(), SignatureError> {
    let token = token_header.ok_or(SignatureError::Missing)?;

    let expected = Sha256::digest(secret.as_bytes());
    let provided = Sha256::digest(token.as_bytes());

    let ok = subtle::ConstantTimeEq::ct_eq(expected.as_slice(), provided.as_slice()).unwrap_u8();
    if ok == 1 { Ok(()) } else { Err(SignatureError::Mismatch) }
}

#[cfg(test)]
mod tests;
