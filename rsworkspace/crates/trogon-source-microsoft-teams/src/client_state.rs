use std::fmt;

use sha2::{Digest, Sha256};
use subtle::ConstantTimeEq;
use trogon_std::{EmptySecret, SecretString};

#[derive(Clone)]
pub struct MicrosoftTeamsClientState(SecretString);

impl MicrosoftTeamsClientState {
    pub fn new(s: impl AsRef<str>) -> Result<Self, EmptySecret> {
        SecretString::new(s).map(Self)
    }

    pub fn as_str(&self) -> &str {
        self.0.as_str()
    }

    pub fn matches(&self, provided: &str) -> bool {
        let expected = Sha256::digest(self.as_str().as_bytes());
        let provided = Sha256::digest(provided.as_bytes());
        expected.as_slice().ct_eq(provided.as_slice()).unwrap_u8() == 1
    }
}

impl fmt::Debug for MicrosoftTeamsClientState {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("MicrosoftTeamsClientState(****)")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn client_state_roundtrips() {
        let client_state = MicrosoftTeamsClientState::new("super-secret").unwrap();
        assert_eq!(client_state.as_str(), "super-secret");
    }

    #[test]
    fn client_state_debug_redacts() {
        let client_state = MicrosoftTeamsClientState::new("super-secret").unwrap();
        assert_eq!(format!("{client_state:?}"), "MicrosoftTeamsClientState(****)");
    }

    #[test]
    fn matches_equal_client_state() {
        let client_state = MicrosoftTeamsClientState::new("super-secret").unwrap();
        assert!(client_state.matches("super-secret"));
    }

    #[test]
    fn rejects_different_client_state() {
        let client_state = MicrosoftTeamsClientState::new("super-secret").unwrap();
        assert!(!client_state.matches("wrong-secret"));
    }
}
