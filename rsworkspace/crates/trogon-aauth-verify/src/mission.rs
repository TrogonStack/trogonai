//! Mission context verification per draft "Mission" (#missions), specifically
//! "Mission Context at Resources" and "AAuth-Mission Request Header".
//!
//! A mission-aware resource copies the `AAuth-Mission` request header's
//! `{approver, s256}` reference into the resource token it issues, and later
//! into any `mission` claim on the auth token presented for that mission. Per
//! "Request-Context Binding" rule 9, a resource holding an auth token with a
//! `mission` claim verifies that claim against the request's `AAuth-Mission`
//! header. This module implements that comparison plus the `s256` hash check
//! described in "Mission Approval": `s256` is the base64url SHA-256 digest of
//! the exact mission blob bytes the agent received from the PS.
//!
//! `AuthClaims` in `trogon-identity-types` does not yet carry a typed
//! `mission` field, so [`MissionClaim`] is parsed directly out of the raw JWT
//! claims JSON by [`extract_mission_claim`] rather than read off a typed
//! struct field.

use trogon_identity_types::aauth::MissionRef;

/// Errors verifying mission context.
#[derive(Debug, thiserror::Error)]
pub enum MissionError {
    /// The token's `mission` claim was present but did not deserialize into
    /// `{approver, s256}`.
    #[error("mission claim is malformed")]
    MalformedClaim(#[source] serde_json::Error),
    /// The `AAuth-Mission` request header's `approver` does not exactly match
    /// the token's `mission.approver` (#aauth-mission -- "compared by exact
    /// string match").
    #[error("mission approver mismatch: header={header_approver:?}, claim={claim_approver:?}")]
    ApproverMismatch {
        header_approver: String,
        claim_approver: String,
    },
    /// The `AAuth-Mission` request header's `s256` does not match the token's
    /// `mission.s256`.
    #[error("mission s256 mismatch: header={header_s256:?}, claim={claim_s256:?}")]
    S256HeaderMismatch { header_s256: String, claim_s256: String },
    /// The recomputed SHA-256 of the supplied mission blob bytes does not
    /// match the expected `s256` (header or claim) -- the blob presented does
    /// not correspond to the approved mission ("Mission Approval": "The agent
    /// verifies the hash by computing SHA-256 over the exact response body
    /// bytes").
    #[error("mission blob hash mismatch: expected s256={expected:?}, computed={computed:?}")]
    BlobHashMismatch { expected: String, computed: String },
}

/// Extract the `mission` claim (`{approver, s256}`) from raw JWT claims JSON,
/// if present. Returns `Ok(None)` when the claim is absent (missions are
/// OPTIONAL), and `Err` only when the claim is present but malformed.
pub fn extract_mission_claim(raw_claims: &serde_json::Value) -> Result<Option<MissionRef>, MissionError> {
    match raw_claims.get("mission") {
        None | Some(serde_json::Value::Null) => Ok(None),
        Some(value) => serde_json::from_value(value.clone())
            .map(Some)
            .map_err(MissionError::MalformedClaim),
    }
}

/// Verifies an `AAuth-Mission` request header against a token's `mission`
/// claim per "Request-Context Binding" rule 9: `approver` and `s256` must
/// match exactly between the header the agent sent and the claim the issuer
/// recorded.
pub fn verify_mission_header_matches_claim(header: &MissionRef, claim: &MissionRef) -> Result<(), MissionError> {
    if header.approver != claim.approver {
        return Err(MissionError::ApproverMismatch {
            header_approver: header.approver.clone(),
            claim_approver: claim.approver.clone(),
        });
    }
    if header.s256 != claim.s256 {
        return Err(MissionError::S256HeaderMismatch {
            header_s256: header.s256.clone(),
            claim_s256: claim.s256.clone(),
        });
    }
    Ok(())
}

/// Verifies that the exact bytes of a mission blob hash to the expected
/// `s256` value, per "Mission Approval": `s256` is the unpadded base64url
/// SHA-256 digest of the mission blob's exact response bytes -- callers MUST
/// pass those bytes unmodified (no re-serialization) for this check to be
/// meaningful.
pub fn verify_mission_blob_hash(expected_s256: &str, blob_bytes: &[u8]) -> Result<(), MissionError> {
    use base64::Engine;
    use sha2::{Digest, Sha256};

    let digest = Sha256::digest(blob_bytes);
    let computed = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(digest);
    if computed != expected_s256 {
        return Err(MissionError::BlobHashMismatch {
            expected: expected_s256.to_string(),
            computed,
        });
    }
    Ok(())
}

#[cfg(test)]
mod tests;
