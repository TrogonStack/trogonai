use serde::{Deserialize, Serialize};

/// Fresh entropy a restored guest feeds to its kernel before it serves a lease.
///
/// The bytes are secret, so the debug form never prints them.
#[derive(Clone, Eq, PartialEq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct GuestEntropySeed([u8; 32]);

impl GuestEntropySeed {
    pub const LEN: usize = 32;

    pub fn new(bytes: [u8; Self::LEN]) -> Self {
        Self(bytes)
    }

    pub fn as_bytes(&self) -> &[u8; Self::LEN] {
        &self.0
    }
}

impl std::fmt::Debug for GuestEntropySeed {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("GuestEntropySeed([redacted])")
    }
}
