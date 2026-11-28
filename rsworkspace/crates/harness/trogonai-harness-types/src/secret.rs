//! Bytes that are worth overwriting before they are given back.
//!
//! [`crate::Credential`] is the same idea for a value that is known to be a
//! non-empty credential. This one is for what came back off the wire, which may
//! be empty, may not be UTF-8, and is not yet known to be a token at all.

use std::fmt;

/// Bytes that are overwritten when they are dropped.
///
/// There is no `Clone`, no `Display` and no serde. A second copy of a secret is
/// a second thing to overwrite, and every way of printing one is a way of
/// leaking one, so reading the bytes means naming [`SecretBytes::expose`].
#[derive(Default, PartialEq, Eq)]
pub struct SecretBytes(Vec<u8>);

impl SecretBytes {
    /// Holds nothing yet.
    #[must_use]
    pub const fn empty() -> Self {
        Self(Vec::new())
    }

    /// Takes ownership of bytes read off the wire.
    #[must_use]
    pub fn new(bytes: Vec<u8>) -> Self {
        Self(bytes)
    }

    /// Takes a copy of text that has to be protected.
    #[must_use]
    pub fn from_text(text: &str) -> Self {
        Self(text.as_bytes().to_vec())
    }

    /// The bytes themselves, for the one caller that has to parse or send them.
    #[must_use]
    pub fn expose(&self) -> &[u8] {
        &self.0
    }

    /// The bytes as text, or nothing at all if they are not text.
    ///
    /// Answers the empty string rather than failing, the way
    /// [`crate::Credential::expose`] does: a caller holding bytes that are
    /// not UTF-8 has nothing to send, and a request with an empty body is
    /// refused by the far end rather than by a panic here.
    #[must_use]
    pub fn expose_text(&self) -> &str {
        std::str::from_utf8(&self.0).unwrap_or_default()
    }

    /// How many bytes are held.
    #[must_use]
    pub fn len(&self) -> usize {
        self.0.len()
    }

    /// Whether anything is held.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    /// Appends text that is part of the same secret.
    pub fn push_text(&mut self, text: &str) {
        self.push_bytes(text.as_bytes());
    }

    /// Appends bytes that are part of the same secret.
    ///
    /// A body arriving in chunks is the reason this is not the caller's job:
    /// collecting into a plain `Vec` and wrapping it at the end leaves every
    /// shorter self that `Vec` grew out of behind unscrubbed, each holding a
    /// prefix of the secret.
    pub fn push_bytes(&mut self, bytes: &[u8]) {
        self.0.extend_from_slice(bytes);
    }

    /// Appends one byte that is part of the same secret.
    pub fn push(&mut self, byte: u8) {
        self.0.push(byte);
    }

    /// Removes the last byte, overwriting it on the way out.
    ///
    /// The byte is cleared before it is dropped rather than after, because
    /// shortening a buffer leaves what was there in the spare capacity, where
    /// nothing else will ever get around to scrubbing it.
    pub fn pop(&mut self) -> Option<u8> {
        let last = self.0.last_mut()?;
        let byte = *last;
        *last = 0;
        self.0.pop();
        Some(byte)
    }

    /// How much can be held before the buffer has to move.
    #[must_use]
    pub fn capacity(&self) -> usize {
        self.0.capacity()
    }

    /// Makes room for `capacity` bytes in one go.
    ///
    /// Worth naming for a secret that arrives a byte at a time: a buffer that
    /// grows as it fills leaves each of its earlier, shorter selves behind
    /// unscrubbed, holding a prefix of the secret. Reserving the ceiling before
    /// the first byte means there is only ever one buffer to overwrite.
    pub fn reserve(&mut self, capacity: usize) {
        if self.0.capacity() < capacity {
            self.0.reserve_exact(capacity - self.0.len());
        }
    }
}

impl fmt::Debug for SecretBytes {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "SecretBytes({} bytes redacted)", self.0.len())
    }
}

impl Drop for SecretBytes {
    fn drop(&mut self) {
        self.0.resize(self.0.capacity(), 0);
        self.0.fill(0);
    }
}

#[cfg(test)]
mod tests {
    use super::SecretBytes;

    #[test]
    fn a_secret_reports_its_size_and_none_of_its_bytes() {
        let secret = SecretBytes::from_text("device-code");
        assert_eq!(format!("{secret:?}"), "SecretBytes(11 bytes redacted)");
    }

    #[test]
    fn text_that_is_not_text_reads_as_nothing_rather_than_panicking() {
        let secret = SecretBytes::new(vec![0xff, 0xfe]);
        assert_eq!(secret.expose_text(), "");
        assert_eq!(secret.expose(), &[0xff, 0xfe]);
    }

    #[test]
    fn a_secret_can_be_assembled_a_piece_at_a_time() {
        let mut secret = SecretBytes::empty();
        assert!(secret.is_empty());
        secret.push_text("token=");
        secret.push(b'a');
        assert_eq!(secret.expose_text(), "token=a");
        assert_eq!(secret.len(), 7);
    }
}
