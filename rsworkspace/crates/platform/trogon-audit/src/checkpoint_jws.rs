//! The detached JWS a checkpoint is signed as (RFC 7515 Appendix F).
//!
//! ```text
//! signing input = BASE64URL(header) "." BASE64URL(statement)
//! detached_jws  = BASE64URL(header) ".." BASE64URL(signature)
//! ```
//!
//! The protected header is written once by the writer and stored inside the JWS,
//! and the statement is stored verbatim beside it, so a verifier rebuilds the
//! signing input from stored bytes and nothing is canonicalized.

use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use ed25519_dalek::Signature;

use crate::constants::{CHECKPOINT_JWS_ALGORITHM, CHECKPOINT_JWS_TYPE};
use crate::key_id::KeyId;

/// The encoded protected header for a checkpoint signed under `key`. A key id is
/// base58btc after a fixed prefix, so it needs no JSON escaping.
pub(crate) fn protected_header(key: &KeyId) -> String {
    let json = format!(r#"{{"alg":"{CHECKPOINT_JWS_ALGORITHM}","kid":"{key}","typ":"{CHECKPOINT_JWS_TYPE}"}}"#);
    URL_SAFE_NO_PAD.encode(json)
}

pub(crate) fn signing_input(header: &str, statement: &[u8]) -> Vec<u8> {
    format!("{header}.{}", URL_SAFE_NO_PAD.encode(statement)).into_bytes()
}

pub(crate) fn detached(header: &str, signature: &Signature) -> String {
    format!("{header}..{}", URL_SAFE_NO_PAD.encode(signature.to_bytes()))
}

/// The header and signature segments of a detached JWS, or `None` when it has
/// other than three segments or carries a payload.
pub(crate) fn split(jws: &str) -> Option<(&str, &str)> {
    let mut segments = jws.split('.');
    match (segments.next(), segments.next(), segments.next(), segments.next()) {
        (Some(header), Some(""), Some(signature), None) => Some((header, signature)),
        _ => None,
    }
}

pub(crate) fn decode_segment(segment: &str) -> Option<Vec<u8>> {
    URL_SAFE_NO_PAD.decode(segment).ok()
}

#[cfg(test)]
mod tests;
