//! Server-issued nonces (ADR#0051 Decision 3, ADR#0082 Decision 4).
//!
//! A resource can answer a request with a fresh nonce challenge, which the
//! client binds into its retry by sending the nonce back in
//! [`SERVER_NONCE_HEADER`](crate::constants::SERVER_NONCE_HEADER) and covering
//! that header with its signature. A bound nonce proves the signature was made
//! after the resource issued it, which a `created` the client chose cannot.
//!
//! A nonce is a compact JWS signed with the resource's challenge key. It names
//! the resource that issued it, the thumbprint of the key it was issued to and
//! its validity window, so a nonce issued to one key or by one resource does
//! not verify for another. Every issued nonce is also recorded in the shared
//! replay store and verifies only while that record lives, so any instance
//! sharing the store and the challenge key honors a nonce another instance
//! issued.

use std::fmt;

use jsonwebtoken::{Algorithm, DecodingKey, EncodingKey, Header, Validation, decode, decode_header, encode};
use p256::ecdsa::SigningKey;
use p256::pkcs8::DecodePrivateKey;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::constants::{DEFAULT_SERVER_NONCE_TTL_SECS, MAX_SERVER_NONCE_TTL_SECS, TYP_SERVER_NONCE};
use crate::freshness::ClockTolerance;

/// Key id of the challenge key, carried in the header of every nonce it
/// signs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChallengeKid(String);

impl ChallengeKid {
    /// # Errors
    ///
    /// [`EmptyChallengeKidError`] when `raw` is empty once trimmed.
    pub fn new(raw: impl AsRef<str>) -> Result<Self, EmptyChallengeKidError> {
        let trimmed = raw.as_ref().trim();
        if trimmed.is_empty() {
            return Err(EmptyChallengeKidError);
        }
        Ok(Self(trimmed.to_string()))
    }

    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

#[derive(Debug, thiserror::Error)]
#[error("challenge key id is empty")]
pub struct EmptyChallengeKidError;

/// The P-256 key a resource signs its nonces with and verifies them against.
pub struct ChallengeKey {
    encoding: EncodingKey,
    decoding: DecodingKey,
    kid: ChallengeKid,
}

impl ChallengeKey {
    /// Loads a PKCS#8 PEM P-256 private key, the form a resource's challenge
    /// key is provisioned in.
    ///
    /// # Errors
    ///
    /// [`ChallengeKeyError::Parse`] when `pem` is not a PKCS#8 P-256 private
    /// key, and [`ChallengeKeyError::Signing`] when it cannot be loaded for
    /// signing.
    pub fn from_pkcs8_pem(pem: &str, kid: ChallengeKid) -> Result<Self, ChallengeKeyError> {
        let signing = SigningKey::from_pkcs8_pem(pem).map_err(ChallengeKeyError::Parse)?;
        let public_point = signing.verifying_key().to_encoded_point(false);
        let encoding = EncodingKey::from_ec_pem(pem.as_bytes()).map_err(ChallengeKeyError::Signing)?;
        Ok(Self {
            encoding,
            decoding: DecodingKey::from_ec_der(public_point.as_bytes()),
            kid,
        })
    }

    #[must_use]
    pub fn kid(&self) -> &ChallengeKid {
        &self.kid
    }
}

impl fmt::Debug for ChallengeKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ChallengeKey")
            .field("kid", &self.kid)
            .finish_non_exhaustive()
    }
}

