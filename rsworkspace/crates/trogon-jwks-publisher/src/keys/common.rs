use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use ed25519_dalek::VerifyingKey;
use jsonwebtoken::jwk::{
    AlgorithmParameters, CommonParameters, EllipticCurve, Jwk, KeyOperations, OctetKeyPairParameters, OctetKeyPairType,
    PublicKeyUse, RSAKeyParameters, RSAKeyType,
};
use rsa::RsaPublicKey;
use rsa::traits::PublicKeyParts;
use sha2::{Digest, Sha256};

use super::KeyError;

pub fn kid_from_public_der(der: &[u8]) -> String {
    let digest = Sha256::digest(der);
    hex::encode(&digest[..8])
}

pub fn b64url_uint_be(bytes: &[u8]) -> String {
    let start = bytes
        .iter()
        .position(|&b| b != 0)
        .unwrap_or(bytes.len().saturating_sub(1));
    let trimmed = if start >= bytes.len() {
        &bytes[bytes.len().saturating_sub(1)..]
    } else {
        &bytes[start..]
    };
    URL_SAFE_NO_PAD.encode(trimmed)
}

pub fn rsa_public_der_to_jwk(der: &[u8], kid: Option<String>) -> Result<Jwk, KeyError> {
    let public: RsaPublicKey = rsa::pkcs8::DecodePublicKey::from_public_key_der(der)
        .map_err(|e| KeyError::Parse(format!("rsa public key der: {e}")))?;
    let kid = kid.unwrap_or_else(|| kid_from_public_der(der));
    let n = b64url_uint_be(&public.n().to_bytes_be());
    let e = b64url_uint_be(&public.e().to_bytes_be());
    Ok(Jwk {
        common: CommonParameters {
            public_key_use: Some(PublicKeyUse::Signature),
            key_operations: Some(vec![KeyOperations::Verify]),
            key_id: Some(kid),
            ..Default::default()
        },
        algorithm: AlgorithmParameters::RSA(RSAKeyParameters {
            key_type: RSAKeyType::RSA,
            n,
            e,
        }),
    })
}

pub fn ed25519_public_der_to_jwk(der: &[u8], kid: Option<String>) -> Result<Jwk, KeyError> {
    let verifying_key: VerifyingKey = ed25519_dalek::pkcs8::DecodePublicKey::from_public_key_der(der)
        .map_err(|e| KeyError::Parse(format!("ed25519 public key der: {e}")))?;
    let kid = kid.unwrap_or_else(|| kid_from_public_der(der));
    let x = URL_SAFE_NO_PAD.encode(verifying_key.as_bytes());
    Ok(Jwk {
        common: CommonParameters {
            public_key_use: Some(PublicKeyUse::Signature),
            key_operations: Some(vec![KeyOperations::Verify]),
            key_id: Some(kid),
            ..Default::default()
        },
        algorithm: AlgorithmParameters::OctetKeyPair(OctetKeyPairParameters {
            key_type: OctetKeyPairType::OctetKeyPair,
            curve: EllipticCurve::Ed25519,
            x,
        }),
    })
}

pub fn spki_der_to_jwk(der: &[u8], kid: Option<String>) -> Result<Jwk, KeyError> {
    if let Ok(jwk) = rsa_public_der_to_jwk(der, kid.clone()) {
        return Ok(jwk);
    }
    ed25519_public_der_to_jwk(der, kid)
}
