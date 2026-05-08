use std::fmt;

use base64::Engine;
use base64::engine::general_purpose::STANDARD;
use hmac::{Hmac, KeyInit, Mac};
use sha2::Sha256;

type HmacSha256 = Hmac<Sha256>;

#[derive(Debug)]
#[non_exhaustive]
pub enum SignatureError {
    MissingPrefix,
    InvalidBase64(base64::DecodeError),
    Mismatch,
}

impl fmt::Display for SignatureError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::MissingPrefix => f.write_str("missing sha256= prefix"),
            Self::InvalidBase64(_) => f.write_str("invalid base64 encoding"),
            Self::Mismatch => f.write_str("signature mismatch"),
        }
    }
}

impl std::error::Error for SignatureError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::InvalidBase64(error) => Some(error),
            _ => None,
        }
    }
}

pub fn crc_response_token(consumer_secret: &str, crc_token: &str) -> String {
    let mut mac = HmacSha256::new_from_slice(consumer_secret.as_bytes()).expect("HMAC-SHA256 accepts any key length");
    mac.update(crc_token.as_bytes());
    Ok(format!("sha256={}", STANDARD.encode(mac.finalize().into_bytes())))
}

pub fn verify(consumer_secret: &str, body: &[u8], signature_header: &str) -> Result<(), SignatureError> {
    let encoded_signature = signature_header
        .strip_prefix("sha256=")
        .ok_or(SignatureError::MissingPrefix)?;

    let expected = STANDARD
        .decode(encoded_signature)
        .map_err(SignatureError::InvalidBase64)?;

    let mut mac = HmacSha256::new_from_slice(consumer_secret.as_bytes()).map_err(SignatureError::InvalidKey)?;
    mac.update(body);
    mac.verify_slice(&expected).map_err(|_| SignatureError::Mismatch)
}

#[cfg(test)]
mod tests;
