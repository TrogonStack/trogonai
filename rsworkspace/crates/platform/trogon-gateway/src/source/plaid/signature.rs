use std::time::{Duration, SystemTime, UNIX_EPOCH};

use jsonwebtoken::{Algorithm, Validation, decode, decode_header};
use serde::Deserialize;

use super::plaid_key_id::PlaidKeyIdError;
use super::verification_key::PlaidVerificationKey;
use super::{PlaidBodyDigest, PlaidDeliveryId, PlaidKeyId};

#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum SignatureError {
    #[error("malformed token header")]
    MalformedHeader(#[source] jsonwebtoken::errors::Error),
    #[error("unsupported algorithm {0:?}, expected ES256")]
    UnsupportedAlgorithm(Algorithm),
    #[error("token header has no key id")]
    MissingKeyId,
    #[error("token header key id is invalid")]
    InvalidKeyId(#[source] PlaidKeyIdError),
    #[error("signature or claims invalid")]
    InvalidToken(#[source] jsonwebtoken::errors::Error),
    #[error("issued-at is {0:?} away from now")]
    IssuedAtOutsideTolerance(Duration),
    #[error("body hash does not match request_body_sha256")]
    BodyHashMismatch,
}

#[derive(Deserialize)]
struct Claims {
    iat: u64,
    request_body_sha256: String,
}

/// Reads the key id from the unverified token header, accepting only ES256.
pub fn key_id(token: &str) -> Result<PlaidKeyId, SignatureError> {
    let header = decode_header(token).map_err(SignatureError::MalformedHeader)?;
    if header.alg != Algorithm::ES256 {
        return Err(SignatureError::UnsupportedAlgorithm(header.alg));
    }
    let kid = header.kid.ok_or(SignatureError::MissingKeyId)?;
    PlaidKeyId::new(kid).map_err(SignatureError::InvalidKeyId)
}

/// Verifies a Plaid `Plaid-Verification` JWT against `key` and the raw request body.
///
/// The signature is checked first, then `iat` against `tolerance`, then the
/// `request_body_sha256` claim against the SHA-256 of `body` in constant time.
/// Returns the delivery id, the dedup identity of this signed request.
pub fn verify(
    token: &str,
    key: &PlaidVerificationKey,
    body: &[u8],
    now: SystemTime,
    tolerance: Duration,
) -> Result<PlaidDeliveryId, SignatureError> {
    let mut validation = Validation::new(Algorithm::ES256);
    validation.validate_exp = false;
    validation.validate_aud = false;
    validation.required_spec_claims.clear();

    let claims = decode::<Claims>(token, key.decoding_key(), &validation)
        .map_err(SignatureError::InvalidToken)?
        .claims;

    let issued_at = UNIX_EPOCH + Duration::from_secs(claims.iat);
    let drift = match now.duration_since(issued_at) {
        Ok(age) => age,
        Err(early) => early.duration(),
    };
    if drift > tolerance {
        return Err(SignatureError::IssuedAtOutsideTolerance(drift));
    }

    if !PlaidBodyDigest::of(body).matches_claim(&claims.request_body_sha256) {
        return Err(SignatureError::BodyHashMismatch);
    }
    Ok(PlaidDeliveryId::of(token))
}

#[cfg(test)]
mod tests;
