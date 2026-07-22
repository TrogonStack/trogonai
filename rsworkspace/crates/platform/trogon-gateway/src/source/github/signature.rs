use hmac::{Hmac, KeyInit, Mac};
use sha2::Sha256;

type HmacSha256 = Hmac<Sha256>;

#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum SignatureError {
    #[error("missing sha256= prefix")]
    MissingPrefix,
    #[error("invalid hex encoding")]
    InvalidHex(#[source] hex::FromHexError),
    #[error("invalid HMAC key")]
    InvalidKey(#[source] hmac::digest::InvalidLength),
    #[error("signature mismatch")]
    Mismatch,
}

/// Verifies a GitHub webhook signature using constant-time comparison.
///
/// GitHub sends `X-Hub-Signature-256: sha256=<hex>`. This function validates
/// the HMAC-SHA256 of the raw request body against that header value.
pub fn verify(secret: &str, body: &[u8], signature_header: &str) -> Result<(), SignatureError> {
    let hex_sig = signature_header
        .strip_prefix("sha256=")
        .ok_or(SignatureError::MissingPrefix)?;

    let expected = hex::decode(hex_sig).map_err(SignatureError::InvalidHex)?;

    let mut mac = HmacSha256::new_from_slice(secret.as_bytes()).map_err(SignatureError::InvalidKey)?;

    mac.update(body);
    mac.verify_slice(&expected).map_err(|_| SignatureError::Mismatch)
}

#[cfg(test)]
mod tests;
