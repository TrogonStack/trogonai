//! Agent-side PoP signer: owns a P-256 signing key and the agent's
//! `aa-agent+jwt`, and produces the NATS header set that
//! `trogon_aauth_verify::NatsPopVerifier` accepts.

use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use p256::ecdsa::signature::Signer as _;
use p256::ecdsa::{Signature, SigningKey};
use p256::pkcs8::DecodePrivateKey;
use trogon_aauth_verify::jwk_thumbprint;
use trogon_aauth_verify::nats_pop::content_digest_sha256;
use trogon_identity_types::aauth::NatsSignatureEnvelope;
use trogon_identity_types::aauth::headers;

use crate::error::AgentSignerError;
use rand_core::{OsRng, RngCore};
use std::time::{SystemTime, UNIX_EPOCH};

/// Order of covered components in the `AAuth-Sig-Input` header. Must match
/// `NatsSignatureEnvelope::canonical_base` and what `NatsPopVerifier` expects.
const SIG_INPUT: &str =
    "(\"@subject\" \"@reply\" \"content-digest\" \"aauth-token\" \"aauth-sig-created\" \"aauth-sig-nonce\")";

/// The six NATS headers produced by a signed request, in the order the
/// verifier expects to find them (order doesn't matter on the wire, but a
/// named struct keeps call sites from mixing up which string is which).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PopHeaders(Vec<(String, String)>);

impl PopHeaders {
    /// Consume into the `(name, value)` pairs a NATS message header map expects.
    #[must_use]
    pub fn into_pairs(self) -> Vec<(String, String)> {
        self.0
    }

    /// Borrow the `(name, value)` pairs without consuming.
    #[must_use]
    pub fn as_pairs(&self) -> &[(String, String)] {
        &self.0
    }
}

/// Agent-side signer: a P-256 key plus the `aa-agent+jwt` issued for it by an
/// Agent Provider, and (optionally) an `aa-auth+jwt` issued for a specific
/// resource interaction.
pub struct AgentSigner {
    signing_key: SigningKey,
    agent_jwt: String,
    agent_jwk: serde_json::Value,
    jkt: String,
    auth_jwt: Option<String>,
}

impl AgentSigner {
    /// Build a signer from an in-memory P-256 signing key and the agent's
    /// `aa-agent+jwt`. The JWK embedded in the token's `cnf` claim must match
    /// the public key derived from `signing_key`, or verification on the
    /// resource side will fail even though signing here succeeds.
    pub fn new(signing_key: SigningKey, agent_jwt: impl Into<String>) -> Result<Self, AgentSignerError> {
        let agent_jwk = public_jwk(&signing_key)?;
        let jkt = jwk_thumbprint(&agent_jwk).map_err(AgentSignerError::Thumbprint)?;
        Ok(Self {
            signing_key,
            agent_jwt: agent_jwt.into(),
            agent_jwk,
            jkt,
            auth_jwt: None,
        })
    }

    /// Build a signer from a PKCS#8 PEM-encoded P-256 private key and the
    /// agent's `aa-agent+jwt`.
    pub fn from_pkcs8_pem(pem: &str, agent_jwt: impl Into<String>) -> Result<Self, AgentSignerError> {
        let signing_key = SigningKey::from_pkcs8_pem(pem).map_err(AgentSignerError::InvalidPkcs8)?;
        Self::new(signing_key, agent_jwt)
    }

    /// Attach an `aa-auth+jwt` so subsequent signed requests also carry
    /// `AAuth-Auth-Token`. Consumes and returns `self` for builder-style use.
    #[must_use]
    pub fn with_auth_token(mut self, auth_jwt: impl Into<String>) -> Self {
        self.auth_jwt = Some(auth_jwt.into());
        self
    }

    /// RFC 7638 thumbprint of the agent's public JWK. Must match the `jkt`
    /// resources and Person Servers bind challenges/auth tokens to.
    #[must_use]
    pub fn jkt(&self) -> &str {
        &self.jkt
    }

