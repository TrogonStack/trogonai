use std::fmt;

pub use crate::challenge_kid::{ChallengeKid, EmptyChallengeKidError};
pub use crate::nonce_ttl::{NonceTtl, NonceTtlError};
pub use crate::server_nonces::{
    ChallengeKey, ChallengeKeyError, NonceChallenge, NonceError, NonceIssueError, NonceRequirement, ServerNonces,
};

/// A nonce as it travels on the wire.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ServerNonce(String);

impl ServerNonce {
    pub(crate) fn new(value: String) -> Self {
        Self(value)
    }

    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for ServerNonce {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}
