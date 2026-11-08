use ed25519_dalek::{PUBLIC_KEY_LENGTH, VerifyingKey};

use crate::constants::{DID_KEY_PREFIX, ED25519_MULTICODEC, MULTIBASE_BASE58BTC};

/// The key a checkpoint is signed with, named as a `did:key`: the JWS `kid` and
/// the statement's `key_id` (ADR#0038 Decision 1).
///
/// A `did:key` carries the public key itself, so a verifier needs no key table,
/// only the set of key ids it trusts. A rotated key is a different key id.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct KeyId(VerifyingKey);

#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum KeyIdError {
    #[error("key id is not a did:key in multibase base58btc")]
    NotDidKey,
    #[error("key id is not base58btc")]
    NotBase58,
    #[error("key id does not name an Ed25519 public key")]
    NotEd25519,
    #[error("key id names bytes that are not an Ed25519 public key")]
    InvalidPublicKey,
}

impl KeyId {
    pub fn from_verifying_key(key: VerifyingKey) -> Self {
        Self(key)
    }

    /// Reads a `did:key` for an Ed25519 key. Base58btc has one spelling per byte
    /// string, so the key id a key renders is the only one that parses to it.
    pub fn parse(value: &str) -> Result<Self, KeyIdError> {
        let encoded = value
            .strip_prefix(DID_KEY_PREFIX)
            .and_then(|multibase| multibase.strip_prefix(MULTIBASE_BASE58BTC))
            .ok_or(KeyIdError::NotDidKey)?;
        let decoded = bs58::decode(encoded).into_vec().map_err(|_| KeyIdError::NotBase58)?;
        let public_key: &[u8; PUBLIC_KEY_LENGTH] = decoded
            .strip_prefix(&ED25519_MULTICODEC)
            .and_then(|key| key.try_into().ok())
            .ok_or(KeyIdError::NotEd25519)?;
        VerifyingKey::from_bytes(public_key)
            .map(Self)
            .map_err(|_| KeyIdError::InvalidPublicKey)
    }

    pub fn verifying_key(&self) -> &VerifyingKey {
        &self.0
    }
}

impl std::fmt::Display for KeyId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let mut tagged = ED25519_MULTICODEC.to_vec();
        tagged.extend_from_slice(self.0.as_bytes());
        write!(
            f,
            "{DID_KEY_PREFIX}{MULTIBASE_BASE58BTC}{}",
            bs58::encode(tagged).into_string()
        )
    }
}

#[cfg(test)]
mod tests;
