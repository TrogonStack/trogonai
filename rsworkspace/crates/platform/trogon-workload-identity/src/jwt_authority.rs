use aws_lc_rs::signature::{
    ECDSA_P256_SHA256_FIXED, ECDSA_P384_SHA384_FIXED, ECDSA_P521_SHA512_FIXED, ParsedPublicKey,
};
use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use jsonwebtoken::{DecodingKey, crypto};

use crate::constants::P521_SIGNATURE_BYTES;
use crate::jwt_algorithm::JwtSvidAlgorithm;

/// Which signature algorithms a key may verify. A key never verifies outside
/// its kind, so an RSA key cannot be used as an HMAC secret and an EC key only
/// verifies the algorithm of its own curve.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum AuthorityKind {
    Rsa,
    EcP256,
    EcP384,
    EcP521,
}

impl AuthorityKind {
    pub(crate) fn accepts(self, algorithm: JwtSvidAlgorithm) -> bool {
        match self {
            Self::Rsa => algorithm.is_rsa(),
            Self::EcP256 => algorithm == JwtSvidAlgorithm::Es256,
            Self::EcP384 => algorithm == JwtSvidAlgorithm::Es384,
            Self::EcP521 => algorithm == JwtSvidAlgorithm::Es512,
        }
    }
}

/// The verification key material. P-521 is verified with aws-lc-rs because
/// `jsonwebtoken` has no ES512 support.
#[derive(Debug, Clone)]
pub(crate) enum AuthorityKey {
    Jose(DecodingKey),
    P521(ParsedPublicKey),
}

impl AuthorityKey {
    /// Parses an uncompressed SEC1 point (`0x04 || x || y`), refusing a point
    /// that is not on the curve.
    pub(crate) fn p521(uncompressed_point: &[u8]) -> Option<Self> {
        ParsedPublicKey::new(&ECDSA_P521_SHA512_FIXED, uncompressed_point)
            .ok()
            .map(Self::P521)
    }

    /// Builds a P-256 or P-384 key for `jsonwebtoken` from an uncompressed SEC1
    /// point and its base64url coordinates. `DecodingKey::from_ec_components`
    /// does not check the point, so aws-lc-rs parses it first and a point that
    /// is not on the curve is refused.
    pub(crate) fn p256_or_p384(kind: AuthorityKind, uncompressed_point: &[u8], x: &str, y: &str) -> Option<Self> {
        let algorithm = match kind {
            AuthorityKind::EcP256 => &ECDSA_P256_SHA256_FIXED,
            AuthorityKind::EcP384 => &ECDSA_P384_SHA384_FIXED,
            AuthorityKind::Rsa | AuthorityKind::EcP521 => return None,
        };
        ParsedPublicKey::new(algorithm, uncompressed_point).ok()?;
        DecodingKey::from_ec_components(x, y).ok().map(Self::Jose)
    }
}

/// One public JWT-SVID signing key of a trust domain's JWT bundle.
#[derive(Debug, Clone)]
pub(crate) struct JwtAuthority {
    pub(crate) key_id: String,
    pub(crate) kind: AuthorityKind,
    pub(crate) key: AuthorityKey,
}

impl JwtAuthority {
    /// Whether `signature` (base64url) is a valid signature of `signing_input`
    /// by this key under `algorithm`. False when the key does not fit the
    /// algorithm.
    pub(crate) fn verifies(&self, algorithm: JwtSvidAlgorithm, signing_input: &str, signature: &str) -> bool {
        if !self.kind.accepts(algorithm) {
            return false;
        }
        match (&self.key, algorithm.backend()) {
            (AuthorityKey::Jose(key), Some(backend)) => {
                crypto::verify(signature, signing_input.as_bytes(), key, backend).unwrap_or(false)
            }
            (AuthorityKey::P521(key), None) => match URL_SAFE_NO_PAD.decode(signature) {
                Ok(raw) => raw.len() == P521_SIGNATURE_BYTES && key.verify_sig(signing_input.as_bytes(), &raw).is_ok(),
                Err(_) => false,
            },
            _ => false,
        }
    }
}
