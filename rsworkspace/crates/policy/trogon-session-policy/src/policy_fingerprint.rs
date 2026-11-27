use sha2::{Digest as _, Sha256};

/// The exact policy document bound to a session.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PolicyFingerprint([u8; 32]);

impl PolicyFingerprint {
    pub(crate) fn of(bytes: &[u8]) -> Self {
        Self(Sha256::digest(bytes).into())
    }

    pub fn bytes(self) -> [u8; 32] {
        self.0
    }
}
