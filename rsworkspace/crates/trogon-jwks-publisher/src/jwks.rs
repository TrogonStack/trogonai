use std::fmt;

use jsonwebtoken::jwk::{Jwk, JwkSet};
use serde::{Deserialize, Serialize};

pub const KV_BUCKET: &str = "mcp-jwks";
pub const KV_KEY_CURRENT: &str = "mesh/current";
pub const REQREP_SUBJECT: &str = "mcp.jwks.mesh.get";
pub const REQREP_QUEUE_GROUP: &str = "trogon-jwks-publisher";

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Jwks {
    pub keys: Vec<Jwk>,
}

impl Jwks {
    pub fn empty() -> Self {
        Self { keys: Vec::new() }
    }

    pub fn from_jwk_set(set: JwkSet) -> Self {
        Self { keys: set.keys }
    }

    pub fn to_jwk_set(&self) -> JwkSet {
        JwkSet {
            keys: self.keys.clone(),
        }
    }

    pub fn to_json(&self) -> Result<Vec<u8>, serde_json::Error> {
        serde_json::to_vec(self)
    }

    pub fn to_json_pretty(&self) -> Result<String, serde_json::Error> {
        serde_json::to_string_pretty(self)
    }

    pub fn kids(&self) -> impl Iterator<Item = &str> {
        self.keys.iter().filter_map(|key| key.common.key_id.as_deref())
    }
}

impl fmt::Display for Jwks {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Jwks({} keys)", self.keys.len())
    }
}

#[cfg(test)]
mod tests {
    use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
    use jsonwebtoken::jwk::{
        AlgorithmParameters, CommonParameters, EllipticCurve, Jwk, KeyOperations, OctetKeyPairParameters,
        OctetKeyPairType, PublicKeyUse, RSAKeyParameters, RSAKeyType,
    };
    use rand::rngs::OsRng;
    use rsa::RsaPrivateKey;
    use rsa::traits::PublicKeyParts;

    use super::*;

    fn b64url_uint_be(bytes: &[u8]) -> String {
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

    fn sample_rsa_jwks() -> Jwks {
        let key = RsaPrivateKey::new(&mut OsRng, 2048).expect("rsa key");
        let public = key.to_public_key();
        let jwk = Jwk {
            common: CommonParameters {
                public_key_use: Some(PublicKeyUse::Signature),
                key_operations: Some(vec![KeyOperations::Verify]),
                key_id: Some("sample-kid".into()),
                ..Default::default()
            },
            algorithm: AlgorithmParameters::RSA(RSAKeyParameters {
                key_type: RSAKeyType::RSA,
                n: b64url_uint_be(&public.n().to_bytes_be()),
                e: b64url_uint_be(&public.e().to_bytes_be()),
            }),
        };
        Jwks { keys: vec![jwk] }
    }

    #[test]
    fn serializes_rfc7517_shape() {
        let jwks = sample_rsa_jwks();
        let json = serde_json::to_value(&jwks).expect("serialize");
        let obj = json.as_object().expect("object");
        let keys = obj.get("keys").and_then(|v| v.as_array()).expect("keys array");
        assert_eq!(keys.len(), 1);
        let key = keys[0].as_object().expect("jwk object");
        assert!(key.contains_key("kty"));
        assert!(key.contains_key("kid"));
    }

    #[test]
    fn round_trips_json_bytes() {
        let jwks = sample_rsa_jwks();
        let bytes = jwks.to_json().expect("to json");
        let parsed: Jwks = serde_json::from_slice(&bytes).expect("from json");
        assert_eq!(parsed, jwks);
    }

    #[test]
    fn accepts_okp_jwk_shape() {
        let jwk = Jwk {
            common: CommonParameters {
                public_key_use: Some(PublicKeyUse::Signature),
                key_operations: Some(vec![KeyOperations::Verify]),
                key_id: Some("ed-kid".into()),
                ..Default::default()
            },
            algorithm: AlgorithmParameters::OctetKeyPair(OctetKeyPairParameters {
                key_type: OctetKeyPairType::OctetKeyPair,
                curve: EllipticCurve::Ed25519,
                x: "abc".into(),
            }),
        };
        let jwks = Jwks { keys: vec![jwk] };
        let json = serde_json::to_string(&jwks).expect("serialize");
        assert!(json.contains("OKP"));
    }
}
