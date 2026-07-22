use bytes::Bytes;
use sha2::{Digest, Sha256};

/// Computes lowercase hex SHA-256 for immutable artifact content.
pub fn sha256_hex(content: &Bytes) -> String {
    let digest = Sha256::digest(content);
    hex::encode(digest)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sha256_is_lowercase_hex() {
        let content = Bytes::from_static(b"hello artifact");
        let digest = sha256_hex(&content);
        assert_eq!(digest.len(), 64);
        assert!(digest.chars().all(|ch| ch.is_ascii_hexdigit() && !ch.is_ascii_uppercase()));
    }
}
