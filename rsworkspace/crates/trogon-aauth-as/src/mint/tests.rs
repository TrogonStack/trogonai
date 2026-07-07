use jsonwebtoken::Algorithm;
use trogon_aauth_verify::{StaticJwks, SystemTimeSource, TokenVerifier};
use trogon_identity_types::aauth::Act;

use super::*;
use crate::test_support::{jwk_set, key_fixture, now_unix};

#[tokio::test]
async fn minted_token_verifies_via_trogon_aauth_verify_with_agent_binding() {
    let as_key = key_fixture("as-kid-1");
    let agent_key = key_fixture("agent-kid-1");
    let now = now_unix();

    let binding = BindingAgent {
        agent: "aauth:asst@agent.example".into(),
        agent_jkt: "agent-jkt-1".into(),
    };
    let scope = GrantedScope::new("documents:read");
    let inputs = AuthTokenInputs {
        iss: "https://as.example",
        aud: "https://resource.example",
        sub: "user:alice",
        jti: "auth-jti-1",
        binding: &binding,
        cnf_jwk: agent_key.jwk_json.clone(),
        scope: &scope,
        act: None,
        principal: None,
        consent_id: None,
        resource: None,
        iat: now,
        ttl: AuthTokenTtl::new(300).unwrap(),
    };

    let jwt = mint_auth_jwt(&as_key.encoding_key, Algorithm::ES256, "as-kid-1", &inputs).unwrap();

    let jwks = StaticJwks::new().with("https://as.example", jwk_set(&[&as_key]));
    let verifier = TokenVerifier::new(jwks, SystemTimeSource);
    let verified = verifier.verify_auth(&jwt, "https://resource.example").await.unwrap();

    assert_eq!(verified.claims.iss, "https://as.example");
    assert_eq!(verified.claims.aud, "https://resource.example");
    assert_eq!(verified.claims.agent, "aauth:asst@agent.example");
    assert_eq!(verified.claims.agent_jkt, "agent-jkt-1");
    assert_eq!(verified.claims.scope, "documents:read");
}

#[test]
fn auth_token_ttl_rejects_non_positive_and_over_max() {
    assert_eq!(AuthTokenTtl::new(0).unwrap_err(), AuthTokenTtlError::NotPositive(0));
    assert_eq!(AuthTokenTtl::new(-5).unwrap_err(), AuthTokenTtlError::NotPositive(-5));
    assert_eq!(
        AuthTokenTtl::new(3601).unwrap_err(),
        AuthTokenTtlError::ExceedsMax(3601)
    );
    assert_eq!(AuthTokenTtl::new(3600).unwrap().get(), 3600);
}

#[test]
fn nest_act_wraps_upstream_chain_preserving_full_nesting() {
    let grandparent = Act {
        agent: "aauth:root@agent.example".into(),
        act: None,
    };
    let parent = nest_act("aauth:mid@agent.example", Some(grandparent));
    let leaf = nest_act("aauth:leaf@agent.example", Some(parent));

    assert_eq!(leaf.agent, "aauth:leaf@agent.example");
    let mid = leaf.act.as_ref().unwrap();
    assert_eq!(mid.agent, "aauth:mid@agent.example");
    let root = mid.act.as_ref().unwrap();
    assert_eq!(root.agent, "aauth:root@agent.example");
    assert!(root.act.is_none());
}

#[test]
fn nest_act_with_no_upstream_has_no_nested_act() {
    let act = nest_act("aauth:solo@agent.example", None);
    assert_eq!(act.agent, "aauth:solo@agent.example");
    assert!(act.act.is_none());
}

#[test]
fn mint_rejects_ttl_overflow() {
    let as_key = key_fixture("as-kid-2");
    let binding = BindingAgent {
        agent: "aauth:asst@agent.example".into(),
        agent_jkt: "agent-jkt-1".into(),
    };
    let scope = GrantedScope::new("documents:read");
    let inputs = AuthTokenInputs {
        iss: "https://as.example",
        aud: "https://resource.example",
        sub: "user:alice",
        jti: "auth-jti-2",
        binding: &binding,
        cnf_jwk: serde_json::json!({"kty": "EC"}),
        scope: &scope,
        act: None,
        principal: None,
        consent_id: None,
        resource: None,
        iat: i64::MAX - 10,
        ttl: AuthTokenTtl::new(300).unwrap(),
    };

    let err = mint_auth_jwt(&as_key.encoding_key, Algorithm::ES256, "as-kid-2", &inputs).unwrap_err();
    assert!(matches!(err, MintError::TtlOverflow { .. }));
}
