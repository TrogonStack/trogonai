//! Shared P-256 / P-384 / Ed25519 fixtures for unit tests.

use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use ed25519_dalek::Signer as _;
use jsonwebtoken::jwk::{
    AlgorithmParameters, CommonParameters, EllipticCurve, EllipticCurveKeyParameters, EllipticCurveKeyType, Jwk,
    JwkSet, OctetKeyPairParameters, OctetKeyPairType, PublicKeyUse,
};
use jsonwebtoken::{Algorithm, EncodingKey};
use p256::ecdsa::SigningKey as P256SigningKey;
use pkcs8::EncodePrivateKey as _;
use rand_core::OsRng;

const P384_PEM: &str = "-----BEGIN PRIVATE KEY-----\nMIG2AgEAMBAGByqGSM49AgEGBSuBBAAiBIGeMIGbAgEBBDBRfxviTDonWX6aS/fj\nZQ5uj7QiwHx2HpCzR1nxJYLtcaMOOqhYM2zxJqu0rW3a+n+hZANiAAS+3cPsTGpv\nQ8xFfyqnTXHTF5e07BSpwnK7t2GdF+KyDyr9TTmsb9JbHoEaqE+k3cB/KYDrC+on\nQDspxD+EYcbcqLuUV9yv/5fEaWhal28ILO1UEKhml1x6NfXENjhr09U=\n-----END PRIVATE KEY-----\n";

const P384_JWK_JSON: &str = r#"{
    "kty": "EC",
    "crv": "P-384",
    "kid": "p384-k1",
    "alg": "ES384",
    "x": "vt3D7Exqb0PMRX8qp01x0xeXtOwUqcJyu7dhnRfisg8q_U05rG_SWx6BGqhPpN3A",
    "y": "fymA6wvqJ0A7KcQ_hGHG3Ki7lFfcr_-XxGloWpdvCCztVBCoZpdcejX1xDY4a9PV"
}"#;

pub struct EcFixture {
    pub signing: EncodingKey,
    pub jwk: Jwk,
    pub jwk_json: serde_json::Value,
    pub alg: Algorithm,
}

pub struct EdFixture {
    pub signing: ed25519_dalek::SigningKey,
    pub encoding: EncodingKey,
    pub jwk: Jwk,
    pub jwk_json: serde_json::Value,
}

impl EdFixture {
    pub fn sign_pop_base(&self, base: &[u8]) -> String {
        URL_SAFE_NO_PAD.encode(self.signing.sign(base).to_bytes())
    }
}

pub fn p256_fixture(kid: &str) -> EcFixture {
    let native = P256SigningKey::random(&mut OsRng);
    let verifying = native.verifying_key();
    let point = verifying.to_encoded_point(false);
    let x = URL_SAFE_NO_PAD.encode(point.x().expect("x"));
    let y = URL_SAFE_NO_PAD.encode(point.y().expect("y"));
    let pem = native.to_pkcs8_pem(p256::pkcs8::LineEnding::LF).expect("pkcs8 pem");
    let signing = EncodingKey::from_ec_pem(pem.as_bytes()).expect("encoding key");
    let jwk = Jwk {
        common: CommonParameters {
            public_key_use: Some(PublicKeyUse::Signature),
            key_id: Some(kid.into()),
            ..Default::default()
        },
        algorithm: AlgorithmParameters::EllipticCurve(EllipticCurveKeyParameters {
            key_type: EllipticCurveKeyType::EC,
            curve: EllipticCurve::P256,
            x: x.clone(),
            y: y.clone(),
        }),
    };
    let jwk_json = serde_json::json!({
        "kty": "EC",
        "crv": "P-256",
        "x": x,
        "y": y,
        "kid": kid,
        "alg": "ES256",
    });
    EcFixture {
        signing,
        jwk,
        jwk_json,
        alg: Algorithm::ES256,
    }
}

pub fn p384_fixture() -> EcFixture {
    let signing = EncodingKey::from_ec_pem(P384_PEM.as_bytes()).expect("encoding key");
    let jwk: Jwk = serde_json::from_str(P384_JWK_JSON).expect("jwk");
    let jwk_json: serde_json::Value = serde_json::from_str(P384_JWK_JSON).expect("jwk json");
    EcFixture {
        signing,
        jwk,
        jwk_json,
        alg: Algorithm::ES384,
    }
}

pub fn ed25519_fixture(kid: &str) -> EdFixture {
    let signing = ed25519_dalek::SigningKey::generate(&mut OsRng);
    let x = URL_SAFE_NO_PAD.encode(signing.verifying_key().as_bytes());
    let pem = signing.to_pkcs8_pem(pkcs8::LineEnding::LF).expect("pkcs8 pem");
    let encoding = EncodingKey::from_ed_pem(pem.as_bytes()).expect("encoding key");
    let jwk = Jwk {
        common: CommonParameters {
            public_key_use: Some(PublicKeyUse::Signature),
            key_id: Some(kid.into()),
            ..Default::default()
        },
        algorithm: AlgorithmParameters::OctetKeyPair(OctetKeyPairParameters {
            key_type: OctetKeyPairType::OctetKeyPair,
            curve: EllipticCurve::Ed25519,
            x: x.clone(),
        }),
    };
    let jwk_json = serde_json::json!({
        "kty": "OKP",
        "crv": "Ed25519",
        "x": x,
        "kid": kid,
        "alg": "EdDSA",
    });
    EdFixture {
        signing,
        encoding,
        jwk,
        jwk_json,
    }
}

pub fn sign_p384_pop(base: &[u8]) -> String {
    let signing = EncodingKey::from_ec_pem(P384_PEM.as_bytes()).expect("encoding key");
    jsonwebtoken::crypto::sign(base, &signing, Algorithm::ES384).expect("sign")
}

pub fn jwks_with_key(issuer: &str, key: Jwk) -> crate::jwks::StaticJwks {
    crate::jwks::StaticJwks::new().with(issuer, JwkSet { keys: vec![key] })
}
