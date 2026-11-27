//! Mints [`OidcToken`]s for a `Forward` domain rule's matched requests (see
//! [`trogon_sandbox_contract::RequestForward`]).
//!
//! Signing key management mirrors [`crate::tls_ca::SandboxCa::generate`]:
//! by default an ephemeral P-256 key pair is generated fresh at process
//! startup and used for the lifetime of the process. This is deliberately
//! the same trust model `SandboxCa` already uses for the TLS-intercept leaf
//! certs it mints, so operators do not have to reason about two different
//! key-provisioning stories in one proxy. Unlike `SandboxCa`, a receiving
//! proxy needs to independently verify a token's signature, so
//! `OidcSigner::generate` alone is only useful in tests or single-process
//! setups where the verifier already has the public key out of band; real
//! deployments that need a receiving proxy to verify tokens minted by a
//! *different* process should load a persistent key via
//! [`OidcSigner::from_ec_pkcs8_pem`] (wired up in `trogon-sandbox-egress-proxyd` via
//! the optional `EGRESS_PROXY_OIDC_SIGNING_KEY_PEM_FILE` env var) so the key
//! (and therefore its public counterpart) is stable across restarts.
//!
//! A JWKS HTTP endpoint for the receiving proxy to fetch the verification key
//! from is deliberately out of scope: it would require this crate to run an
//! embedded HTTP server and own a key-rotation/publishing story, neither of
//! which this task's scope calls for. Whatever verifies `OidcToken`s today
//! must be handed the signing key's public counterpart out of band (e.g. the
//! same way `SandboxCa::root_cert_pem` is written to `EGRESS_PROXY_CA_OUT_FILE`
//! for the sandboxed workload to trust).

use jsonwebtoken::{Algorithm, DecodingKey, EncodingKey, Header, Validation, encode};
use rcgen::KeyPair;
use trogon_sandbox_contract::{LeaseId, OidcClaims, OidcToken, SandboxId, TenantId};

#[derive(Debug, thiserror::Error)]
pub enum OidcSignerError {
    #[error("failed to generate OIDC signing key pair: {0}")]
    KeyGeneration(rcgen::Error),
    #[error("failed to parse OIDC signing key PEM: {0}")]
    InvalidSigningKeyPem(rcgen::Error),
    #[error("failed to load OIDC signing key PEM into the JWT encoder: {0}")]
    InvalidSigningKeyPemForEncoding(jsonwebtoken::errors::Error),
    #[error("failed to sign OIDC token: {0}")]
    Signing(jsonwebtoken::errors::Error),
}

/// Everything [`OidcSigner`] needs to mint a token for one matched forwarded
/// request, distinct from [`OidcClaims`] itself: `subject`/`tenant_id`/
/// `lease_id`/`issuer` are per-process runtime identity (see
/// [`trogon_sandbox_contract::RequestForward`]'s doc comment on why these do not
/// live on the wire rule), while `audience`/`ttl_seconds` come from the
/// specific `RequestForward` rule that matched.
pub struct MintRequest<'a> {
    pub issuer: &'a str,
    pub subject: &'a SandboxId,
    pub tenant_id: &'a TenantId,
    pub lease_id: &'a LeaseId,
    pub audience: &'a str,
    pub ttl_seconds: u32,
}

/// Signs [`OidcClaims`] into an [`OidcToken`] using ES256, and (mostly for
/// tests) can verify one it minted itself. Holds one key pair for the
/// lifetime of the proxy process; every mint uses the same key, so a matched
/// request's token is cheap to produce (no per-request key generation).
pub struct OidcSigner {
    encoding_key: EncodingKey,
    /// Kept so [`OidcSigner::decoding_key`] can hand callers (tests, or a
    /// receiving proxy embedded in the same process) a verifier for tokens
    /// this signer mints, without re-deriving the public key from the
    /// private one on every call.
    verifying_public_key_sec1: Vec<u8>,
}

impl OidcSigner {
    /// Generates a fresh ephemeral P-256 key pair, valid only for this
    /// process's lifetime. See the module doc comment for why this is the
    /// default and when [`OidcSigner::from_ec_pkcs8_pem`] should be used
    /// instead.
    pub fn generate() -> Result<Self, OidcSignerError> {
        let key_pair = KeyPair::generate().map_err(OidcSignerError::KeyGeneration)?;
        let verifying_public_key_sec1 = key_pair.public_key_raw().to_vec();
        let encoding_key = EncodingKey::from_ec_der(&key_pair.serialize_der());
        Ok(Self {
            encoding_key,
            verifying_public_key_sec1,
        })
    }

    /// Loads a persistent EC key pair from a PKCS#8 PEM string (the format
    /// `rcgen::KeyPair::serialize_pem` produces, and what
    /// `openssl ecparam -genkey -noout -name prime256v1 | openssl pkcs8 -topk8 -nocrypt`
    /// produces), so the signing key (and its public counterpart, which a
    /// receiving proxy must trust out of band) survives process restarts.
    pub fn from_ec_pkcs8_pem(pem: &str) -> Result<Self, OidcSignerError> {
        let key_pair = KeyPair::from_pem(pem).map_err(OidcSignerError::InvalidSigningKeyPem)?;
        let verifying_public_key_sec1 = key_pair.public_key_raw().to_vec();
        let encoding_key =
            EncodingKey::from_ec_pem(pem.as_bytes()).map_err(OidcSignerError::InvalidSigningKeyPemForEncoding)?;
        Ok(Self {
            encoding_key,
            verifying_public_key_sec1,
        })
    }

    /// Mints and signs an [`OidcToken`] for `request`, computing `iat`/`exp`
    /// from the current time and `request.ttl_seconds`.
    pub fn mint(&self, request: MintRequest<'_>) -> Result<OidcToken, OidcSignerError> {
        let now = jsonwebtoken::get_current_timestamp();
        let claims = OidcClaims {
            iss: request.issuer.to_string(),
            sub: request.subject.clone(),
            aud: request.audience.to_string(),
            tenant_id: request.tenant_id.clone(),
            lease_id: request.lease_id.clone(),
            iat: now,
            exp: now + u64::from(request.ttl_seconds),
        };

        let header = Header::new(Algorithm::ES256);
        let encoded = encode(&header, &claims, &self.encoding_key).map_err(OidcSignerError::Signing)?;
        Ok(OidcToken::new(encoded))
    }

    /// The SEC1-encoded (uncompressed point) public key counterpart to this
    /// signer's private key, usable to build a [`DecodingKey`] via
    /// [`DecodingKey::from_ec_der`]. Exposed for tests and for any
    /// in-process verifier; there is no JWKS endpoint (see module docs).
    pub fn decoding_key(&self) -> DecodingKey {
        DecodingKey::from_ec_der(&self.verifying_public_key_sec1)
    }
}

/// Builds a [`Validation`] that checks signature, expiry, and an exact
/// `audience` match, mirroring [`trogon_sandbox_contract::OidcClaims`]'s doc
/// comment on why `aud` is exact-matched rather than prefix-matched.
pub fn validation_for_audience(audience: &str) -> Validation {
    let mut validation = Validation::new(Algorithm::ES256);
    validation.set_audience(&[audience]);
    validation.set_required_spec_claims(&["exp", "aud"]);
    validation
}

#[cfg(test)]
mod tests;
