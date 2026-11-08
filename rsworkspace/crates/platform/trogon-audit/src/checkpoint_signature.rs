use ed25519_dalek::Signature;

use crate::key_id::KeyId;

/// A checkpoint signature and the key that made it. One signing call returns
/// both, so a key rotated between choosing a key and signing cannot pair a
/// signature with another key's id.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CheckpointSignature {
    key: KeyId,
    signature: Signature,
}

impl CheckpointSignature {
    pub fn new(key: KeyId, signature: Signature) -> Self {
        Self { key, signature }
    }

    pub fn key(&self) -> &KeyId {
        &self.key
    }

    pub fn signature(&self) -> &Signature {
        &self.signature
    }
}
