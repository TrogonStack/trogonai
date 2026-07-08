use jsonwebtoken::{Algorithm, DecodingKey, EncodingKey, Validation, decode};
use p256::ecdsa::SigningKey;
use pkcs8::{EncodePrivateKey, EncodePublicKey};
use rand_core::OsRng;
use trogon_aauth_verify::TimeSource;
use trogon_identity_types::aauth::{Act, TYP_AUTH};

use super::*;

fn test_key() -> (EncodingKey, serde_json::Value, DecodingKey) {
    let signing_key = SigningKey::random(&mut OsRng);
    let pkcs8_pem = signing_key.to_pkcs8_pem(pkcs8::LineEnding::LF).expect("encode pkcs8");
    let encoding_key = EncodingKey::from_ec_pem(pkcs8_pem.as_bytes()).expect("encoding key");

    let verifying_key = signing_key.verifying_key();
    let point = verifying_key.to_encoded_point(false);
    let x = base64::Engine::encode(&base64::engine::general_purpose::URL_SAFE_NO_PAD, point.x().unwrap());
    let y = base64::Engine::encode(&base64::engine::general_purpose::URL_SAFE_NO_PAD, point.y().unwrap());
    let jwk = serde_json::json!({"kty": "EC", "crv": "P-256", "x": x, "y": y});

    let pub_pem = verifying_key.to_public_key_pem(pkcs8::LineEnding::LF).expect("pub pem");
    let decoding_key = DecodingKey::from_ec_pem(pub_pem.as_bytes()).expect("decoding key");

    (encoding_key, jwk, decoding_key)
}

#[test]
fn auth_token_ttl_rejects_non_positive() {
    assert!(matches!(AuthTokenTtl::new(0), Err(AuthTokenTtlError::NotPositive(0))));
    assert!(matches!(AuthTokenTtl::new(-1), Err(AuthTokenTtlError::NotPositive(-1))));
}

#[test]
fn auth_token_ttl_rejects_over_one_hour() {
    assert!(matches!(
        AuthTokenTtl::new(3601),
        Err(AuthTokenTtlError::ExceedsMax(3601))
    ));
}

#[test]
fn auth_token_ttl_accepts_valid_range() {
    assert_eq!(AuthTokenTtl::new(1).unwrap().get(), 1);
    assert_eq!(AuthTokenTtl::new(3600).unwrap().get(), 3600);
}

#[test]
fn auth_token_ttl_default_is_max() {
    assert_eq!(AuthTokenTtl::default().get(), AuthTokenTtl::MAX_SECS);
}

#[test]
fn nest_act_wraps_intermediary_around_upstream_chain() {
    let upstream_act = Act {
        agent: "aauth:root@agent.example".to_string(),
        act: None,
    };
    let nested = nest_act("aauth:intermediary@agent.example", Some(upstream_act.clone()));
    assert_eq!(nested.agent, "aauth:intermediary@agent.example");
    assert_eq!(nested.act.as_deref(), Some(&upstream_act));
}

#[test]
fn nest_act_with_no_upstream_has_no_nested_act() {
    let nested = nest_act("aauth:solo@agent.example", None);
    assert_eq!(nested.agent, "aauth:solo@agent.example");
    assert!(nested.act.is_none());
}

#[test]
fn mint_auth_jwt_produces_expected_typ_dwk_and_claims() {
    let (encoding_key, jwk, decoding_key) = test_key();
    let binding = BindingAgent {
        agent: "aauth:assistant@agent.example".to_string(),
        agent_jkt: "thumbprint123".to_string(),
    };
    let now = trogon_aauth_verify::SystemTimeSource.now();
    let inputs = AuthTokenInputs {
        iss: "https://ps.example",
        aud: "https://calendar.example",
        sub: "aauth:assistant@agent.example",
        jti: "jti-1",
        binding: &binding,
        cnf_jwk: jwk,
        scope: "calendar.readwrite",
        act: None,
        principal: Some("user@example.com"),
        consent_id: None,
        resource: Some("https://calendar.example"),
        iat: now,
        ttl: AuthTokenTtl::new(3600).unwrap(),
    };

    let jwt = mint_auth_jwt(&encoding_key, Algorithm::ES256, "kid-1", &inputs).expect("mint");

    let header = jsonwebtoken::decode_header(&jwt).unwrap();
    assert_eq!(header.typ.as_deref(), Some(TYP_AUTH));
    assert_eq!(header.kid.as_deref(), Some("kid-1"));

    let mut validation = Validation::new(Algorithm::ES256);
    validation.set_audience(&["https://calendar.example"]);
    validation.set_issuer(&["https://ps.example"]);
    let data = decode::<serde_json::Value>(&jwt, &decoding_key, &validation).expect("decode");
    assert_eq!(data.claims["dwk"], "aauth-person.json");
    assert_eq!(data.claims["scope"], "calendar.readwrite");
    assert_eq!(data.claims["agent"], "aauth:assistant@agent.example");
    assert_eq!(data.claims["principal"], "user@example.com");
    assert_eq!(data.claims["exp"], now + 3600);
}

#[test]
fn mint_auth_jwt_ttl_overflow_is_rejected() {
    let (encoding_key, jwk, _decoding_key) = test_key();
    let binding = BindingAgent {
        agent: "aauth:a@agent.example".to_string(),
        agent_jkt: "jkt".to_string(),
    };
    let inputs = AuthTokenInputs {
        iss: "https://ps.example",
        aud: "https://resource.example",
        sub: "aauth:a@agent.example",
        jti: "jti",
        binding: &binding,
        cnf_jwk: jwk,
        scope: "read",
        act: None,
        principal: None,
        consent_id: None,
        resource: None,
        iat: i64::MAX,
        ttl: AuthTokenTtl::new(10).unwrap(),
    };
    let err = mint_auth_jwt(&encoding_key, Algorithm::ES256, "kid", &inputs).unwrap_err();
    assert!(matches!(err, MintError::TtlOverflow { .. }));
}
