//! Naming a file on disk after the bytes it holds.

use core::fmt;

use crate::ContentHash;

/// How many hex characters of the digest a sealed handle carries.
pub const DIGEST_HEX_LEN: usize = 16;

/// Text that is not a handle with the expected suffix.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct InvalidArtifactHandle;

impl fmt::Display for InvalidArtifactHandle {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("an artifact handle must end with its suffix")
    }
}

impl std::error::Error for InvalidArtifactHandle {}

/// The name of one session sidecar file, split at its suffix.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct ArtifactHandle {
    text: String,
    suffix_len: usize,
}

impl ArtifactHandle {
    /// A handle, if the text ends with the suffix it is supposed to have.
    pub fn new(text: impl Into<String>, suffix: &str) -> Result<Self, InvalidArtifactHandle> {
        let text = text.into();
        if suffix.is_empty() || !text.ends_with(suffix) {
            return Err(InvalidArtifactHandle);
        }
        Ok(Self {
            text,
            suffix_len: suffix.len(),
        })
    }

    /// The whole name, as it appears on disk.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.text
    }

    /// The name without its suffix.
    #[must_use]
    pub fn stem(&self) -> &str {
        &self.text[..self.text.len() - self.suffix_len]
    }

    /// The suffix.
    #[must_use]
    pub fn suffix(&self) -> &str {
        &self.text[self.text.len() - self.suffix_len..]
    }

    /// The same handle with the digest of its contents appended to the stem.
    #[must_use]
    pub fn content_addressed(&self, digest: &ContentHash) -> Self {
        let mut text = String::with_capacity(self.text.len() + DIGEST_HEX_LEN + 1);
        text.push_str(self.stem());
        text.push('-');
        text.push_str(&digest_prefix_hex(digest));
        text.push_str(self.suffix());
        Self {
            text,
            suffix_len: self.suffix_len,
        }
    }

    /// The digest this handle carries, when it carries one at all.
    #[must_use]
    pub fn digest_hex(&self) -> Option<&str> {
        let stem = self.stem();
        let separator = stem.rfind('-')?;
        let encoded = &stem[separator + 1..];
        if encoded.len() != DIGEST_HEX_LEN {
            return None;
        }
        if !encoded
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
        {
            return None;
        }
        Some(encoded)
    }

    /// Whether this handle names exactly the bytes that hash to this digest.
    #[must_use]
    pub fn matches_digest(&self, digest: &ContentHash) -> bool {
        self.digest_hex() == Some(digest_prefix_hex(digest).as_str())
    }
}

fn digest_prefix_hex(digest: &ContentHash) -> String {
    crate::hex::encode(&digest.as_bytes()[..DIGEST_HEX_LEN / 2])
}

impl fmt::Display for ArtifactHandle {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.text)
    }
}

#[cfg(test)]
mod tests {
    use super::{ArtifactHandle, InvalidArtifactHandle};
    use crate::ContentHash;

    #[test]
    fn a_content_addressed_handle_authenticates_exact_bytes() {
        let digest = ContentHash::of(b"saved output");
        let handle = ArtifactHandle::new("trogonai-command-123.log", ".log").expect("a .log handle");
        let sealed = handle.content_addressed(&digest);

        assert_eq!(sealed.as_str(), "trogonai-command-123-1b2a9cb5a0298dcb.log");
        assert!(sealed.matches_digest(&digest));
        assert!(!sealed.matches_digest(&ContentHash::of(b"xaved output")));
    }

    #[test]
    fn a_handle_that_was_never_sealed_carries_no_digest() {
        let handle = ArtifactHandle::new("trogonai-command-123.log", ".log").expect("a .log handle");
        assert!(handle.digest_hex().is_none());
        assert!(!handle.matches_digest(&ContentHash::of(b"saved output")));
    }

    #[test]
    fn a_stem_that_only_looks_like_a_digest_is_not_one() {
        let uppercase = ArtifactHandle::new("trogonai-command-1B2A9CB5A0298DCB.log", ".log").expect("a .log handle");
        assert!(uppercase.digest_hex().is_none());

        let too_short = ArtifactHandle::new("trogonai-command-1b2a9cb5.log", ".log").expect("a handle");
        assert!(too_short.digest_hex().is_none());
    }

    #[test]
    fn a_handle_is_only_a_handle_alongside_the_suffix_it_ends_with() {
        assert_eq!(
            ArtifactHandle::new("trogonai-command-123.log", ".bin"),
            Err(InvalidArtifactHandle)
        );
        assert_eq!(
            ArtifactHandle::new("trogonai-command-123.log", ""),
            Err(InvalidArtifactHandle)
        );
    }

    #[test]
    fn sealing_keeps_the_suffix_where_it_was() {
        let handle = ArtifactHandle::new("trogonai-command-replay-9-1-0.bin", ".bin").expect("a handle");
        let sealed = handle.content_addressed(&ContentHash::of(b"frames"));
        assert_eq!(sealed.suffix(), ".bin");
        assert!(sealed.stem().starts_with("trogonai-command-replay-9-1-0-"));
    }
}
