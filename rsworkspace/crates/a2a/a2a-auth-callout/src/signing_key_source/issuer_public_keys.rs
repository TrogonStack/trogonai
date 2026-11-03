use super::verifying_key::{VerifyingKey, VerifyingKeySource};
use crate::admission_keys::{AccountPublicKeyError, IssuerPublicKey};

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum IssuerPublicKeysError {
    #[error("lists no issuer public key")]
    Empty,
    #[error("entry {position} is not an account public key: it {source}")]
    Invalid {
        position: usize,
        #[source]
        source: AccountPublicKeyError,
    },
    #[error("lists issuer public key {0} more than once")]
    Duplicate(IssuerPublicKey),
}

/// The account public keys a verify-only process accepts as issuers of minted
/// user JWTs: the current key and, while a rotation overlaps, the previous one.
#[derive(Debug, Clone)]
pub struct IssuerPublicKeys(Vec<IssuerPublicKey>);

impl IssuerPublicKeys {
    pub fn new(keys: impl IntoIterator<Item = IssuerPublicKey>) -> Result<Self, IssuerPublicKeysError> {
        let mut listed: Vec<IssuerPublicKey> = Vec::new();
        for key in keys {
            if listed.contains(&key) {
                return Err(IssuerPublicKeysError::Duplicate(key));
            }
            listed.push(key);
        }
        if listed.is_empty() {
            return Err(IssuerPublicKeysError::Empty);
        }
        Ok(Self(listed))
    }

    /// Parses a comma-separated list of account public keys. Blank entries are
    /// skipped. A rejected entry is named by position and never echoed, since
    /// the operator may have pasted a seed.
    pub fn parse(raw: &str) -> Result<Self, IssuerPublicKeysError> {
        let mut keys = Vec::new();
        for (index, entry) in raw.split(',').enumerate() {
            let entry = entry.trim();
            if entry.is_empty() {
                continue;
            }
            keys.push(
                IssuerPublicKey::parse(entry).map_err(|source| IssuerPublicKeysError::Invalid {
                    position: index + 1,
                    source,
                })?,
            );
        }
        Self::new(keys)
    }

    pub fn keys(&self) -> &[IssuerPublicKey] {
        &self.0
    }
}

impl VerifyingKeySource for IssuerPublicKeys {
    fn accepted_verifying_keys(&self) -> Vec<VerifyingKey> {
        self.0.iter().map(VerifyingKey::from_issuer).collect()
    }
}
