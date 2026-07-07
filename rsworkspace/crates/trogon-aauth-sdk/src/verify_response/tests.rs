#![allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]

use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use jsonwebtoken::Algorithm;
use p256::ecdsa::SigningKey;
use rand_core::OsRng;
use trogon_aauth_verify::StaticJwks;
use trogon_aauth_verify::time_source::SystemTimeSource;
use trogon_identity_types::aauth::{Act, Cnf, TYP_AUTH};

use super::*;

const RESOURCE_TOKEN_AUD: &str = "https://ps.example";
const RESOURCE: &str = "https://calendar.example";
const OWN_AGENT: &str = "aauth:asst@agent.example";

const P256_PEM: &[u8] = b"-----BEGIN PRIVATE KEY-----\nMIGHAgEAMBMGByqGSM49AgEGCCqGSM49AwEHBG0wawIBAQQgevZzL1gdAFr88hb2\nOF/2NxApJCzGCEDdfSp6VQO30hyhRANCAAQRWz+jn65BtOMvdyHKcvjBeBSDZH2r\n1RTwjmYSi9R/zpBnuQ4EiMnCqfMPWiZqB4QdbAd0E7oH50VpuZ1P087G\n-----END PRIVATE KEY-----\n";

fn p256_jwk_for_test() -> jsonwebtoken::jwk::Jwk {
    serde_json::from_value(serde_json::json!({
        "kty": "EC", "crv": "P-256", "kid": "k1", "alg": "ES256", "use": "sig",
        "x": "EVs_o5-uQbTjL3chynL4wXgUg2R9q9UU8I5mEovUf84",
        "y": "kGe5DgSIycKp8w9aJmoHhB1sB3QTugfnRWm5nU_TzsY"
    }))
    .unwrap()
}

fn signed_auth_token(claims: &serde_json::Value) -> String {
    let signing = jsonwebtoken::EncodingKey::from_ec_pem(P256_PEM).expect("signing key");
    let mut header = jsonwebtoken::Header::new(Algorithm::ES256);
    header.typ = Some(TYP_AUTH.into());
    header.kid = Some("k1".into());
    jsonwebtoken::encode(&header, claims, &signing).expect("encode")
}

fn verifier_with_key() -> TokenVerifier<StaticJwks, SystemTimeSource> {
    let jwks = StaticJwks::new().with(
        RESOURCE_TOKEN_AUD,
        jsonwebtoken::jwk::JwkSet {
            keys: vec![p256_jwk_for_test()],
        },
    );
    TokenVerifier::new(jwks, SystemTimeSource)
}

fn valid_auth_token_claims_json(agent_jwk: &serde_json::Value) -> serde_json::Value {
    serde_json::json!({
        "iss": RESOURCE_TOKEN_AUD,
        "sub": "user-1",
        "aud": RESOURCE,
        "jti": "auth-jti-1",
        "iat": 1_700_000_000,
        "exp": 9_999_999_999_i64,
        "agent": OWN_AGENT,
        "agent_jkt": "unused-legacy-field",
        "scope": "calendar.read",
        "cnf": { "jwk": agent_jwk },
    })
}

fn own_jwk() -> serde_json::Value {
    let sk = SigningKey::random(&mut OsRng);
    let point = sk.verifying_key().to_encoded_point(false);
    serde_json::json!({
        "kty": "EC",
        "crv": "P-256",
        "x": URL_SAFE_NO_PAD.encode(point.x().expect("x")),
        "y": URL_SAFE_NO_PAD.encode(point.y().expect("y")),
    })
}

fn valid_claims(agent_jwk: serde_json::Value) -> AuthClaims {
    AuthClaims {
        iss: RESOURCE_TOKEN_AUD.to_string(),
        sub: "user-1".to_string(),
        aud: RESOURCE.to_string(),
        jti: "auth-jti-1".to_string(),
        iat: 1_700_000_000,
        exp: 1_700_003_600,
        agent: OWN_AGENT.to_string(),
        agent_jkt: "unused-legacy-field".to_string(),
        scope: "calendar.read".to_string(),
        principal: None,
        consent_id: None,
        resource: None,
        act: None,
        cnf: Some(Cnf { jwk: agent_jwk }),
    }
}

#[test]
fn all_rules_pass_positive_case() {
    let jwk = own_jwk();
    let claims = valid_claims(jwk.clone());
    let result = verify_auth_claims(&claims, RESOURCE_TOKEN_AUD, RESOURCE, &jwk, OWN_AGENT, None)
        .expect("all rules should pass");
    assert_eq!(result.upstream_agent, None);
}

#[test]
fn rule2_fails_when_iss_does_not_match_resource_token_aud() {
    let jwk = own_jwk();
    let mut claims = valid_claims(jwk.clone());
    claims.iss = "https://wrong-ps.example".to_string();
    let err = verify_auth_claims(&claims, RESOURCE_TOKEN_AUD, RESOURCE, &jwk, OWN_AGENT, None).unwrap_err();
    assert!(matches!(err, VerifyResponseError::IssuerMismatch { .. }));
}

#[test]
fn rule3_fails_when_aud_does_not_match_intended_resource() {
    let jwk = own_jwk();
    let mut claims = valid_claims(jwk.clone());
    claims.aud = "https://other-resource.example".to_string();
    let err = verify_auth_claims(&claims, RESOURCE_TOKEN_AUD, RESOURCE, &jwk, OWN_AGENT, None).unwrap_err();
    assert!(matches!(err, VerifyResponseError::AudienceMismatch { .. }));
}

