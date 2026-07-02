use sha2::{Digest, Sha256};
use std::fmt;

/// A SHA-256 digest rendered as a lowercase hex string (64 characters).
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct Sha256Digest(String);

impl Sha256Digest {
    /// Hash arbitrary bytes and render the digest as lowercase hex.
    pub fn of(bytes: impl AsRef<[u8]>) -> Self {
        let mut hasher = Sha256::new();
        hasher.update(bytes.as_ref());
        let digest = hasher.finalize();
        Self(hex_encode(&digest))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for Sha256Digest {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

fn hex_encode(bytes: &[u8]) -> String {
    const HEX_CHARS: &[u8; 16] = b"0123456789abcdef";
    let mut out = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        out.push(HEX_CHARS[(byte >> 4) as usize] as char);
        out.push(HEX_CHARS[(byte & 0x0f) as usize] as char);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hashes_are_64_hex_chars() {
        let digest = Sha256Digest::of(b"Search the web");
        assert_eq!(digest.as_str().len(), 64);
        assert!(digest.as_str().chars().all(|c| c.is_ascii_hexdigit()));
    }

    #[test]
    fn matches_known_vector() {
        // sha256("") is a well-known test vector.
        let digest = Sha256Digest::of(b"");
        assert_eq!(
            digest.as_str(),
            "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
        );
    }

    #[test]
    fn deterministic() {
        assert_eq!(Sha256Digest::of(b"same input"), Sha256Digest::of(b"same input"));
    }

    #[test]
    fn differs_on_different_input() {
        assert_ne!(Sha256Digest::of(b"a"), Sha256Digest::of(b"b"));
    }
}
