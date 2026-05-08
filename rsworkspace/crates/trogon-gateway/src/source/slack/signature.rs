use std::fmt;

use hmac::{Hmac, KeyInit, Mac};
use sha2::Sha256;

type HmacSha256 = Hmac<Sha256>;

#[derive(Debug)]
#[non_exhaustive]
pub enum SignatureError {
    MissingPrefix,
    InvalidHex(hex::FromHexError),
    Mismatch,
}

impl fmt::Display for SignatureError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            SignatureError::MissingPrefix => f.write_str("missing v0= prefix"),
            SignatureError::InvalidHex(_) => f.write_str("invalid hex encoding"),
            SignatureError::Mismatch => f.write_str("signature mismatch"),
        }
    }
}

impl std::error::Error for SignatureError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            SignatureError::InvalidHex(e) => Some(e),
            _ => None,
        }
    }
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
