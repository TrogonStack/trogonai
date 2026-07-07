use jsonwebtoken::{Algorithm, EncodingKey, Header, encode};
use p256::ecdsa::SigningKey;
use pkcs8::{EncodePrivateKey, LineEnding};
use rand_core::OsRng;
use trogon_aauth_verify::{StaticJwks, SystemTimeSource, TimeSource, TokenVerifier};
use trogon_identity_types::aauth::person_server::TokenRequest;
use trogon_identity_types::aauth::{AgentClaims, Cnf, ResourceClaims, TYP_AGENT, TYP_RESOURCE};

use super::*;

fn now_unix() -> i64 {
    SystemTimeSource.now()
}

struct KeyFixture {
    encoding_key: EncodingKey,
    jwk: serde_json::Value,
    jwk_set: jsonwebtoken::jwk::JwkSet,
}

fn key_fixture(kid: &str) -> KeyFixture {
    let signing_key = SigningKey::random(&mut OsRng);
    let pkcs8_pem = signing_key.to_pkcs8_pem(LineEnding::LF).expect("pkcs8 pem");
    let encoding_key = EncodingKey::from_ec_pem(pkcs8_pem.as_bytes()).expect("encoding key");

    let point = signing_key.verifying_key().to_encoded_point(false);
    let x = base64::Engine::encode(&base64::engine::general_purpose::URL_SAFE_NO_PAD, point.x().unwrap());
    let y = base64::Engine::encode(&base64::engine::general_purpose::URL_SAFE_NO_PAD, point.y().unwrap());
    let jwk = serde_json::json!({"kty": "EC", "crv": "P-256", "x": x, "y": y, "kid": kid});

    let jwk_set: jsonwebtoken::jwk::JwkSet = serde_json::from_value(serde_json::json!({"keys": [jwk]})).unwrap();

    KeyFixture {
        encoding_key,
        jwk,
        jwk_set,
    }
}

fn mint_agent_jwt(fixture: &KeyFixture, iss: &str, sub: &str, kid: &str) -> String {
    let now = now_unix();
    let claims = AgentClaims {
        iss: iss.to_string(),
        sub: sub.to_string(),
        jti: "agent-jti".to_string(),
        iat: now - 5,
        exp: now + 600,
        dwk: "aauth-agent.json".to_string(),
        cnf: Cnf {
            jwk: fixture.jwk.clone(),
        },
        ps: None,
    };
    let mut header = Header::new(Algorithm::ES256);
    header.typ = Some(TYP_AGENT.into());
    header.kid = Some(kid.to_string());
    encode(&header, &claims, &fixture.encoding_key).unwrap()
}

fn mint_resource_jwt(fixture: &KeyFixture, iss: &str, aud: &str, agent: &str, agent_jkt: &str, kid: &str) -> String {
    let now = now_unix();
    let claims = ResourceClaims {
        iss: iss.to_string(),
        aud: aud.to_string(),
        jti: "resource-jti".to_string(),
        iat: now - 5,
        exp: now + 600,
        dwk: "aauth-resource.json".to_string(),
        agent: agent.to_string(),
        agent_jkt: agent_jkt.to_string(),
        scope: "calendar.readwrite".to_string(),
        mission: None,
    };
    let mut header = Header::new(Algorithm::ES256);
    header.typ = Some(TYP_RESOURCE.into());
    header.kid = Some(kid.to_string());
    encode(&header, &claims, &fixture.encoding_key).unwrap()
}

fn jkt_of(jwk: &serde_json::Value) -> String {
    trogon_aauth_verify::jwk_thumbprint(jwk).unwrap()
}

#[tokio::test]
async fn verify_request_grants_binding_for_direct_agent() {
    let agent_fixture = key_fixture("agent-kid");
    let resource_fixture = key_fixture("resource-kid");

    let jwks = StaticJwks::new()
        .with("https://ap.example", agent_fixture.jwk_set.clone())
        .with("https://calendar.example", resource_fixture.jwk_set.clone());
    let verifier = TokenVerifier::new(jwks, SystemTimeSource);

    let agent_jwt = mint_agent_jwt(
        &agent_fixture,
        "https://ap.example",
        "aauth:assistant@agent.example",
        "agent-kid",
    );
    let agent_jkt = jkt_of(&agent_fixture.jwk);
    let resource_jwt = mint_resource_jwt(
        &resource_fixture,
        "https://calendar.example",
        "https://ps.example",
        "aauth:assistant@agent.example",
        &agent_jkt,
        "resource-kid",
    );

    let req = TokenRequest::new(resource_jwt);
    let verified = verify_request(&verifier, "https://ps.example", &agent_jwt, &req)
        .await
        .unwrap();

    assert_eq!(verified.binding.agent, "aauth:assistant@agent.example");
    assert_eq!(verified.binding.agent_jkt, agent_jkt);
    assert!(verified.subagent_claims.is_none());
    assert!(verified.upstream.is_none());
}

#[tokio::test]
async fn verify_request_rejects_resource_token_wrong_audience() {
    let agent_fixture = key_fixture("agent-kid");
    let resource_fixture = key_fixture("resource-kid");
    let jwks = StaticJwks::new()
        .with("https://ap.example", agent_fixture.jwk_set.clone())
        .with("https://calendar.example", resource_fixture.jwk_set.clone());
    let verifier = TokenVerifier::new(jwks, SystemTimeSource);

    let agent_jwt = mint_agent_jwt(
        &agent_fixture,
        "https://ap.example",
        "aauth:assistant@agent.example",
        "agent-kid",
    );
    let agent_jkt = jkt_of(&agent_fixture.jwk);
    let resource_jwt = mint_resource_jwt(
        &resource_fixture,
        "https://calendar.example",
        "https://wrong-ps.example",
        "aauth:assistant@agent.example",
        &agent_jkt,
        "resource-kid",
    );

    let req = TokenRequest::new(resource_jwt);
    let err = verify_request(&verifier, "https://ps.example", &agent_jwt, &req)
        .await
        .unwrap_err();
    assert!(matches!(err, RequestVerificationError::ResourceToken(_)));
}

#[tokio::test]
async fn verify_request_rejects_agent_key_mismatch() {
    let agent_fixture = key_fixture("agent-kid");
    let resource_fixture = key_fixture("resource-kid");
    let jwks = StaticJwks::new()
        .with("https://ap.example", agent_fixture.jwk_set.clone())
        .with("https://calendar.example", resource_fixture.jwk_set.clone());
    let verifier = TokenVerifier::new(jwks, SystemTimeSource);

    let agent_jwt = mint_agent_jwt(
        &agent_fixture,
        "https://ap.example",
        "aauth:assistant@agent.example",
        "agent-kid",
    );
    let resource_jwt = mint_resource_jwt(
        &resource_fixture,
        "https://calendar.example",
        "https://ps.example",
        "aauth:assistant@agent.example",
        "some-other-thumbprint",
        "resource-kid",
    );

    let req = TokenRequest::new(resource_jwt);
    let err = verify_request(&verifier, "https://ps.example", &agent_jwt, &req)
        .await
        .unwrap_err();
    assert!(matches!(err, RequestVerificationError::ResourceTokenAgentKeyMismatch));
}
