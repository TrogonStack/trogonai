use jsonwebtoken::{Algorithm, EncodingKey, Header, encode};
use p256::ecdsa::SigningKey;
use pkcs8::{EncodePrivateKey, LineEnding};
use rand_core::OsRng;
use trogon_aauth_verify::{StaticJwks, SystemTimeSource, TimeSource, TokenVerifier};
use trogon_identity_types::aauth::person_server::TokenRequest;
use trogon_identity_types::aauth::{AgentClaims, Cnf, ResourceClaims, TYP_AGENT, TYP_AUTH, TYP_RESOURCE};

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

/// Mints an `aa-agent+jwt` carrying `parent_agent`, since `AgentClaims` has
/// no typed field for it yet -- mirrors how real sub-agent tokens are
/// minted, per `crate::subagent`.
fn mint_subagent_jwt(fixture: &KeyFixture, iss: &str, sub: &str, kid: &str, parent_agent: &str) -> String {
    let now = now_unix();
    let claims = serde_json::json!({
        "iss": iss,
        "sub": sub,
        "jti": "subagent-jti",
        "iat": now - 5,
        "exp": now + 600,
        "dwk": "aauth-agent.json",
        "cnf": { "jwk": fixture.jwk.clone() },
        "parent_agent": parent_agent,
    });
    let mut header = Header::new(Algorithm::ES256);
    header.typ = Some(TYP_AGENT.into());
    header.kid = Some(kid.to_string());
    encode(&header, &claims, &fixture.encoding_key).unwrap()
}

