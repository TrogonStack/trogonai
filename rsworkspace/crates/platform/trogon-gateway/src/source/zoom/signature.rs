use hmac::{Hmac, KeyInit, Mac};
use sha2::Sha256;

type HmacSha256 = Hmac<Sha256>;

#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum SignatureError {
    #[error("missing v0= prefix")]
    MissingPrefix,
    #[error("invalid hex encoding")]
    InvalidHex(#[source] hex::FromHexError),
    #[error("invalid HMAC key")]
    InvalidKey(#[source] hmac::digest::InvalidLength),
    #[error("signature mismatch")]
    Mismatch,
}

/// Verifies a Zoom webhook signature using constant-time comparison.
///
/// Zoom sends `x-zm-signature: v0=<hex>` and `x-zm-request-timestamp`.
/// The HMAC-SHA256 is computed over `v0:{timestamp}:{body}` keyed by the secret token.
pub fn verify(secret_token: &str, timestamp: &str, body: &[u8], signature_header: &str) -> Result<(), SignatureError> {
    let hex_sig = signature_header
        .strip_prefix("v0=")
        .ok_or(SignatureError::MissingPrefix)?;

    let expected = hex::decode(hex_sig).map_err(SignatureError::InvalidHex)?;

    let mut mac = HmacSha256::new_from_slice(secret_token.as_bytes()).map_err(SignatureError::InvalidKey)?;

    mac.update(b"v0:");
    mac.update(timestamp.as_bytes());
    mac.update(b":");
    mac.update(body);

    mac.verify_slice(&expected).map_err(|_| SignatureError::Mismatch)
}

/// Computes the `encryptedToken` Zoom expects in reply to `endpoint.url_validation`:
/// the hex-encoded HMAC-SHA256 of the plain token keyed by the secret token.
pub fn encrypt_plain_token(secret_token: &str, plain_token: &str) -> Result<String, SignatureError> {
    let mut mac = HmacSha256::new_from_slice(secret_token.as_bytes()).map_err(SignatureError::InvalidKey)?;
    mac.update(plain_token.as_bytes());
    Ok(hex::encode(mac.finalize().into_bytes()))
}

#[cfg(test)]
mod tests;
