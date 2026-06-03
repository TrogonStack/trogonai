//! NATS proof-of-possession verifier.
//!
//! Mirrors RFC 9421 conceptually but for NATS: the agent signs a canonical base
//! formed from subject/reply, content digest, the agent token, a created
//! timestamp, and a nonce. Verifier reconstructs the base from the inbound
//! message, fetches the agent's `cnf.jwk` from a verified `aa-agent+jwt`, and
//! checks the ECDSA / EdDSA signature.

use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use jsonwebtoken::{
    Algorithm, DecodingKey, Validation,
    crypto::verify,
    jwk::{AlgorithmParameters, EllipticCurve, Jwk},
};
use sha2::{Digest, Sha256};
use trogon_identity_types::aauth::{NatsSignatureEnvelope, headers};

use crate::replay::ReplayStore;
use crate::time_source::TimeSource;
use crate::token::{TokenError, TokenVerifier, VerifiedAgent};
use crate::jwks::JwksResolver;

/// Errors specific to NATS PoP verification.
#[derive(Debug, thiserror::Error)]
pub enum NatsPopError {
    #[error("missing required header: {0}")]
    MissingHeader(&'static str),
    #[error("invalid header {0}: {1}")]
    InvalidHeader(&'static str, String),
    #[error("agent token: {0}")]
    Agent(#[from] TokenError),
    #[error("signature did not verify")]
    BadSignature,
    #[error("content digest mismatch")]
    DigestMismatch,
    #[error("signature too old or skewed")]
    Skew,
    #[error("nonce replay detected")]
    Replay,
    #[error("backend: {0}")]
    Backend(String),
}

/// Subset of NATS message inputs needed for verification. Adapters convert from
/// `async_nats::Message` into this struct so this crate doesn't depend on async-nats.
#[derive(Clone, Debug)]
pub struct NatsRequest<'a> {
    pub subject: &'a str,
    pub reply: Option<&'a str>,
    pub payload: &'a [u8],
    pub headers: NatsHeaders<'a>,
}

/// Header view: pick the first value for a given case-insensitive name.
pub struct NatsHeaders<'a> {
    items: &'a [(String, String)],
}

impl<'a> NatsHeaders<'a> {
    #[must_use]
    pub fn new(items: &'a [(String, String)]) -> Self {
        Self { items }
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
            .map_err(|e: std::num::ParseIntError| NatsPopError::InvalidHeader(headers::NATS_SIG_CREATED, e.to_string()))?;
        let nonce = req
            .headers
            .get(headers::NATS_SIG_NONCE)
            .ok_or(NatsPopError::MissingHeader(headers::NATS_SIG_NONCE))?;

        let now = self.clock.now();
        if (now - created).abs() > self.max_skew_secs {
            return Err(NatsPopError::Skew);
        }

        let verified_agent = self.token_verifier.verify_agent(token).await?;

        // Content digest binding.
        let expected_digest = content_digest_sha256(req.payload);
        let supplied_digest = req
            .headers
            .get(headers::CONTENT_DIGEST)
            .unwrap_or(expected_digest.as_str());
        if supplied_digest != expected_digest {
            return Err(NatsPopError::DigestMismatch);
        }

        // Replay protection.
        let nonce_key = format!("nats-pop:{nonce}");
        let fresh = self
            .replay
            .check_and_insert(&nonce_key, u32::try_from(self.max_skew_secs * 2).unwrap_or(600))
            .await
            .map_err(|e| NatsPopError::Backend(e.to_string()))?;
        if !fresh {
            return Err(NatsPopError::Replay);
        }

        let envelope = NatsSignatureEnvelope {
            token: token.to_string(),
            sig_input: sig_input.to_string(),
            sig: sig.to_string(),
            created,
            nonce: nonce.to_string(),
            content_digest: supplied_digest.to_string(),
        };

        let canonical = envelope.canonical_base(req.subject, req.reply, &verified_agent.jkt);
        verify_signature_with_jwk(&verified_agent.claims.cnf.jwk, canonical.as_bytes(), sig)?;

        Ok(verified_agent)
    }
}

pub fn content_digest_sha256(payload: &[u8]) -> String {
    let digest = Sha256::digest(payload);
    format!("sha-256=:{}:", URL_SAFE_NO_PAD.encode(digest))
}

fn verify_signature_with_jwk(jwk_val: &serde_json::Value, base: &[u8], sig_b64: &str) -> Result<(), NatsPopError> {
    let jwk: Jwk = serde_json::from_value(jwk_val.clone()).map_err(|e| NatsPopError::InvalidHeader("cnf.jwk", e.to_string()))?;
    let alg = match &jwk.algorithm {
        AlgorithmParameters::EllipticCurve(ec) if ec.curve == EllipticCurve::P256 => Algorithm::ES256,
        AlgorithmParameters::EllipticCurve(ec) if ec.curve == EllipticCurve::P384 => Algorithm::ES384,
        AlgorithmParameters::OctetKeyPair(okp) if okp.curve == EllipticCurve::Ed25519 => Algorithm::EdDSA,
        _ => return Err(NatsPopError::InvalidHeader("cnf.jwk", "unsupported algorithm".into())),
    };
    let key = DecodingKey::from_jwk(&jwk).map_err(|e| NatsPopError::InvalidHeader("cnf.jwk", e.to_string()))?;
    let _ = Validation::new(alg);
    // `verify` operates on the raw signing input (no JWS wrapping). The signature
    // is base64url-no-pad encoded in our envelope.
    let ok = verify(sig_b64, base, &key, alg).map_err(|e| NatsPopError::InvalidHeader("sig", e.to_string()))?;
    if !ok {
        return Err(NatsPopError::BadSignature);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn content_digest_is_stable() {
        let a = content_digest_sha256(b"hello");
        let b = content_digest_sha256(b"hello");
        assert_eq!(a, b);
        assert!(a.starts_with("sha-256=:"));
    }
}
