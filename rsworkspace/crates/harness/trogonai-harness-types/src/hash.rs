//! Content digests used to tell whether a file is still what the runtime last saw.

use std::fmt;

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

/// A SHA-256 digest of file content.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct ContentHash([u8; 32]);

impl ContentHash {
    /// Digests bytes.
    #[must_use]
    pub fn of(bytes: &[u8]) -> Self {
        let mut hasher = Sha256::new();
        hasher.update(bytes);
        Self(hasher.finalize().into())
    }

    /// The digest as bytes.
    #[must_use]
    pub fn as_bytes(&self) -> &[u8; 32] {
        &self.0
    }

    /// Rebuilds a digest from bytes that were already computed.
    #[must_use]
    pub fn from_bytes(bytes: [u8; 32]) -> Self {
        Self(bytes)
    }

    /// The digest this text names, if it names one.
    ///
    /// Only the lowercase hex [`std::fmt::Display`] writes is read back, so a
    /// digest that would print differently than it was stored is refused rather
    /// than quietly canonicalized: a record whose bytes change on a round trip
    /// is a record two readers can disagree about.
    #[must_use]
    pub fn parse(text: &str) -> Option<Self> {
        let raw = text.as_bytes();
        if raw.len() != 64 {
            return None;
        }
        let mut bytes = [0_u8; 32];
        for (byte, pair) in bytes.iter_mut().zip(raw.as_chunks::<2>().0) {
            let mut value = 0_u8;
            for digit in pair {
                let nibble = match digit {
                    b'0'..=b'9' => digit - b'0',
                    b'a'..=b'f' => digit - b'a' + 10,
                    _ => return None,
                };
                value = value * 16 + nibble;
            }
            *byte = value;
        }
        Some(Self(bytes))
    }
}

/// A digest being taken over bytes that arrive a few at a time.
///
/// A command's output is digested as it is written, because the whole of it is
/// exactly what would not fit in memory in the first place.
#[derive(Clone, Default)]
pub struct ContentHasher(Sha256);

impl ContentHasher {
    /// A digest of nothing yet.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Takes in the next of the bytes being digested.
    pub fn update(&mut self, bytes: &[u8]) {
        self.0.update(bytes);
    }

    /// The digest of everything taken in.
    #[must_use]
    pub fn finish(self) -> ContentHash {
        ContentHash(self.0.finalize().into())
    }
}

impl fmt::Debug for ContentHasher {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("ContentHasher")
    }
}

impl fmt::Debug for ContentHash {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "ContentHash({self})")
    }
}

impl fmt::Display for ContentHash {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        for byte in &self.0 {
            write!(formatter, "{byte:02x}")?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_empty_file_and_a_written_one_are_told_apart() {
        assert_ne!(ContentHash::of(b""), ContentHash::of(b"x"));
        assert_eq!(ContentHash::of(b""), ContentHash::of(b""));
    }

    #[test]
    fn a_digest_reads_as_hex_so_it_can_be_compared_by_eye_in_a_log() {
        assert_eq!(
            ContentHash::of(b"").to_string(),
            "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
        );
    }

    #[test]
    fn a_digest_read_back_is_the_digest_that_was_written() {
        let digest = ContentHash::of(b"a file");
        assert_eq!(ContentHash::parse(&digest.to_string()), Some(digest));
    }

    #[test]
    fn text_that_is_not_how_a_digest_is_written_is_not_a_digest() {
        let written = ContentHash::of(b"").to_string();
        assert_eq!(ContentHash::parse(&written.to_uppercase()), None);
        assert_eq!(ContentHash::parse(&written[..63]), None);
        assert_eq!(ContentHash::parse(&format!("{written}0")), None);
        assert_eq!(ContentHash::parse(&format!("zz{}", &written[2..])), None);
    }
}
