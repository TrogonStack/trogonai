//! The stored form of a chained record (ADR#0086 Decision 6).
//!
//! A record is hashed over its exact encoded bytes `B`, with `previous_hash` set
//! and `record_hash` absent, and stored as `B || T`. The trailer `T` is a second
//! `AuditRecord.integrity` holding only `record_hash`, which every protobuf
//! decoder merges into the record's integrity, so a reader sees the hash without
//! knowing about the trailer. A verifier strips `T` and hashes `B` as stored, so
//! no canonical encoding is needed and a record is never re-serialized.

use bytes::{BufMut, Bytes, BytesMut};

use crate::constants::{SEAL_TRAILER_BYTES, SEAL_TRAILER_PREFIX};
use crate::record_hash::RecordHash;
use crate::seal_error::SealError;

/// Hashes `body` and appends the trailer, returning the hash and the bytes to
/// store.
pub fn seal(body: Bytes) -> (RecordHash, Bytes) {
    let hash = RecordHash::of(&body);
    let mut payload = BytesMut::with_capacity(body.len() + SEAL_TRAILER_BYTES);
    payload.put(body);
    payload.put_slice(&SEAL_TRAILER_PREFIX);
    payload.put_slice(hash.as_bytes());
    (hash, payload.freeze())
}

/// Splits stored bytes into the hashed body and its record hash, and checks the
/// hash against the body.
pub fn open(payload: &[u8]) -> Result<(&[u8], RecordHash), SealError> {
    let body_len = payload
        .len()
        .checked_sub(SEAL_TRAILER_BYTES)
        .ok_or(SealError::TooShort(payload.len()))?;
    let (body, trailer) = payload.split_at(body_len);
    let (prefix, stored) = trailer.split_at(SEAL_TRAILER_PREFIX.len());
    if prefix != SEAL_TRAILER_PREFIX {
        return Err(SealError::MissingTrailer);
    }
    let hash = RecordHash::of(body);
    if hash.as_bytes() != stored {
        return Err(SealError::HashMismatch);
    }
    Ok((body, hash))
}

#[cfg(test)]
mod tests;
