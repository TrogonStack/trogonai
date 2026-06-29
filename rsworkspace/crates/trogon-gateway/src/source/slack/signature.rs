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

/// Verifies a Slack request signature using constant-time comparison.
///
/// Slack sends `X-Slack-Signature: v0=<hex>` and `X-Slack-Request-Timestamp`.
/// The HMAC-SHA256 is computed over `v0:{timestamp}:{body}`.
pub fn verify(
    signing_secret: &str,
    timestamp: &str,
    body: &[u8],
    signature_header: &str,
) -> Result<(), SignatureError> {
    let hex_sig = signature_header
        .strip_prefix("v0=")
        .ok_or(SignatureError::MissingPrefix)?;

    let expected = hex::decode(hex_sig).map_err(SignatureError::InvalidHex)?;

    let mut mac = HmacSha256::new_from_slice(signing_secret.as_bytes()).map_err(SignatureError::InvalidKey)?;

    mac.update(b"v0:");
    mac.update(timestamp.as_bytes());
    mac.update(b":");
    mac.update(body);

    mac.verify_slice(&expected).map_err(|_| SignatureError::Mismatch)
}

#[cfg(test)]
mod tests;