    /// The agent's public JWK (as embedded in the `cnf` claim of its
    /// `aa-agent+jwt`), exposed for callers that need to publish or
    /// cross-check it independently of the signer.
    #[must_use]
    pub fn public_jwk(&self) -> &serde_json::Value {
        &self.agent_jwk
    }

    /// Sign a NATS request, producing the header set `NatsPopVerifier`
    /// accepts: `AAuth-Token`, `AAuth-Sig-Input`, `AAuth-Sig`,
    /// `AAuth-Sig-Created`, `AAuth-Sig-Nonce`, `Content-Digest`, and
    /// (if [`AgentSigner::with_auth_token`] was used) `AAuth-Auth-Token`.
    ///
    /// `created` and `nonce` are caller-supplied so this function stays pure
    /// and deterministic; use [`AgentSigner::sign_nats_request_now`] for a
    /// convenience that fills them in.
    pub fn sign_nats_request(
        &self,
        subject: &str,
        reply: Option<&str>,
        payload: &[u8],
        created: i64,
        nonce: &str,
    ) -> PopHeaders {
        let content_digest = content_digest_sha256(payload);
        let envelope = NatsSignatureEnvelope {
            token: self.agent_jwt.clone(),
            sig_input: SIG_INPUT.to_string(),
            sig: String::new(),
            created,
            nonce: nonce.to_string(),
            content_digest: content_digest.clone(),
        };
        let canonical = envelope.canonical_base(subject, reply, &self.jkt);
        let signature: Signature = self.signing_key.sign(canonical.as_bytes());
        let sig_b64 = URL_SAFE_NO_PAD.encode(signature.to_bytes());

        let mut pairs = vec![
            (headers::NATS_TOKEN.to_string(), self.agent_jwt.clone()),
            (headers::NATS_SIG_INPUT.to_string(), SIG_INPUT.to_string()),
            (headers::NATS_SIG.to_string(), sig_b64),
            (headers::NATS_SIG_CREATED.to_string(), created.to_string()),
            (headers::NATS_SIG_NONCE.to_string(), nonce.to_string()),
            (headers::CONTENT_DIGEST.to_string(), content_digest),
        ];
        if let Some(auth_jwt) = &self.auth_jwt {
            pairs.push((headers::NATS_AUTH_TOKEN.to_string(), auth_jwt.clone()));
        }
        PopHeaders(pairs)
    }

    /// Convenience over [`AgentSigner::sign_nats_request`] that fills
    /// `created` from the system clock and `nonce` from a random source, for
    /// callers that don't need deterministic output.
    pub fn sign_nats_request_now(&self, subject: &str, reply: Option<&str>, payload: &[u8]) -> PopHeaders {
        let created = now_unix_secs();
        let nonce = random_nonce();
        self.sign_nats_request(subject, reply, payload, created, &nonce)
    }
}

fn now_unix_secs() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| i64::try_from(d.as_secs()).unwrap_or(i64::MAX))
        .unwrap_or(0)
}

/// A nonce unique enough to avoid replay-store collisions between honest
/// requests. Not a cryptographic secret: its only job is to make the
/// canonical signing base unique per request.
fn random_nonce() -> String {
    let mut bytes = [0u8; 16];
    OsRng.fill_bytes(&mut bytes);
    URL_SAFE_NO_PAD.encode(bytes)
}

fn public_jwk(signing_key: &SigningKey) -> Result<serde_json::Value, AgentSignerError> {
    let verifying = signing_key.verifying_key();
    let point = verifying.to_encoded_point(false);
    let x = point.x().ok_or(AgentSignerError::InvalidPublicKey)?;
    let y = point.y().ok_or(AgentSignerError::InvalidPublicKey)?;
    Ok(serde_json::json!({
        "kty": "EC",
        "crv": "P-256",
        "x": URL_SAFE_NO_PAD.encode(x),
        "y": URL_SAFE_NO_PAD.encode(y),
    }))
}
