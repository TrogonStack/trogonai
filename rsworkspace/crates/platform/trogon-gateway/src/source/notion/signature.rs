use hmac::{Hmac, KeyInit, Mac};
use sha2::Sha256;

use super::NotionVerificationToken;

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

pub fn verify(secret: &NotionVerificationToken, body: &[u8], signature_header: &str) -> Result<(), SignatureError> {
    let hex_signature = signature_header
        .strip_prefix("sha256=")
        .ok_or(SignatureError::MissingPrefix)?;

    let expected = hex::decode(hex_signature).map_err(SignatureError::InvalidHex)?;

    let mut mac = HmacSha256::new_from_slice(secret.as_str().as_bytes()).map_err(SignatureError::InvalidKey)?;
    mac.update(body);
    mac.verify_slice(&expected).map_err(|_| SignatureError::Mismatch)
}

#[cfg(test)]
mod tests;
