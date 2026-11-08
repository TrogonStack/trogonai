use async_trait::async_trait;
use ed25519_dalek::{Signer, SigningKey};

use crate::checkpoint_draft::CheckpointDraft;
use crate::checkpoint_sign_error::CheckpointSignError;
use crate::checkpoint_signature::CheckpointSignature;
use crate::checkpoint_signer::CheckpointSigner;
use crate::key_id::KeyId;

/// Signs checkpoints with an Ed25519 key held in this process.
///
/// It is for local and development use until the signing key has a custody
/// tier (ADR#0033). It loads no key itself: the caller hands one over, and the
/// key is zeroized when the signer drops and never appears in `Debug`.
pub struct LocalCheckpointSigner {
    key: SigningKey,
    key_id: KeyId,
}

impl LocalCheckpointSigner {
    pub fn new(key: SigningKey) -> Self {
        let key_id = KeyId::from_verifying_key(key.verifying_key());
        Self { key, key_id }
    }

    pub fn key_id(&self) -> KeyId {
        self.key_id
    }
}

impl std::fmt::Debug for LocalCheckpointSigner {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LocalCheckpointSigner")
            .field("key_id", &format_args!("{}", self.key_id))
            .finish_non_exhaustive()
    }
}

#[async_trait]
impl CheckpointSigner for LocalCheckpointSigner {
    async fn sign(&self, draft: &CheckpointDraft) -> Result<CheckpointSignature, CheckpointSignError> {
        let signature = self.key.sign(&draft.signing_input(&self.key_id));
        Ok(CheckpointSignature::new(self.key_id, signature))
    }
}

#[cfg(test)]
mod tests;
