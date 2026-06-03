//! Agent keypair value object. P-256 / ES256 only for v0; EdDSA is reserved
//! for a follow-up.

use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use jsonwebtoken::{Algorithm, EncodingKey};
use p256::ecdsa::{SigningKey, signature::Signer};
use p256::pkcs8::{DecodePrivateKey, EncodePrivateKey};
use rand_core::OsRng;
use serde_json::Value;
use trogon_aauth_verify::jwk_thumbprint;

use crate::error::SdkError;

/// Agent keypair: holds the private ECDSA key and produces both the JWS-friendly
/// [`EncodingKey`] and the public JWK that goes into `cnf.jwk` at bootstrap.
pub struct AgentKeypair {
    signing: SigningKey,
    encoding: EncodingKey,
    public_jwk: Value,
    jkt: String,
}

impl AgentKeypair {
    /// Generate a fresh ES256 keypair using the OS RNG.
    pub fn generate() -> Result<Self, SdkError> {
        let signing = SigningKey::random(&mut OsRng);
        Self::from_signing(signing)
    }

    /// Reconstruct from an existing PKCS#8 PEM-encoded private key (useful for
    /// persistent agents whose key sits in a Kubernetes Secret).
    pub fn from_pkcs8_pem(pem: &str) -> Result<Self, SdkError> {
        let signing = SigningKey::from_pkcs8_pem(pem)
            .map_err(|e| SdkError::InvalidResponse(format!("pkcs8 pem: {e}")))?;
        Self::from_signing(signing)
    }

    fn from_signing(signing: SigningKey) -> Result<Self, SdkError> {
        let verifying = signing.verifying_key();
        let point = verifying.to_encoded_point(false);
        let x = URL_SAFE_NO_PAD.encode(
            point
                .x()
                .ok_or_else(|| SdkError::InvalidResponse("verifying key missing x".into()))?,
        );
        let y = URL_SAFE_NO_PAD.encode(
            point
                .y()
                .ok_or_else(|| SdkError::InvalidResponse("verifying key missing y".into()))?,
        );
        let public_jwk = serde_json::json!({
            "kty": "EC",
            "crv": "P-256",
            "x": x,
            "y": y,
            "alg": "ES256",
            "use": "sig",
        });
        let jkt = jwk_thumbprint(&public_jwk)
            .map_err(|e| SdkError::InvalidResponse(format!("jkt: {e}")))?;
        let pem = signing
            .to_pkcs8_pem(p256::pkcs8::LineEnding::LF)
            .map_err(|e| SdkError::InvalidResponse(format!("pkcs8: {e}")))?
            .to_string();
        let encoding = EncodingKey::from_ec_pem(pem.as_bytes())?;

        Ok(Self {
            signing,
            encoding,
            public_jwk,
            jkt,
        })
    }

    /// The agent's signing algorithm. Fixed at ES256 for v0.
    #[must_use]
    pub fn algorithm(&self) -> Algorithm {
        Algorithm::ES256
    }

    /// The agent's public JWK (`cnf.jwk` payload). Includes `alg=ES256`, `use=sig`.
    #[must_use]
    pub fn public_jwk(&self) -> &Value {
        &self.public_jwk
    }

    /// RFC 7638 JWK thumbprint of [`public_jwk`].
    #[must_use]
    pub fn jkt(&self) -> &str {
        &self.jkt
    }

    /// [`EncodingKey`] suitable for `jsonwebtoken::encode`.
    #[must_use]
    pub fn encoding_key(&self) -> &EncodingKey {
        &self.encoding
    }

    /// Sign raw bytes (the canonical signature base) returning a URL-safe
    /// base64 (no padding) ECDSA-P256 signature.
    pub fn sign_raw(&self, base: &[u8]) -> String {
        let sig: p256::ecdsa::Signature = self.signing.sign(base);
        URL_SAFE_NO_PAD.encode(sig.to_bytes())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn generate_round_trips_through_pem() {
        let kp = AgentKeypair::generate().expect("generate");
        let pem = kp
            .signing
            .to_pkcs8_pem(p256::pkcs8::LineEnding::LF)
            .unwrap()
            .to_string();
        let again = AgentKeypair::from_pkcs8_pem(&pem).expect("from pem");
        assert_eq!(kp.jkt(), again.jkt());
    }

    #[test]
    fn jwk_has_expected_fields() {
        let kp = AgentKeypair::generate().unwrap();
        let v = kp.public_jwk();
        assert_eq!(v["kty"], "EC");
        assert_eq!(v["crv"], "P-256");
        assert_eq!(v["alg"], "ES256");
    }
}
