//! Shared P-256 fixtures + token minting helpers for this crate's unit and
//! integration tests. Mirrors `trogon-aauth-verify::test_support` /
//! `a2a-gateway/tests/aauth_roundtrip.rs`'s fixture style.

#![allow(clippy::expect_used, clippy::unwrap_used)]

use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use jsonwebtoken::jwk::{
    AlgorithmParameters, CommonParameters, EllipticCurve, EllipticCurveKeyParameters, EllipticCurveKeyType, Jwk,
    JwkSet, PublicKeyUse,
};
use jsonwebtoken::{Algorithm, EncodingKey, Header, encode};
use p256::ecdsa::SigningKey;
use p256::pkcs8::EncodePrivateKey;
use rand_core::OsRng;
use trogon_identity_types::aauth::{Cnf, DWK_AGENT, DWK_RESOURCE, TYP_AGENT, TYP_AUTH, TYP_RESOURCE};

pub struct KeyFixture {
    pub encoding_key: EncodingKey,
    pub jwk: Jwk,
    pub jwk_json: serde_json::Value,
}

pub fn key_fixture(kid: &str) -> KeyFixture {
    let signing_key = SigningKey::random(&mut OsRng);
    let verifying = signing_key.verifying_key();
    let point = verifying.to_encoded_point(false);
    let x = URL_SAFE_NO_PAD.encode(point.x().expect("x"));
    let y = URL_SAFE_NO_PAD.encode(point.y().expect("y"));
    let pem = signing_key
        .to_pkcs8_pem(p256::pkcs8::LineEnding::LF)
        .expect("pkcs8 pem")
        .to_string();
    let encoding_key = EncodingKey::from_ec_pem(pem.as_bytes()).expect("encoding key");
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
    let jwk_json = serde_json::json!({"kty": "EC", "crv": "P-256", "x": x, "y": y, "kid": kid});
    KeyFixture {
        encoding_key,
        jwk,
        jwk_json,
    }
}

pub fn jwk_set(fixtures: &[&KeyFixture]) -> JwkSet {
    JwkSet {
        keys: fixtures.iter().map(|f| f.jwk.clone()).collect(),
    }
}

#[allow(clippy::too_many_arguments)]
pub fn mint_agent_jwt(
    ap: &KeyFixture,
    ap_kid: &str,
    ap_iss: &str,
    agent_sub: &str,
    agent_jwk: &serde_json::Value,
    parent_agent: Option<&str>,
    now: i64,
) -> String {
    let mut header = Header::new(Algorithm::ES256);
    header.typ = Some(TYP_AGENT.into());
    header.kid = Some(ap_kid.into());
    let mut claims = serde_json::json!({
        "iss": ap_iss,
        "sub": agent_sub,
        "jti": format!("agent-jti-{agent_sub}"),
        "iat": now - 5,
        "exp": now + 600,
        "dwk": DWK_AGENT,
        "cnf": Cnf { jwk: agent_jwk.clone() },
    });
    if let Some(parent) = parent_agent {
        claims["parent_agent"] = serde_json::Value::String(parent.to_string());
    }
    encode(&header, &claims, &ap.encoding_key).expect("encode agent jwt")
}

#[allow(clippy::too_many_arguments)]
pub fn mint_resource_jwt(
    resource: &KeyFixture,
    resource_kid: &str,
    resource_iss: &str,
    aud: &str,
    agent: &str,
    agent_jkt: &str,
    scope: &str,
    now: i64,
) -> String {
    let mut header = Header::new(Algorithm::ES256);
    header.typ = Some(TYP_RESOURCE.into());
    header.kid = Some(resource_kid.into());
    let claims = serde_json::json!({
        "iss": resource_iss,
        "aud": aud,
        "jti": format!("resource-jti-{agent}"),
        "iat": now - 5,
        "exp": now + 600,
        "dwk": DWK_RESOURCE,
        "agent": agent,
        "agent_jkt": agent_jkt,
        "scope": scope,
    });
    encode(&header, &claims, &resource.encoding_key).expect("encode resource jwt")
}

/// Mints an `aa-auth+jwt` directly (bypassing this crate's own `mint`
/// module) so tests can construct arbitrary upstream tokens, including ones
/// with a pre-existing `act` chain, independent of the code under test.
#[allow(clippy::too_many_arguments)]
pub fn mint_auth_jwt_raw(
    issuer: &KeyFixture,
    issuer_kid: &str,
    iss: &str,
    aud: &str,
    sub: &str,
    agent: &str,
    agent_jkt: &str,
    agent_jwk: &serde_json::Value,
    scope: &str,
    act: Option<serde_json::Value>,
    now: i64,
) -> String {
    let mut header = Header::new(Algorithm::ES256);
    header.typ = Some(TYP_AUTH.into());
    header.kid = Some(issuer_kid.into());
    let mut claims = serde_json::json!({
        "iss": iss,
        "sub": sub,
        "aud": aud,
        "jti": format!("auth-jti-{agent}"),
        "iat": now - 5,
        "exp": now + 600,
        "agent": agent,
        "agent_jkt": agent_jkt,
        "scope": scope,
        "cnf": { "jwk": agent_jwk },
    });
    if let Some(act) = act {
        claims["act"] = act;
    }
    encode(&header, &claims, &issuer.encoding_key).expect("encode auth jwt")
}

pub fn now_unix() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(1_700_000_000)
}