#[derive(Debug, thiserror::Error)]
pub enum ChallengeKeyError {
    #[error("challenge key is not a PKCS#8 PEM P-256 private key")]
    Parse(#[source] p256::pkcs8::Error),
    #[error("challenge key could not be loaded for signing")]
    Signing(#[source] jsonwebtoken::errors::Error),
}

/// How long a server-issued nonce stays usable after it is issued.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct NonceTtl(u32);

impl NonceTtl {
    /// # Errors
    ///
    /// [`NonceTtlError`] when `secs` is zero or exceeds
    /// [`MAX_SERVER_NONCE_TTL_SECS`].
    pub fn new(secs: u32) -> Result<Self, NonceTtlError> {
        if secs == 0 || secs > MAX_SERVER_NONCE_TTL_SECS {
            return Err(NonceTtlError { secs });
        }
        Ok(Self(secs))
    }

    #[must_use]
    pub fn secs(self) -> u32 {
        self.0
    }
}

impl Default for NonceTtl {
    fn default() -> Self {
        Self(DEFAULT_SERVER_NONCE_TTL_SECS)
    }
}

#[derive(Debug, thiserror::Error)]
#[error("nonce lifetime must be between 1 and {max}s, got {secs}s", max = MAX_SERVER_NONCE_TTL_SECS)]
pub struct NonceTtlError {
    secs: u32,
}

/// Whether a request has to bind a server-issued nonce.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NonceRequirement {
    /// A request that binds no nonce is answered with a fresh nonce challenge.
    Required,
    /// A request may bind a nonce, which then still has to verify.
    Optional,
}

/// A nonce as it travels on the wire.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ServerNonce(String);

impl ServerNonce {
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for ServerNonce {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

/// A fresh nonce a resource refuses a request with, for the client to bind
/// into its retry, and why the nonce the request bound was refused when it
/// bound one.
#[derive(Debug)]
pub struct NonceChallenge {
    nonce: ServerNonce,
    refused: Option<NonceError>,
}

impl NonceChallenge {
    pub(crate) fn new(nonce: ServerNonce, refused: Option<NonceError>) -> Self {
        Self { nonce, refused }
    }

    #[must_use]
    pub fn nonce(&self) -> &ServerNonce {
        &self.nonce
    }

    /// Why the nonce the request bound did not verify, or `None` when it
    /// bound none.
    #[must_use]
    pub fn refused(&self) -> Option<&NonceError> {
        self.refused.as_ref()
    }
}

/// The replay-store key recording that a nonce was issued.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct NonceRecord(String);

impl NonceRecord {
    fn for_id(id: &str) -> Self {
        Self(format!("nonce.{id}"))
    }

    pub(crate) fn key(&self) -> &str {
        &self.0
    }
}

pub(crate) struct IssuedNonce {
    pub(crate) nonce: ServerNonce,
    pub(crate) record: NonceRecord,
}

#[derive(Debug, Serialize, Deserialize)]
struct NonceClaims {
    iss: String,
    jti: String,
    jkt: String,
    iat: i64,
    exp: i64,
}

/// A resource's nonce policy: the key its nonces are signed with, how long
/// they stay usable, and whether a request has to bind one.
#[derive(Debug)]
pub struct ServerNonces {
    key: ChallengeKey,
    ttl: NonceTtl,
    requirement: NonceRequirement,
}

impl ServerNonces {
    #[must_use]
    pub fn new(key: ChallengeKey, ttl: NonceTtl, requirement: NonceRequirement) -> Self {
        Self { key, ttl, requirement }
    }

    #[must_use]
    pub fn ttl(&self) -> NonceTtl {
        self.ttl
    }

    #[must_use]
    pub fn requirement(&self) -> NonceRequirement {
        self.requirement
    }

    /// How long the record of an issued nonce has to live: a verifier whose
    /// clock trails the issuer's by the tolerated skew still accepts the
    /// nonce until its expiry plus that skew.
    #[must_use]
    pub fn record_ttl_secs(&self, skew: ClockTolerance) -> u32 {
        self.ttl.secs() + 2 * skew.secs()
    }

    pub(crate) fn mint(&self, issuer: &str, jkt: &str, now: i64) -> Result<IssuedNonce, NonceIssueError> {
        let exp = now
            .checked_add(i64::from(self.ttl.secs()))
            .ok_or(NonceIssueError::Overflow)?;
        let id = Uuid::new_v4().simple().to_string();
        let mut header = Header::new(Algorithm::ES256);
        header.typ = Some(TYP_SERVER_NONCE.to_string());
        header.kid = Some(self.key.kid.as_str().to_string());
        let claims = NonceClaims {
            iss: issuer.to_string(),
            jti: id.clone(),
            jkt: jkt.to_string(),
            iat: now,
            exp,
        };
        let token = encode(&header, &claims, &self.key.encoding).map_err(NonceIssueError::Sign)?;
        Ok(IssuedNonce {
            nonce: ServerNonce(token),
            record: NonceRecord::for_id(&id),
        })
    }

    /// Checks that `raw` is a nonce this resource issued to the key with
    /// thumbprint `jkt` and that it is still usable at `now`, returning the
    /// record that has to be live for it to verify.
    pub(crate) fn check(
        &self,
        raw: &str,
        issuer: &str,
        jkt: &str,
        now: i64,
        skew: ClockTolerance,
    ) -> Result<NonceRecord, NonceError> {
        let header = decode_header(raw).map_err(NonceError::Malformed)?;
        if header.typ.as_deref() != Some(TYP_SERVER_NONCE) {
            return Err(NonceError::NotANonce);
        }
        if header.kid.as_deref() != Some(self.key.kid.as_str()) {
            return Err(NonceError::UnknownKey);
        }

        let mut validation = Validation::new(Algorithm::ES256);
        validation.validate_exp = false;
        validation.validate_nbf = false;
        validation.validate_aud = false;
        validation.required_spec_claims.clear();
        let claims = decode::<NonceClaims>(raw, &self.key.decoding, &validation)
            .map_err(NonceError::Rejected)?
            .claims;

        if claims.iss != issuer {
            return Err(NonceError::WrongIssuer);
        }
        if claims.jkt != jkt {
            return Err(NonceError::KeyMismatch);
        }
        let lifetime = claims.exp.checked_sub(claims.iat).ok_or(NonceError::InvalidWindow)?;
        if lifetime <= 0 || lifetime > i64::from(MAX_SERVER_NONCE_TTL_SECS) {
            return Err(NonceError::InvalidWindow);
        }
        let skew = i64::from(skew.secs());
        if claims.iat > now.saturating_add(skew) {
            return Err(NonceError::NotYetValid);
        }
        if now > claims.exp.saturating_add(skew) {
            return Err(NonceError::Expired);
        }
        Ok(NonceRecord::for_id(&claims.jti))
    }
}

/// Why a nonce a request binds does not verify.
#[derive(Debug, thiserror::Error)]
pub enum NonceError {
    #[error("nonce is not a compact JWS")]
    Malformed(#[source] jsonwebtoken::errors::Error),
    #[error("nonce is not typed as a server nonce")]
    NotANonce,
    #[error("nonce names a challenge key this resource does not hold")]
    UnknownKey,
    #[error("nonce did not verify against the challenge key")]
    Rejected(#[source] jsonwebtoken::errors::Error),
    #[error("nonce was issued by another resource")]
    WrongIssuer,
    #[error("nonce was issued to another signing key")]
    KeyMismatch,
    #[error("nonce window does not end after it starts, or outlives the longest nonce lifetime")]
    InvalidWindow,
    #[error("nonce is issued further ahead than the tolerated clock skew")]
    NotYetValid,
    #[error("nonce expired beyond the tolerated clock skew")]
    Expired,
    #[error("nonce has no live record of being issued")]
    NotIssued,
}

/// Why a fresh nonce could not be issued.
#[derive(Debug, thiserror::Error)]
pub enum NonceIssueError {
    #[error("nonce expiry overflows the clock")]
    Overflow,
    #[error("nonce could not be signed")]
    Sign(#[source] jsonwebtoken::errors::Error),
    #[error("a nonce with the same id is already recorded")]
    Collision,
}

#[cfg(test)]
mod tests;