#[test]
fn rule4_fails_when_cnf_jwk_does_not_match_agents_own_key() {
    let jwk = own_jwk();
    let other_jwk = own_jwk();
    let claims = valid_claims(other_jwk);
    let err = verify_auth_claims(&claims, RESOURCE_TOKEN_AUD, RESOURCE, &jwk, OWN_AGENT, None).unwrap_err();
    assert!(matches!(err, VerifyResponseError::ConfirmationKeyMismatch));
}

#[test]
fn rule4_fails_when_cnf_claim_is_missing() {
    let jwk = own_jwk();
    let mut claims = valid_claims(jwk.clone());
    claims.cnf = None;
    let err = verify_auth_claims(&claims, RESOURCE_TOKEN_AUD, RESOURCE, &jwk, OWN_AGENT, None).unwrap_err();
    assert!(matches!(err, VerifyResponseError::ConfirmationClaimMissing));
}

#[test]
fn rule5_fails_when_agent_does_not_match_own_identifier() {
    let jwk = own_jwk();
    let mut claims = valid_claims(jwk.clone());
    claims.agent = "aauth:someone-else@agent.example".to_string();
    let err = verify_auth_claims(&claims, RESOURCE_TOKEN_AUD, RESOURCE, &jwk, OWN_AGENT, None).unwrap_err();
    assert!(matches!(err, VerifyResponseError::AgentMismatch { .. }));
}

#[test]
fn rule6_fails_when_act_agent_does_not_match_expected_upstream() {
    let jwk = own_jwk();
    let mut claims = valid_claims(jwk.clone());
    claims.act = Some(Act {
        agent: "aauth:booking@booking.example".to_string(),
        act: None,
    });
    let err = verify_auth_claims(
        &claims,
        RESOURCE_TOKEN_AUD,
        RESOURCE,
        &jwk,
        OWN_AGENT,
        Some("aauth:planner@agent.example"),
    )
    .unwrap_err();
    assert!(matches!(err, VerifyResponseError::UpstreamAgentMismatch { .. }));
}

#[test]
fn rule6_passes_and_surfaces_upstream_agent_when_expected_matches() {
    let jwk = own_jwk();
    let mut claims = valid_claims(jwk.clone());
    claims.act = Some(Act {
        agent: "aauth:booking@booking.example".to_string(),
        act: None,
    });
    let result = verify_auth_claims(
        &claims,
        RESOURCE_TOKEN_AUD,
        RESOURCE,
        &jwk,
        OWN_AGENT,
        Some("aauth:booking@booking.example"),
    )
    .expect("matching expected upstream agent should pass");
    assert_eq!(result.upstream_agent.as_deref(), Some("aauth:booking@booking.example"));
}

#[test]
fn rule6_does_not_fail_when_act_present_but_no_expectation_supplied() {
    let jwk = own_jwk();
    let mut claims = valid_claims(jwk.clone());
    claims.act = Some(Act {
        agent: "aauth:booking@booking.example".to_string(),
        act: None,
    });
    let result = verify_auth_claims(&claims, RESOURCE_TOKEN_AUD, RESOURCE, &jwk, OWN_AGENT, None)
        .expect("no expectation supplied means rule 6 is not enforced");
    assert_eq!(result.upstream_agent.as_deref(), Some("aauth:booking@booking.example"));
}

#[tokio::test(flavor = "current_thread")]
async fn with_jwks_verifies_signature_then_passes_claims_checks_end_to_end() {
    let verifier = verifier_with_key();
    let jwk = p256_jwk_for_test();
    let jwk_value = serde_json::to_value(&jwk).expect("jwk to value");
    let jwt = signed_auth_token(&valid_auth_token_claims_json(&jwk_value));

    let result = verify_auth_token_response_with_jwks(
        &verifier,
        &jwt,
        RESOURCE_TOKEN_AUD,
        RESOURCE,
        &jwk_value,
        OWN_AGENT,
        None,
    )
    .await
    .expect("signature and claims checks both pass");

    assert_eq!(result.upstream_agent, None);
}

#[tokio::test(flavor = "current_thread")]
async fn with_jwks_surfaces_signature_invalid_when_jwt_signature_is_corrupted() {
    let verifier = verifier_with_key();
    let jwk = p256_jwk_for_test();
    let jwk_value = serde_json::to_value(&jwk).expect("jwk to value");
    let jwt = signed_auth_token(&valid_auth_token_claims_json(&jwk_value));
    let corrupted = format!("{}AA", &jwt[..jwt.len() - 4]);

    let err = verify_auth_token_response_with_jwks(
        &verifier,
        &corrupted,
        RESOURCE_TOKEN_AUD,
        RESOURCE,
        &jwk_value,
        OWN_AGENT,
        None,
    )
    .await
    .unwrap_err();

    assert!(matches!(err, VerifyResponseError::SignatureInvalid(_)));
}

#[tokio::test(flavor = "current_thread")]
async fn with_jwks_runs_claims_checks_after_signature_and_surfaces_audience_mismatch() {
    let verifier = verifier_with_key();
    let jwk = p256_jwk_for_test();
    let jwk_value = serde_json::to_value(&jwk).expect("jwk to value");
    let jwt = signed_auth_token(&valid_auth_token_claims_json(&jwk_value));

    let err = verify_auth_token_response_with_jwks(
        &verifier,
        &jwt,
        RESOURCE_TOKEN_AUD,
        RESOURCE,
        &jwk_value,
        "aauth:someone-else@agent.example",
        None,
    )
    .await
    .unwrap_err();

    assert!(matches!(err, VerifyResponseError::AgentMismatch { .. }));
}
