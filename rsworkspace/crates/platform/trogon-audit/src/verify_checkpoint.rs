//! Verifies a signed checkpoint against a chain head that verified (ADR#0086
//! Decision 6), offline and without a NATS client.

use std::time::SystemTime;

use buffa::Message;
use ed25519_dalek::Signature;
use trogon_nats::NatsToken;
use trogonai_proto::platform::audit::v1::{AuditCheckpoint, CheckpointStatement};

use crate::chain_id::ChainId;
use crate::checkpoint_id::CheckpointId;
use crate::checkpoint_jws;
use crate::checkpoint_verification_error::CheckpointVerificationError;
use crate::constants::{CHECKPOINT_JWS_ALGORITHM, CHECKPOINT_JWS_TYPE};
use crate::key_id::KeyId;
use crate::protected_header::ProtectedHeader;
use crate::time_bounds::TimeBounds;
use crate::trusted_keys::TrustedKeys;
use crate::verified_chain_head::VerifiedChainHead;
use crate::verified_checkpoint::VerifiedCheckpoint;

/// Checks `payload`, an `AuditCheckpoint` as stored, against `head`.
///
/// The detached JWS must carry the checkpoint type, EdDSA and a `kid` in
/// `trusted`, and its signature must verify over the stored statement bytes
/// exactly as they are. The statement must then name the same key, `head`'s
/// writer, chain, sequence, hash and stream sequence, and an issue time inside
/// `bounds`. The stream name is not compared: a verified head does not carry it,
/// and the stream sequence already places the head.
pub fn verify_checkpoint(
    payload: &[u8],
    head: &VerifiedChainHead,
    trusted: &TrustedKeys,
    bounds: &TimeBounds,
) -> Result<VerifiedCheckpoint, CheckpointVerificationError> {
    let checkpoint = AuditCheckpoint::decode_from_slice(payload).map_err(CheckpointVerificationError::Malformed)?;
    let key = verify_signature(&checkpoint, trusted)?;
    let statement = CheckpointStatement::decode_from_slice(&checkpoint.statement)
        .map_err(CheckpointVerificationError::MalformedStatement)?;
    if statement.key_id != key.to_string() {
        return Err(CheckpointVerificationError::KeyIdMismatch);
    }
    check_head(&statement, head)?;
    let issued_at = statement
        .issued_at
        .as_option()
        .and_then(|issued_at| SystemTime::try_from(issued_at.clone()).ok())
        .ok_or(CheckpointVerificationError::InvalidIssuedAt)?;
    if !bounds.contains(issued_at) {
        return Err(CheckpointVerificationError::IssuedOutsideBounds);
    }
    let id =
        CheckpointId::parse(&statement.checkpoint_id).map_err(|_| CheckpointVerificationError::InvalidCheckpointId)?;
    let component = NatsToken::new(&statement.component).map_err(|_| CheckpointVerificationError::InvalidComponent)?;
    Ok(VerifiedCheckpoint::new(id, issued_at, key, component, head.clone()))
}

/// The trusted key whose signature covers the stored statement. Nothing in the
/// statement is read before this succeeds.
fn verify_signature(checkpoint: &AuditCheckpoint, trusted: &TrustedKeys) -> Result<KeyId, CheckpointVerificationError> {
    let (header_segment, signature_segment) =
        checkpoint_jws::split(&checkpoint.detached_jws).ok_or(CheckpointVerificationError::MalformedJws)?;
    let header = ProtectedHeader::decode(header_segment).ok_or(CheckpointVerificationError::MalformedHeader)?;
    if header.typ != CHECKPOINT_JWS_TYPE {
        return Err(CheckpointVerificationError::WrongType);
    }
    if header.alg != CHECKPOINT_JWS_ALGORITHM {
        return Err(CheckpointVerificationError::AlgorithmNotAllowed);
    }
    let key = KeyId::parse(&header.kid).map_err(CheckpointVerificationError::MalformedKeyId)?;
    if !trusted.contains(&key) {
        return Err(CheckpointVerificationError::UntrustedKey { key_id: Box::new(key) });
    }
    let signature = checkpoint_jws::decode_segment(signature_segment)
        .and_then(|bytes| Signature::from_slice(&bytes).ok())
        .ok_or(CheckpointVerificationError::MalformedSignature)?;
    key.verifying_key()
        .verify_strict(
            &checkpoint_jws::signing_input(header_segment, &checkpoint.statement),
            &signature,
        )
        .map_err(|_| CheckpointVerificationError::BadSignature)?;
    Ok(key)
}

fn check_head(statement: &CheckpointStatement, head: &VerifiedChainHead) -> Result<(), CheckpointVerificationError> {
    if statement.writer_id != head.writer().as_str() {
        return Err(CheckpointVerificationError::WrongWriter {
            expected: head.writer().clone(),
        });
    }
    let chain = ChainId::parse(&statement.chain_id).map_err(|_| CheckpointVerificationError::InvalidChainId)?;
    if chain != head.chain() {
        return Err(CheckpointVerificationError::WrongChain {
            expected: head.chain(),
            observed: chain,
        });
    }
    if statement.head_sequence != head.sequence() {
        return Err(CheckpointVerificationError::HeadSequenceMismatch {
            expected: head.sequence(),
            observed: statement.head_sequence,
        });
    }
    if statement.head_hash.as_slice() != head.hash().as_bytes() {
        return Err(CheckpointVerificationError::HeadHashMismatch {
            sequence: head.sequence(),
        });
    }
    if statement.audit_stream_sequence != head.stream_sequence() {
        return Err(CheckpointVerificationError::StreamSequenceMismatch {
            expected: head.stream_sequence(),
            observed: statement.audit_stream_sequence,
        });
    }
    Ok(())
}

#[cfg(test)]
mod tests;
