use std::fmt;

use hmac::{Hmac, KeyInit, Mac};
use sha2::Sha256;

type HmacSha256 = Hmac<Sha256>;

#[derive(Debug)]
#[non_exhaustive]
pub enum SignatureError {
    InvalidHex(hex::FromHexError),
    InvalidKey,
    Mismatch,
}

impl fmt::Display for SignatureError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            SignatureError::InvalidHex(_) => f.write_str("invalid hex encoding"),
            SignatureError::InvalidKey => f.write_str("invalid HMAC key"),
            SignatureError::Mismatch => f.write_str("signature mismatch"),
        }
    }
}

impl std::error::Error for SignatureError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            SignatureError::InvalidHex(error) => Some(error),
            SignatureError::InvalidKey => None,
            SignatureError::Mismatch => None,
        }
    }
}

pub fn verify(secret: &str, body: &[u8], signature_header: &str) -> Result<(), SignatureError> {
    let expected = hex::decode(signature_header).map_err(SignatureError::InvalidHex)?;
    let mut mac = HmacSha256::new_from_slice(secret.as_bytes()).map_err(|_| SignatureError::InvalidKey)?;
    mac.update(body);
    mac.verify_slice(&expected).map_err(|_| SignatureError::Mismatch)
}

#[cfg(test)]
mod tests;
