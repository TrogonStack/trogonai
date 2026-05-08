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
            SignatureError::MissingPrefix => f.write_str("missing sha256= prefix"),
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
