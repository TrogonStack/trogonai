use async_trait::async_trait;

use crate::checkpoint_draft::CheckpointDraft;
use crate::checkpoint_sign_error::CheckpointSignError;
use crate::checkpoint_signature::CheckpointSignature;

/// Signs checkpoints. The port is an operation and never a key source: a signer
/// backed by KeyManagement (ADR#0023) signs without the key leaving custody, and
/// the writer never holds key material.
///
/// One call chooses the key and signs [`CheckpointDraft::signing_input`] under
/// it, and returns the key id with the signature, so a key rotated in between
/// cannot split them. It is async and object safe so a remote signer replaces
/// the local one without changing the writer.
#[async_trait]
pub trait CheckpointSigner: std::fmt::Debug + Send + Sync + 'static {
    async fn sign(&self, draft: &CheckpointDraft) -> Result<CheckpointSignature, CheckpointSignError>;
}
