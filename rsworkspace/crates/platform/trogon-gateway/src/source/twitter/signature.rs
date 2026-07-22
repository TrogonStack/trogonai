use base64::Engine;
use base64::engine::general_purpose::STANDARD;
use hmac::{Hmac, KeyInit, Mac};
use sha2::Sha256;

type HmacSha256 = Hmac<Sha256>;

#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum SignatureError {
    #[error("missing sha256= prefix")]
    MissingPrefix,
    #[error("invalid base64 encoding")]
    InvalidBase64(#[source] base64::DecodeError),
    #[error("invalid HMAC key")]
    InvalidKey(#[source] hmac::digest::InvalidLength),
    #[error("signature mismatch")]
    Mismatch,
}

pub fn crc_response_token(consumer_secret: &str, crc_token: &str) -> Result<String, SignatureError> {
    let mut mac = HmacSha256::new_from_slice(consumer_secret.as_bytes()).map_err(SignatureError::InvalidKey)?;
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
