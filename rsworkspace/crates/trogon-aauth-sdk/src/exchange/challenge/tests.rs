#![allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]

use jsonwebtoken::Algorithm;
use trogon_aauth_verify::StaticJwks;
use trogon_aauth_verify::time_source::SystemTimeSource;
use trogon_identity_types::aauth::TYP_RESOURCE;

use super::*;

const RESOURCE: &str = "https://calendar.example";
const OWN_AGENT: &str = "aauth:asst@agent.example";
const OWN_JKT: &str = "own-jkt-value";

const P256_PEM: &[u8] = b"-----BEGIN PRIVATE KEY-----\nMIGHAgEAMBMGByqGSM49AgEGCCqGSM49AwEHBG0wawIBAQQgevZzL1gdAFr88hb2\nOF/2NxApJCzGCEDdfSp6VQO30hyhRANCAAQRWz+jn65BtOMvdyHKcvjBeBSDZH2r\n1RTwjmYSi9R/zpBnuQ4EiMnCqfMPWiZqB4QdbAd0E7oH50VpuZ1P087G\n-----END PRIVATE KEY-----\n";

fn p256_jwk_for_test() -> jsonwebtoken::jwk::Jwk {
    serde_json::from_value(serde_json::json!({
        "kty": "EC", "crv": "P-256", "kid": "k1", "alg": "ES256", "use": "sig",
        "x": "EVs_o5-uQbTjL3chynL4wXgUg2R9q9UU8I5mEovUf84",
        "y": "kGe5DgSIycKp8w9aJmoHhB1sB3QTugfnRWm5nU_TzsY"
    }))
    .unwrap()
}

fn signed_resource_token(claims: &serde_json::Value) -> String {
    let signing = jsonwebtoken::EncodingKey::from_ec_pem(P256_PEM).expect("signing key");
    let mut header = jsonwebtoken::Header::new(Algorithm::ES256);
    header.typ = Some(TYP_RESOURCE.into());
    header.kid = Some("k1".into());
    jsonwebtoken::encode(&header, claims, &signing).expect("encode")
}

fn verifier_with_key() -> TokenVerifier<StaticJwks, SystemTimeSource> {
    let jwks = StaticJwks::new().with(
        RESOURCE,
        jsonwebtoken::jwk::JwkSet {
            keys: vec![p256_jwk_for_test()],
        },
    );
    TokenVerifier::new(jwks, SystemTimeSource)
}

fn valid_resource_token_claims_json() -> serde_json::Value {
    serde_json::json!({
        "iss": RESOURCE,
        "aud": "https://ps.example",
        "jti": "resource-jti-1",
        "iat": 1_700_000_000,
        "exp": 9_999_999_999_i64,
        "dwk": "aauth-resource.json",
        "agent": OWN_AGENT,
        "agent_jkt": OWN_JKT,
        "scope": "calendar.read",
    })
}

fn valid_claims() -> ResourceClaims {
    ResourceClaims {
        iss: RESOURCE.to_string(),
        aud: "https://ps.example".to_string(),
        jti: "resource-jti-1".to_string(),
        iat: 1_700_000_000,
        exp: 1_700_003_600,
        dwk: "aauth-resource.json".to_string(),
        agent: OWN_AGENT.to_string(),
        agent_jkt: OWN_JKT.to_string(),
        scope: "calendar.read".to_string(),
        mission: None,
    }
}

#[test]
fn all_steps_pass_positive_case() {
    let claims = valid_claims();
    assert!(verify_resource_challenge_claims(&claims, RESOURCE, OWN_AGENT, OWN_JKT, 1_700_000_100).is_ok());
}

#[test]
fn step3_fails_when_iss_does_not_match_requested_resource() {
    let mut claims = valid_claims();
    claims.iss = "https://other-resource.example".to_string();
    let err = verify_resource_challenge_claims(&claims, RESOURCE, OWN_AGENT, OWN_JKT, 1_700_000_100).unwrap_err();
    assert!(matches!(err, ChallengeVerifyError::IssuerMismatch { .. }));
}

#[test]
fn step4_fails_when_agent_does_not_match_own_identifier() {
    let mut claims = valid_claims();
    claims.agent = "aauth:someone-else@agent.example".to_string();
    let err = verify_resource_challenge_claims(&claims, RESOURCE, OWN_AGENT, OWN_JKT, 1_700_000_100).unwrap_err();
    assert!(matches!(err, ChallengeVerifyError::AgentMismatch { .. }));
}

#[test]
fn step5_fails_when_agent_jkt_does_not_match_own_jkt() {
    let mut claims = valid_claims();
    claims.agent_jkt = "wrong-jkt".to_string();
    let err = verify_resource_challenge_claims(&claims, RESOURCE, OWN_AGENT, OWN_JKT, 1_700_000_100).unwrap_err();
    assert!(matches!(err, ChallengeVerifyError::JktMismatch { .. }));
}

#[test]
fn step6_fails_when_exp_is_not_in_the_future() {
    let claims = valid_claims();
    let err = verify_resource_challenge_claims(&claims, RESOURCE, OWN_AGENT, OWN_JKT, claims.exp + 1).unwrap_err();
    assert!(matches!(err, ChallengeVerifyError::Expired { .. }));
}

#[tokio::test(flavor = "current_thread")]
async fn with_jwks_verifies_signature_then_passes_claims_checks_end_to_end() {
    let verifier = verifier_with_key();
    let jwt = signed_resource_token(&valid_resource_token_claims_json());

    let claims = verify_resource_challenge_with_jwks(
        &verifier,
        &jwt,
        "https://ps.example",
        RESOURCE,
        OWN_AGENT,
        OWN_JKT,
        1_700_000_100,
    )
    .await
    .expect("signature and claims checks both pass");

    assert_eq!(claims.iss, RESOURCE);
    assert_eq!(claims.agent, OWN_AGENT);
}

#[tokio::test(flavor = "current_thread")]
async fn with_jwks_surfaces_signature_invalid_when_jwt_signature_is_corrupted() {
    let verifier = verifier_with_key();
    let jwt = signed_resource_token(&valid_resource_token_claims_json());
    let corrupted = format!("{}AA", &jwt[..jwt.len() - 4]);

    let err = verify_resource_challenge_with_jwks(
        &verifier,
        &corrupted,
        "https://ps.example",
        RESOURCE,
        OWN_AGENT,
        OWN_JKT,
        1_700_000_100,
    )
    .await
    .unwrap_err();

    assert!(matches!(err, ChallengeVerifyError::SignatureInvalid(_)));
}

#[tokio::test(flavor = "current_thread")]
async fn with_jwks_runs_claims_checks_after_signature_and_surfaces_issuer_mismatch() {
    let verifier = verifier_with_key();
    let mut claims_json = valid_resource_token_claims_json();
    claims_json["iss"] = serde_json::json!(RESOURCE);
    let jwt = signed_resource_token(&claims_json);

    let err = verify_resource_challenge_with_jwks(
        &verifier,
        &jwt,
        "https://ps.example",
        "https://other-resource.example",
        OWN_AGENT,
        OWN_JKT,
        1_700_000_100,
    )
    .await
    .unwrap_err();

    assert!(matches!(err, ChallengeVerifyError::IssuerMismatch { .. }));
}
