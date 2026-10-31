use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use jsonwebtoken::{Algorithm, EncodingKey, Header};
use p256::ecdsa::SigningKey;
use p256::pkcs8::EncodePrivateKey;
use serde_json::json;
use sha2::{Digest, Sha256};

use super::PlaidKeyId;
use super::verification_key::{PlaidJwk, PlaidVerificationKey};

pub struct TestKeyPair {
    key_id: String,
    encoding_key: EncodingKey,
    x: String,
    y: String,
}

impl TestKeyPair {
    pub fn generate(key_id: &str) -> Self {
        let signing_key = SigningKey::random(&mut rand_core::OsRng);
        let der = signing_key.to_pkcs8_der().unwrap();
        let point = signing_key.verifying_key().to_encoded_point(false);
        Self {
            key_id: key_id.to_owned(),
            encoding_key: EncodingKey::from_ec_der(der.as_bytes()),
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

    pub fn sign(&self, claims: &serde_json::Value) -> String {
        let mut header = Header::new(Algorithm::ES256);
        header.kid = Some(self.key_id.clone());
        jsonwebtoken::encode(&header, claims, &self.encoding_key).unwrap()
    }

    pub fn token(&self, iat: u64, body: &[u8]) -> String {
        self.sign(&json!({
            "iat": iat,
            "request_body_sha256": body_sha256(body),
        }))
    }

    pub fn token_without_key_id(&self, iat: u64, body: &[u8]) -> String {
        let header = Header::new(Algorithm::ES256);
        jsonwebtoken::encode(
            &header,
            &json!({
                "iat": iat,
                "request_body_sha256": body_sha256(body),
            }),
            &self.encoding_key,
        )
        .unwrap()
    }
}

fn body_sha256(body: &[u8]) -> String {
    hex::encode(Sha256::digest(body))
}

pub fn hs256_token(key_id: &str, iat: u64, body: &[u8]) -> String {
    let mut header = Header::new(Algorithm::HS256);
    header.kid = Some(key_id.to_owned());
    jsonwebtoken::encode(
        &header,
        &json!({
            "iat": iat,
            "request_body_sha256": body_sha256(body),
        }),
        &EncodingKey::from_secret(b"not-the-plaid-key"),
    )
    .unwrap()
}
