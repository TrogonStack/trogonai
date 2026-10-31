use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use p256::ecdsa::SigningKey;
use serde_json::json;

use super::PlaidKeyId;
use super::verification_key::{PlaidJwk, PlaidVerificationKey};

pub struct TestKeyPair {
    key_id: String,
    x: String,
    y: String,
}

impl TestKeyPair {
    pub fn generate(key_id: &str) -> Self {
        let signing_key = SigningKey::random(&mut rand_core::OsRng);
        let point = signing_key.verifying_key().to_encoded_point(false);
        Self {
            key_id: key_id.to_owned(),
            x: URL_SAFE_NO_PAD.encode(point.x().unwrap()),
            y: URL_SAFE_NO_PAD.encode(point.y().unwrap()),
        }
    }

    pub fn key_id(&self) -> PlaidKeyId {
        PlaidKeyId::new(&self.key_id).unwrap()
    }

    pub fn jwk(&self, expired_at: Option<u64>) -> PlaidJwk {
        PlaidJwk {
            alg: "ES256".to_owned(),
            crv: "P-256".to_owned(),
            kty: "EC".to_owned(),
            kid: self.key_id.clone(),
            x: self.x.clone(),
            y: self.y.clone(),
            expired_at,
        }
    }

    pub fn jwk_json(&self, expired_at: Option<u64>) -> serde_json::Value {
        json!({
            "key": {
                "alg": "ES256",
                "crv": "P-256",
                "kty": "EC",
                "use": "sig",
                "kid": self.key_id,
                "x": self.x,
                "y": self.y,
                "created_at": 1_600_000_000u64,
                "expired_at": expired_at,
            }
        })
    }

    pub fn verification_key(&self, expired_at: Option<u64>) -> PlaidVerificationKey {
        PlaidVerificationKey::from_jwk(&self.jwk(expired_at), &self.key_id()).unwrap()
    }
}