#[allow(clippy::too_many_arguments)]
fn mint_upstream_auth_jwt(fixture: &KeyFixture, iss: &str, aud: &str, sub: &str, agent: &str, kid: &str) -> String {
    let now = now_unix();
    let claims = serde_json::json!({
        "iss": iss,
        "sub": sub,
        "aud": aud,
        "jti": "auth-jti",
        "iat": now - 5,
        "exp": now + 600,
        "agent": agent,
        "agent_jkt": "root-jkt",
        "scope": "calendar.readwrite",
        "cnf": { "jwk": {"kty": "EC"} },
    });
    let mut header = Header::new(Algorithm::ES256);
    header.typ = Some(TYP_AUTH.into());
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

#[tokio::test]
async fn verify_request_rejects_agent_token_that_is_itself_a_subagent() {
    let agent_fixture = key_fixture("agent-kid-sa");
    let resource_fixture = key_fixture("resource-kid-sa");
    let jwks = StaticJwks::new()
        .with("https://ap.example", agent_fixture.jwk_set.clone())
        .with("https://calendar.example", resource_fixture.jwk_set.clone());
    let verifier = TokenVerifier::new(jwks, SystemTimeSource);

    // agent_token itself carries parent_agent -- must not directly request
    // authorization (single-level depth).
    let agent_jwt = mint_subagent_jwt(
        &agent_fixture,
        "https://ap.example",
        "aauth:sub@agent.example",
        "agent-kid-sa",
        "aauth:parent@agent.example",
    );
    let agent_jkt = jkt_of(&agent_fixture.jwk);
    let resource_jwt = mint_resource_jwt(
        &resource_fixture,
        "https://calendar.example",
        "https://ps.example",
        "aauth:sub@agent.example",
        &agent_jkt,
        "resource-kid-sa",
    );

    let req = TokenRequest::new(resource_jwt);
    let err = verify_request(&verifier, "https://ps.example", &agent_jwt, &req)
        .await
        .unwrap_err();
    assert!(matches!(err, RequestVerificationError::AgentTokenIsSubagent));
}

#[tokio::test]
async fn verify_request_accepts_subagent_bound_correctly() {
    let parent_fixture = key_fixture("parent-kid-ok");
    let sub_fixture = key_fixture("sub-kid-ok");
    let resource_fixture = key_fixture("resource-kid-ok");
    let jwks = StaticJwks::new()
        .with("https://ap.example", parent_fixture.jwk_set.clone())
        .with("https://subap.example", sub_fixture.jwk_set.clone())
        .with("https://calendar.example", resource_fixture.jwk_set.clone());
    let verifier = TokenVerifier::new(jwks, SystemTimeSource);

    let parent_jwt = mint_agent_jwt(
        &parent_fixture,
        "https://ap.example",
        "aauth:parent@agent.example",
        "parent-kid-ok",
    );
    let subagent_jwt = mint_subagent_jwt(
        &sub_fixture,
        "https://subap.example",
        "aauth:sub@agent.example",
        "sub-kid-ok",
        "aauth:parent@agent.example",
    );
    let sub_jkt = jkt_of(&sub_fixture.jwk);
    let resource_jwt = mint_resource_jwt(
        &resource_fixture,
        "https://calendar.example",
        "https://ps.example",
        "aauth:sub@agent.example",
        &sub_jkt,
        "resource-kid-ok",
    );

    let mut req = TokenRequest::new(resource_jwt);
    req.subagent_token = Some(subagent_jwt);
    let verified = verify_request(&verifier, "https://ps.example", &parent_jwt, &req)
        .await
        .unwrap();
    assert_eq!(verified.binding.agent, "aauth:sub@agent.example");
    assert_eq!(verified.binding.agent_jkt, sub_jkt);
    assert_eq!(verified.subagent_claims.unwrap().sub, "aauth:sub@agent.example");
}

#[tokio::test]
async fn verify_request_rejects_subagent_parent_mismatch() {
    let parent_fixture = key_fixture("parent-kid-mm");
    let sub_fixture = key_fixture("sub-kid-mm");
    let resource_fixture = key_fixture("resource-kid-mm");
    let jwks = StaticJwks::new()
        .with("https://ap.example", parent_fixture.jwk_set.clone())
        .with("https://subap.example", sub_fixture.jwk_set.clone())
        .with("https://calendar.example", resource_fixture.jwk_set.clone());
    let verifier = TokenVerifier::new(jwks, SystemTimeSource);

    let parent_jwt = mint_agent_jwt(
        &parent_fixture,
        "https://ap.example",
        "aauth:parent@agent.example",
        "parent-kid-mm",
    );
    // subagent's parent_agent points at someone other than agent_token's sub.
    let subagent_jwt = mint_subagent_jwt(
        &sub_fixture,
        "https://subap.example",
        "aauth:sub@agent.example",
        "sub-kid-mm",
        "aauth:someone-else@agent.example",
    );
    let sub_jkt = jkt_of(&sub_fixture.jwk);
    let resource_jwt = mint_resource_jwt(
        &resource_fixture,
        "https://calendar.example",
        "https://ps.example",
        "aauth:sub@agent.example",
        &sub_jkt,
        "resource-kid-mm",
    );

    let mut req = TokenRequest::new(resource_jwt);
    req.subagent_token = Some(subagent_jwt);
    let err = verify_request(&verifier, "https://ps.example", &parent_jwt, &req)
        .await
        .unwrap_err();
    assert!(matches!(err, RequestVerificationError::SubagentParentMismatch { .. }));
}

#[tokio::test]
async fn verify_request_rejects_subagent_token_that_is_itself_a_subagent() {
    let parent_fixture = key_fixture("parent-kid-depth");
    let sub_fixture = key_fixture("sub-kid-depth");
    let resource_fixture = key_fixture("resource-kid-depth");
    let jwks = StaticJwks::new()
        .with("https://ap.example", parent_fixture.jwk_set.clone())
        .with("https://subap.example", sub_fixture.jwk_set.clone())
        .with("https://calendar.example", resource_fixture.jwk_set.clone());
    let verifier = TokenVerifier::new(jwks, SystemTimeSource);

    let parent_jwt = mint_agent_jwt(
        &parent_fixture,
        "https://ap.example",
        "aauth:parent@agent.example",
        "parent-kid-depth",
    );
    // subagent_token itself has no parent_agent -- not a sub-agent token,
    // must be rejected (single-level depth).
    let subagent_jwt = mint_agent_jwt(
        &sub_fixture,
        "https://subap.example",
        "aauth:sub@agent.example",
        "sub-kid-depth",
    );
    let sub_jkt = jkt_of(&sub_fixture.jwk);
    let resource_jwt = mint_resource_jwt(
        &resource_fixture,
        "https://calendar.example",
        "https://ps.example",
        "aauth:sub@agent.example",
        &sub_jkt,
        "resource-kid-depth",
    );

    let mut req = TokenRequest::new(resource_jwt);
    req.subagent_token = Some(subagent_jwt);
    let err = verify_request(&verifier, "https://ps.example", &parent_jwt, &req)
        .await
        .unwrap_err();
    assert!(matches!(err, RequestVerificationError::SubagentTokenIsItselfSubagent));
}

#[tokio::test]
async fn verify_request_rejects_resource_token_subagent_identifier_mismatch() {
    let parent_fixture = key_fixture("parent-kid-id");
    let sub_fixture = key_fixture("sub-kid-id");
    let resource_fixture = key_fixture("resource-kid-id");
    let jwks = StaticJwks::new()
        .with("https://ap.example", parent_fixture.jwk_set.clone())
        .with("https://subap.example", sub_fixture.jwk_set.clone())
        .with("https://calendar.example", resource_fixture.jwk_set.clone());
    let verifier = TokenVerifier::new(jwks, SystemTimeSource);

    let parent_jwt = mint_agent_jwt(
        &parent_fixture,
        "https://ap.example",
        "aauth:parent@agent.example",
        "parent-kid-id",
    );
    let subagent_jwt = mint_subagent_jwt(
        &sub_fixture,
        "https://subap.example",
        "aauth:sub@agent.example",
        "sub-kid-id",
        "aauth:parent@agent.example",
    );
    // resource_token names a different agent than the presenting subagent.
    let sub_jkt = jkt_of(&sub_fixture.jwk);
    let resource_jwt = mint_resource_jwt(
        &resource_fixture,
        "https://calendar.example",
        "https://ps.example",
        "aauth:someone-else@agent.example",
        &sub_jkt,
        "resource-kid-id",
    );

    let mut req = TokenRequest::new(resource_jwt);
    req.subagent_token = Some(subagent_jwt);
    let err = verify_request(&verifier, "https://ps.example", &parent_jwt, &req)
        .await
        .unwrap_err();
    assert!(matches!(
        err,
        RequestVerificationError::ResourceTokenSubagentIdentifierMismatch
    ));
}

#[tokio::test]
async fn verify_request_rejects_resource_token_subagent_key_mismatch() {
    let parent_fixture = key_fixture("parent-kid-key");
    let sub_fixture = key_fixture("sub-kid-key");
    let other_fixture = key_fixture("other-kid-key");
    let resource_fixture = key_fixture("resource-kid-key");
    let jwks = StaticJwks::new()
        .with("https://ap.example", parent_fixture.jwk_set.clone())
        .with("https://subap.example", sub_fixture.jwk_set.clone())
        .with("https://calendar.example", resource_fixture.jwk_set.clone());
    let verifier = TokenVerifier::new(jwks, SystemTimeSource);

    let parent_jwt = mint_agent_jwt(
        &parent_fixture,
        "https://ap.example",
        "aauth:parent@agent.example",
        "parent-kid-key",
    );
    let subagent_jwt = mint_subagent_jwt(
        &sub_fixture,
        "https://subap.example",
        "aauth:sub@agent.example",
        "sub-kid-key",
        "aauth:parent@agent.example",
    );
    // resource_token binds to a different key's thumbprint than the
    // subagent's own cnf.jwk.
    let other_jkt = jkt_of(&other_fixture.jwk);
    let resource_jwt = mint_resource_jwt(
        &resource_fixture,
        "https://calendar.example",
        "https://ps.example",
        "aauth:sub@agent.example",
        &other_jkt,
        "resource-kid-key",
    );

    let mut req = TokenRequest::new(resource_jwt);
    req.subagent_token = Some(subagent_jwt);
    let err = verify_request(&verifier, "https://ps.example", &parent_jwt, &req)
        .await
        .unwrap_err();
    assert!(matches!(
        err,
        RequestVerificationError::ResourceTokenSubagentKeyMismatch
    ));
}

#[tokio::test]
async fn verify_request_accepts_upstream_token_bound_to_intermediary() {
    let agent_fixture = key_fixture("agent-kid-up");
    let resource_fixture = key_fixture("resource-kid-up");
    let upstream_fixture = key_fixture("upstream-kid-up");
    let jwks = StaticJwks::new()
        .with("https://ap.example", agent_fixture.jwk_set.clone())
        .with("https://calendar.example", resource_fixture.jwk_set.clone())
        .with("https://upstream-ps.example", upstream_fixture.jwk_set.clone());
    let verifier = TokenVerifier::new(jwks, SystemTimeSource);

    let agent_jwt = mint_agent_jwt(
        &agent_fixture,
        "https://ap.example",
        "aauth:intermediary@agent.example",
        "agent-kid-up",
    );
    let agent_jkt = jkt_of(&agent_fixture.jwk);
    let resource_jwt = mint_resource_jwt(
        &resource_fixture,
        "https://calendar.example",
        "https://ps.example",
        "aauth:intermediary@agent.example",
        &agent_jkt,
        "resource-kid-up",
    );
    // Upstream token's aud MUST equal the intermediary agent_token's iss.
    let upstream_jwt = mint_upstream_auth_jwt(
        &upstream_fixture,
        "https://upstream-ps.example",
        "https://ap.example",
        "user:alice",
        "aauth:root@agent.example",
        "upstream-kid-up",
    );

    let mut req = TokenRequest::new(resource_jwt);
    req.upstream_token = Some(upstream_jwt);
    let verified = verify_request(&verifier, "https://ps.example", &agent_jwt, &req)
        .await
        .unwrap();
    let upstream = verified.upstream.expect("upstream present");
    assert_eq!(upstream.claims.iss, "https://upstream-ps.example");
    assert_eq!(upstream.claims.aud, "https://ap.example");
}

#[tokio::test]
async fn verify_request_rejects_upstream_token_wrong_audience_as_upstream_token_error() {
    // `TokenVerifier::verify_auth` enforces `aud == expected_aud` itself
    // (see `trogon_aauth_verify::token::TokenVerifier::decode_with_jwks`),
    // so a mismatched upstream `aud` never reaches this module's own
    // `UpstreamAudienceBindingMismatch` check -- it surfaces as the
    // upstream token failing verification outright.
    let agent_fixture = key_fixture("agent-kid-upx");
    let resource_fixture = key_fixture("resource-kid-upx");
    let upstream_fixture = key_fixture("upstream-kid-upx");
    let jwks = StaticJwks::new()
        .with("https://ap.example", agent_fixture.jwk_set.clone())
        .with("https://calendar.example", resource_fixture.jwk_set.clone())
        .with("https://upstream-ps.example", upstream_fixture.jwk_set.clone());
    let verifier = TokenVerifier::new(jwks, SystemTimeSource);

    let agent_jwt = mint_agent_jwt(
        &agent_fixture,
        "https://ap.example",
        "aauth:intermediary@agent.example",
        "agent-kid-upx",
    );
    let agent_jkt = jkt_of(&agent_fixture.jwk);
    let resource_jwt = mint_resource_jwt(
        &resource_fixture,
        "https://calendar.example",
        "https://ps.example",
        "aauth:intermediary@agent.example",
        &agent_jkt,
        "resource-kid-upx",
    );
    // Upstream token's aud does NOT match the intermediary agent_token's iss.
    let upstream_jwt = mint_upstream_auth_jwt(
        &upstream_fixture,
        "https://upstream-ps.example",
        "https://some-other-audience.example",
        "user:alice",
        "aauth:root@agent.example",
        "upstream-kid-upx",
    );

    let mut req = TokenRequest::new(resource_jwt);
    req.upstream_token = Some(upstream_jwt);
    let err = verify_request(&verifier, "https://ps.example", &agent_jwt, &req)
        .await
        .unwrap_err();
    assert!(matches!(err, RequestVerificationError::UpstreamToken(_)));
}
