use hmac::{Hmac, KeyInit, Mac};
use sha2::Sha256;

type HmacSha256 = Hmac<Sha256>;

#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum SignatureError {
    #[error("invalid hex encoding")]
    InvalidHex(#[source] hex::FromHexError),
    #[error("invalid HMAC key")]
    InvalidKey,
    #[error("signature mismatch")]
    Mismatch,
}

pub fn verify(secret: &str, body: &[u8], signature_header: &str) -> Result<(), SignatureError> {
    let expected = hex::decode(signature_header).map_err(SignatureError::InvalidHex)?;
    let mut mac = HmacSha256::new_from_slice(secret.as_bytes()).map_err(|_| SignatureError::InvalidKey)?;
    mac.update(body);
    mac.verify_slice(&expected).map_err(|_| SignatureError::Mismatch)
}

#[cfg(test)]
mod tests;
