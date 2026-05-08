use std::fmt;

use hmac::{Hmac, KeyInit, Mac};
use sha2::Sha256;

use super::NotionVerificationToken;

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
            Self::MissingPrefix => f.write_str("missing sha256= prefix"),
            Self::InvalidHex(_) => f.write_str("invalid hex encoding"),
            Self::Mismatch => f.write_str("signature mismatch"),
        }
    }
}

impl std::error::Error for SignatureError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::InvalidHex(err) => Some(err),
            Self::MissingPrefix | Self::Mismatch => None,
        }
    }
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
