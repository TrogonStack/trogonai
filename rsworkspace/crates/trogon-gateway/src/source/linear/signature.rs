use hmac::{Hmac, KeyInit, Mac};
use sha2::Sha256;

type HmacSha256 = Hmac<Sha256>;

/// Verifies a Linear webhook signature using constant-time comparison.
///
/// Linear sends `linear-signature: <hex>` — a raw lowercase hex-encoded
/// HMAC-SHA256 of the request body, with no prefix.
pub fn verify(secret: &str, body: &[u8], signature_header: &str) -> bool {
    let Ok(expected) = hex::decode(signature_header) else {
        return false;
    };

    let Ok(mut mac) = HmacSha256::new_from_slice(secret.as_bytes()) else {
        return false;
    };

    mac.update(body);
    mac.verify_slice(&expected).is_ok()
}

#[cfg(test)]
mod tests;
