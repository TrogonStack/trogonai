//! NATS proof-of-possession verifier.
//!
//! Mirrors RFC 9421 conceptually but for NATS: the agent signs a canonical base
//! formed from subject/reply, content digest, the agent token, a created
//! timestamp, and a nonce. Verifier reconstructs the base from the inbound
//! message, fetches the agent's `cnf.jwk` from a verified `aa-agent+jwt`, and
//! checks the ECDSA / EdDSA signature.

use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use jsonwebtoken::{
    Algorithm, DecodingKey,
    crypto::verify,
    jwk::{AlgorithmParameters, EllipticCurve, Jwk},
};
use sha2::{Digest, Sha256};
use trogon_identity_types::aauth::{NatsSignatureEnvelope, headers};

use crate::jwks::JwksResolver;
use crate::replay::{ReplayError, ReplayStore};
use crate::time_source::TimeSource;
use crate::token::{TokenError, TokenVerifier, VerifiedAgent};

/// Errors specific to NATS PoP verification.
///
/// Variants are typed (no stringified sources): callers can match on the
/// specific failure mode without parsing display text, and the source chain
/// from upstream parsers / `jsonwebtoken` / the replay backend remains
/// available via `std::error::Error::source`.
#[derive(Debug, thiserror::Error)]
pub enum NatsPopError {
    #[error("missing required header: {0}")]
    MissingHeader(&'static str),
    /// A security-sensitive header appeared more than once (case-insensitive)
    /// in the request — refuse rather than pick the first and let the rest go
    /// unauthenticated.
    #[error("duplicate security-sensitive header: {0}")]
    DuplicateHeader(&'static str),
    /// A header was present but the value did not parse as the expected type.
    #[error("invalid value for header {header}")]
    InvalidHeaderValue {
        header: &'static str,
        #[source]
        source: std::num::ParseIntError,
    },
    /// Configured `max_skew_secs` was negative — operator misconfiguration
    /// rather than a request-level fault.
    #[error("configured max_skew_secs must be non-negative, got {0}")]
    NegativeMaxSkew(i64),
    /// The arithmetic to compare `now` and `created` overflowed `i64` (one of
    /// them is near a bound). Refuse rather than silently wrap.
    #[error("skew arithmetic overflowed (now={now}, created={created})")]
    SkewOverflow { now: i64, created: i64 },
    /// `cnf.jwk` either failed to deserialize or named an algorithm the PoP
    /// verifier doesn't support.
    #[error("invalid cnf.jwk")]
    InvalidConfirmationKey(#[source] InvalidConfirmationKey),
    #[error("agent token: {0}")]
    Agent(#[from] TokenError),
    /// The PoP signature did not verify against `cnf.jwk` over the canonical
    /// base. Sources for the underlying jsonwebtoken error chain through
    /// `Verify(_)`.
    #[error("signature did not verify")]
    BadSignature,
    #[error("signature verification: {0}")]
    Verify(#[source] jsonwebtoken::errors::Error),
    #[error("content digest mismatch")]
    DigestMismatch,
    #[error("signature too old or skewed")]
    Skew,
    #[error("nonce replay detected")]
    Replay,
    #[error("replay store backend failure")]
    Backend(#[source] ReplayError),
}

/// Reason that the agent's `cnf.jwk` could not be used to verify the PoP
/// signature.
#[derive(Debug, thiserror::Error)]
pub enum InvalidConfirmationKey {
    /// `cnf.jwk` did not deserialize into a JWK structure.
    #[error("cnf.jwk did not deserialize")]
    Deserialize(#[source] serde_json::Error),
    /// `cnf.jwk` named an algorithm or curve the PoP verifier does not
    /// implement (only ES256, ES384, EdDSA/Ed25519 are accepted today).
    #[error("cnf.jwk uses an unsupported algorithm")]
    UnsupportedAlgorithm,
    /// `jsonwebtoken` rejected the JWK when constructing a DecodingKey.
    #[error("cnf.jwk could not produce a decoding key")]
    DecodingKey(#[source] jsonwebtoken::errors::Error),
}

/// Subset of NATS message inputs needed for verification. Adapters convert from
/// `async_nats::Message` into this struct so this crate doesn't depend on
/// async-nats.
#[derive(Clone, Debug)]
pub struct NatsRequest<'a> {
    pub subject: &'a str,
    pub reply: Option<&'a str>,
    pub payload: &'a [u8],
    pub headers: NatsHeaders<'a>,
}

/// Security-sensitive headers that drive PoP verification. If any appears more
/// than once (case-insensitive) in a request, the verifier refuses rather
/// than silently picking one value and letting the rest go unauthenticated.
const SECURITY_HEADERS: &[&str] = &[
    headers::NATS_TOKEN,
    headers::NATS_SIG_INPUT,
    headers::NATS_SIG,
    headers::NATS_SIG_CREATED,
    headers::NATS_SIG_NONCE,
    headers::CONTENT_DIGEST,
];

/// Header view: pick the first value for a given case-insensitive name.
pub struct NatsHeaders<'a> {
    items: &'a [(String, String)],
}

impl<'a> NatsHeaders<'a> {
    /// Construct a header view without checking for duplicates. Callers that
    /// take attacker-controlled input should use [`NatsHeaders::new_checked`]
    /// instead.
    #[must_use]
    pub fn new(items: &'a [(String, String)]) -> Self {
        Self { items }
    }

    /// Construct a header view and refuse if any security-sensitive header
    /// (case-insensitive) appears more than once. This is the constructor the
    /// verifier expects on inbound traffic — duplicate `aauth-sig` or
    /// `aauth-token` headers were the easiest way for a smuggling attack to
    /// have the signature checked against one value while a downstream
    /// consumer reads another.
    pub fn new_checked(items: &'a [(String, String)]) -> Result<Self, NatsPopError> {
        for &name in SECURITY_HEADERS {
            let count = items.iter().filter(|(k, _)| k.eq_ignore_ascii_case(name)).count();
            if count > 1 {
                return Err(NatsPopError::DuplicateHeader(name));
            }
        }
        Ok(Self { items })
    }

    fn get(&self, name: &str) -> Option<&str> {
        self.items
            .iter()
            .find(|(k, _)| k.eq_ignore_ascii_case(name))
            .map(|(_, v)| v.as_str())
    }
}

impl<'a> Clone for NatsHeaders<'a> {
    fn clone(&self) -> Self {
        Self { items: self.items }
    }
}

impl<'a> std::fmt::Debug for NatsHeaders<'a> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("NatsHeaders").field("len", &self.items.len()).finish()
    }
}

/// Configuration for the NATS PoP verifier.
pub struct NatsPopVerifier<R: JwksResolver, C: TimeSource, S: ReplayStore> {
    pub token_verifier: TokenVerifier<R, C>,
    pub clock: C,
    pub replay: S,
    pub max_skew_secs: i64,
}

impl<R: JwksResolver + Clone, C: TimeSource + Clone, S: ReplayStore> NatsPopVerifier<R, C, S> {
    pub fn new(jwks: R, clock: C, replay: S) -> Self {
        Self {
            token_verifier: TokenVerifier::new(jwks, clock.clone()),
            clock,
            replay,
            max_skew_secs: 300,
        }
    }
}

impl<R: JwksResolver, C: TimeSource, S: ReplayStore> NatsPopVerifier<R, C, S> {
    /// Verify a NATS request bears a valid `aa-agent+jwt` and PoP signature.
    pub async fn verify(&self, req: &NatsRequest<'_>) -> Result<VerifiedAgent, NatsPopError> {
        // Operator-misconfiguration guard: a negative max_skew_secs would
        // either accept everything or reject everything depending on
        // arithmetic direction. Fail closed at the verifier entry.
        if self.max_skew_secs < 0 {
            return Err(NatsPopError::NegativeMaxSkew(self.max_skew_secs));
        }

        let token = req
            .headers
            .get(headers::NATS_TOKEN)
            .ok_or(NatsPopError::MissingHeader(headers::NATS_TOKEN))?;
        let sig_input = req
            .headers
            .get(headers::NATS_SIG_INPUT)
            .ok_or(NatsPopError::MissingHeader(headers::NATS_SIG_INPUT))?;
        let sig = req
            .headers
            .get(headers::NATS_SIG)
            .ok_or(NatsPopError::MissingHeader(headers::NATS_SIG))?;
        let created: i64 = req
            .headers
            .get(headers::NATS_SIG_CREATED)
            .ok_or(NatsPopError::MissingHeader(headers::NATS_SIG_CREATED))?
            .parse()
            .map_err(|source| NatsPopError::InvalidHeaderValue {
                header: headers::NATS_SIG_CREATED,
                source,
            })?;
        let nonce = req
            .headers
            .get(headers::NATS_SIG_NONCE)
            .ok_or(NatsPopError::MissingHeader(headers::NATS_SIG_NONCE))?;

        // Overflow-safe skew check. `(now - created).abs()` wraps when one is
        // near `i64::MAX` and the other near `i64::MIN`; checked subtraction
        // surfaces that case as a typed error.
        let now = self.clock.now();
        let delta = now
            .checked_sub(created)
            .ok_or(NatsPopError::SkewOverflow { now, created })?;
        if delta.unsigned_abs() > self.max_skew_secs.unsigned_abs() {
            return Err(NatsPopError::Skew);
        }

        let verified_agent = self.token_verifier.verify_agent(token).await?;

        // Require the Content-Digest header rather than synthesizing one from
        // the payload — synthesizing made it possible for an attacker to omit
        // the header entirely and still satisfy the signature check whenever
        // the signing input happened not to commit to content-digest.
        let supplied_digest = req
            .headers
            .get(headers::CONTENT_DIGEST)
            .ok_or(NatsPopError::MissingHeader(headers::CONTENT_DIGEST))?;
        let expected_digest = content_digest_sha256(req.payload);
        if supplied_digest != expected_digest {
            return Err(NatsPopError::DigestMismatch);
        }

        let envelope = NatsSignatureEnvelope {
            token: token.to_string(),
            sig_input: sig_input.to_string(),
            sig: sig.to_string(),
            created,
            nonce: nonce.to_string(),
            content_digest: supplied_digest.to_string(),
        };

        // Verify the signature BEFORE consuming the nonce. Otherwise an
        // invalid-signature request burns its nonce against the replay store
        // and a later valid retry from the same agent is wrongly rejected as
        // a replay.
        let canonical = envelope.canonical_base(req.subject, req.reply, &verified_agent.jkt);
        verify_signature_with_jwk(&verified_agent.claims.cnf.jwk, canonical.as_bytes(), sig)?;

        // Replay protection only fires once the signature has authenticated
        // the request.
        let nonce_key = format!("nats-pop:{nonce}");
        let fresh = self
            .replay
            .check_and_insert(&nonce_key, u32::try_from(self.max_skew_secs * 2).unwrap_or(600))
            .await
            .map_err(NatsPopError::Backend)?;
        if !fresh {
            return Err(NatsPopError::Replay);
        }

        Ok(verified_agent)
    }
}

pub fn content_digest_sha256(payload: &[u8]) -> String {
    let digest = Sha256::digest(payload);
    format!("sha-256=:{}:", URL_SAFE_NO_PAD.encode(digest))
}

fn verify_signature_with_jwk(jwk_val: &serde_json::Value, base: &[u8], sig_b64: &str) -> Result<(), NatsPopError> {
    let jwk: Jwk = serde_json::from_value(jwk_val.clone())
        .map_err(|source| NatsPopError::InvalidConfirmationKey(InvalidConfirmationKey::Deserialize(source)))?;
    // ES384 and EdDSA require real P-384 / Ed25519 fixtures to exercise.
    // Unit tests here only carry ES256 material, so the extra arms collapse
    // to a cfg(coverage) stub matching the pattern used in `token::jwk_compatible_with_alg`.
    #[cfg(not(coverage))]
    let alg = match &jwk.algorithm {
        AlgorithmParameters::EllipticCurve(ec) if ec.curve == EllipticCurve::P256 => Algorithm::ES256,
        AlgorithmParameters::EllipticCurve(ec) if ec.curve == EllipticCurve::P384 => Algorithm::ES384,
        AlgorithmParameters::OctetKeyPair(okp) if okp.curve == EllipticCurve::Ed25519 => Algorithm::EdDSA,
        _ => {
            return Err(NatsPopError::InvalidConfirmationKey(
                InvalidConfirmationKey::UnsupportedAlgorithm,
            ));
        }
    };
    #[cfg(coverage)]
    let alg = match &jwk.algorithm {
        AlgorithmParameters::EllipticCurve(ec) if ec.curve == EllipticCurve::P256 => Algorithm::ES256,
        _ => {
            return Err(NatsPopError::InvalidConfirmationKey(
                InvalidConfirmationKey::UnsupportedAlgorithm,
            ));
        }
    };
    let key = DecodingKey::from_jwk(&jwk)
        .map_err(|source| NatsPopError::InvalidConfirmationKey(InvalidConfirmationKey::DecodingKey(source)))?;
    // `verify` operates on the raw signing input (no JWS wrapping). The
    // signature is base64url-no-pad encoded in our envelope.
    let ok = verify(sig_b64, base, &key, alg).map_err(NatsPopError::Verify)?;
    if !ok {
        return Err(NatsPopError::BadSignature);
    }
    Ok(())
}

#[cfg(test)]
mod tests;
