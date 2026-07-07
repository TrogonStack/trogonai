use trogon_aauth_verify::{StaticJwks, SystemTimeSource, TokenVerifier};
use trogon_identity_types::aauth::federation::AsTokenRequest;

use super::*;
use crate::test_support::{jwk_set, key_fixture, mint_agent_jwt, mint_auth_jwt_raw, mint_resource_jwt, now_unix};
use crate::trust::TrustBasisRecord;

const AS_ISS: &str = "https://as.example";
const PS_ISS: &str = "https://ps.example";
const AP_ISS: &str = "https://ap.example";
const RESOURCE_ISS: &str = "https://resource.example";

fn base_request(
    now: i64,
) -> (
    AsTokenRequest,
    TrustRegistry,
    TokenVerifier<StaticJwks, SystemTimeSource>,
) {
    let as_key = key_fixture("as-kid");
    let ap_key = key_fixture("ap-kid");
    let resource_key = key_fixture("resource-kid");
    let agent_key = key_fixture("agent-key");

    let agent_token = mint_agent_jwt(
        &ap_key,
        "ap-kid",
        AP_ISS,
        "aauth:asst@agent.example",
        &agent_key.jwk_json,
        None,
        now,
    );
    let agent_jkt = trogon_aauth_verify::jwk_thumbprint(&agent_key.jwk_json).unwrap();
    let resource_token = mint_resource_jwt(
        &resource_key,
        "resource-kid",
        RESOURCE_ISS,
        AS_ISS,
        "aauth:asst@agent.example",
        &agent_jkt,
        "documents:read",
        now,
    );

    let jwks = StaticJwks::new()
        .with(AP_ISS, jwk_set(&[&ap_key]))
        .with(RESOURCE_ISS, jwk_set(&[&resource_key]))
        .with(AS_ISS, jwk_set(&[&as_key]));
    let verifier = TokenVerifier::new(jwks, SystemTimeSource);
    let trust = TrustRegistry::from_entries([(PS_ISS.to_string(), TrustBasisRecord::PreEstablished)]).unwrap();

    let req = AsTokenRequest {
        resource_token,
        agent_token,
        subagent_token: None,
        upstream_token: None,
    };
    (req, trust, verifier)
}

#[tokio::test]
async fn verify_request_rejects_untrusted_ps() {
    let now = now_unix();
    let (req, trust, verifier) = base_request(now);

    let err = verify_request(&verifier, &trust, AS_ISS, "https://unknown-ps.example", &req)
        .await
        .unwrap_err();
    assert!(matches!(err, RequestVerificationError::UntrustedPs(_)));
}

#[tokio::test]
async fn verify_request_accepts_trusted_ps_and_binds_agent() {
    let now = now_unix();
    let (req, trust, verifier) = base_request(now);

    let (verified, trusted) = verify_request(&verifier, &trust, AS_ISS, PS_ISS, &req).await.unwrap();
    assert_eq!(trusted.issuer().as_str(), PS_ISS);
    assert_eq!(verified.binding.agent, "aauth:asst@agent.example");
    assert!(verified.subagent_claims.is_none());
    assert!(verified.upstream.is_none());
}

#[tokio::test]
async fn verify_request_rejects_resource_token_wrong_audience_as_resource_token_error() {
    // `TokenVerifier::verify_resource` itself enforces `aud == expected_aud`
    // (same `decode_with_jwks` path as `verify_auth`), so a mismatched
    // resource `aud` never reaches this module's own
    // `ResourceTokenWrongAudience` check -- it surfaces as the resource
    // token failing verification outright.
    let now = now_unix();
    let as_key = key_fixture("as-kid");
    let ap_key = key_fixture("ap-kid");
    let resource_key = key_fixture("resource-kid");
    let agent_key = key_fixture("agent-key");

    let agent_token = mint_agent_jwt(
        &ap_key,
        "ap-kid",
        AP_ISS,
        "aauth:asst@agent.example",
        &agent_key.jwk_json,
        None,
        now,
    );
    let agent_jkt = trogon_aauth_verify::jwk_thumbprint(&agent_key.jwk_json).unwrap();
    // aud points at a different AS than the one verifying this request.
    let resource_token = mint_resource_jwt(
        &resource_key,
        "resource-kid",
        RESOURCE_ISS,
        "https://other-as.example",
        "aauth:asst@agent.example",
        &agent_jkt,
        "documents:read",
        now,
    );

    let jwks = StaticJwks::new()
        .with(AP_ISS, jwk_set(&[&ap_key]))
        .with(RESOURCE_ISS, jwk_set(&[&resource_key]))
        .with(AS_ISS, jwk_set(&[&as_key]));
    let verifier = TokenVerifier::new(jwks, SystemTimeSource);
    let trust = TrustRegistry::from_entries([(PS_ISS.to_string(), TrustBasisRecord::PreEstablished)]).unwrap();
    let req = AsTokenRequest {
        resource_token,
        agent_token,
        subagent_token: None,
        upstream_token: None,
    };

    let err = verify_request(&verifier, &trust, AS_ISS, PS_ISS, &req)
        .await
        .unwrap_err();
    assert!(matches!(err, RequestVerificationError::ResourceToken(_)));
}

#[tokio::test]
async fn verify_request_rejects_agent_key_binding_mismatch() {
    let now = now_unix();
    let as_key = key_fixture("as-kid");
    let ap_key = key_fixture("ap-kid");
    let resource_key = key_fixture("resource-kid");
    let agent_key = key_fixture("agent-key");
    let other_key = key_fixture("other-key");

    let agent_token = mint_agent_jwt(
        &ap_key,
        "ap-kid",
        AP_ISS,
        "aauth:asst@agent.example",
        &agent_key.jwk_json,
        None,
        now,
    );
    // resource_token binds to a different key's thumbprint than the agent's own cnf.jwk.
    let other_jkt = trogon_aauth_verify::jwk_thumbprint(&other_key.jwk_json).unwrap();
    let resource_token = mint_resource_jwt(
        &resource_key,
        "resource-kid",
        RESOURCE_ISS,
        AS_ISS,
        "aauth:asst@agent.example",
        &other_jkt,
        "documents:read",
        now,
    );

    let jwks = StaticJwks::new()
        .with(AP_ISS, jwk_set(&[&ap_key]))
        .with(RESOURCE_ISS, jwk_set(&[&resource_key]))
        .with(AS_ISS, jwk_set(&[&as_key]));
    let verifier = TokenVerifier::new(jwks, SystemTimeSource);
    let trust = TrustRegistry::from_entries([(PS_ISS.to_string(), TrustBasisRecord::PreEstablished)]).unwrap();
    let req = AsTokenRequest {
        resource_token,
        agent_token,
        subagent_token: None,
        upstream_token: None,
    };

    let err = verify_request(&verifier, &trust, AS_ISS, PS_ISS, &req)
        .await
        .unwrap_err();
    assert!(matches!(err, RequestVerificationError::ResourceTokenAgentKeyMismatch));
}

#[tokio::test]
async fn verify_request_rejects_agent_token_that_is_itself_a_subagent() {
    let now = now_unix();
    let as_key = key_fixture("as-kid");
    let ap_key = key_fixture("ap-kid");
    let resource_key = key_fixture("resource-kid");
    let agent_key = key_fixture("agent-key");

    // agent_token itself carries parent_agent -- must not be used to directly
    // request authorization (single-level depth).
    let agent_token = mint_agent_jwt(
        &ap_key,
        "ap-kid",
        AP_ISS,
        "aauth:sub@agent.example",
        &agent_key.jwk_json,
        Some("aauth:parent@agent.example"),
        now,
    );
    let agent_jkt = trogon_aauth_verify::jwk_thumbprint(&agent_key.jwk_json).unwrap();
    let resource_token = mint_resource_jwt(
        &resource_key,
        "resource-kid",
        RESOURCE_ISS,
        AS_ISS,
        "aauth:sub@agent.example",
        &agent_jkt,
        "documents:read",
        now,
    );

    let jwks = StaticJwks::new()
        .with(AP_ISS, jwk_set(&[&ap_key]))
        .with(RESOURCE_ISS, jwk_set(&[&resource_key]))
        .with(AS_ISS, jwk_set(&[&as_key]));
    let verifier = TokenVerifier::new(jwks, SystemTimeSource);
    let trust = TrustRegistry::from_entries([(PS_ISS.to_string(), TrustBasisRecord::PreEstablished)]).unwrap();
    let req = AsTokenRequest {
        resource_token,
        agent_token,
        subagent_token: None,
        upstream_token: None,
    };

    let err = verify_request(&verifier, &trust, AS_ISS, PS_ISS, &req)
        .await
        .unwrap_err();
    assert!(matches!(err, RequestVerificationError::AgentTokenIsSubagent));
}

#[tokio::test]
async fn verify_request_accepts_subagent_bound_correctly() {
    let now = now_unix();
    let as_key = key_fixture("as-kid");
    let ap_key = key_fixture("ap-kid");
    let resource_key = key_fixture("resource-kid");
    let parent_key = key_fixture("parent-key");
    let sub_key = key_fixture("sub-key");

    let agent_token = mint_agent_jwt(
        &ap_key,
        "ap-kid",
        AP_ISS,
        "aauth:parent@agent.example",
        &parent_key.jwk_json,
        None,
        now,
    );
    let subagent_token = mint_agent_jwt(
        &ap_key,
        "ap-kid",
        AP_ISS,
        "aauth:sub@agent.example",
        &sub_key.jwk_json,
        Some("aauth:parent@agent.example"),
        now,
    );
    let sub_jkt = trogon_aauth_verify::jwk_thumbprint(&sub_key.jwk_json).unwrap();
    let resource_token = mint_resource_jwt(
        &resource_key,
        "resource-kid",
        RESOURCE_ISS,
        AS_ISS,
        "aauth:sub@agent.example",
        &sub_jkt,
        "documents:read",
        now,
    );

    let jwks = StaticJwks::new()
        .with(AP_ISS, jwk_set(&[&ap_key]))
        .with(RESOURCE_ISS, jwk_set(&[&resource_key]))
        .with(AS_ISS, jwk_set(&[&as_key]));
    let verifier = TokenVerifier::new(jwks, SystemTimeSource);
    let trust = TrustRegistry::from_entries([(PS_ISS.to_string(), TrustBasisRecord::PreEstablished)]).unwrap();
    let req = AsTokenRequest {
        resource_token,
        agent_token,
        subagent_token: Some(subagent_token),
        upstream_token: None,
    };

    let (verified, _) = verify_request(&verifier, &trust, AS_ISS, PS_ISS, &req).await.unwrap();
    assert_eq!(verified.binding.agent, "aauth:sub@agent.example");
    assert_eq!(verified.subagent_claims.unwrap().sub, "aauth:sub@agent.example");
}

#[tokio::test]
async fn verify_request_rejects_subagent_parent_mismatch() {
    let now = now_unix();
    let as_key = key_fixture("as-kid");
    let ap_key = key_fixture("ap-kid");
    let resource_key = key_fixture("resource-kid");
    let parent_key = key_fixture("parent-key");
    let sub_key = key_fixture("sub-key");

    let agent_token = mint_agent_jwt(
        &ap_key,
        "ap-kid",
        AP_ISS,
        "aauth:parent@agent.example",
        &parent_key.jwk_json,
        None,
        now,
    );
    // subagent's parent_agent points at someone other than agent_token's sub.
    let subagent_token = mint_agent_jwt(
        &ap_key,
        "ap-kid",
        AP_ISS,
        "aauth:sub@agent.example",
        &sub_key.jwk_json,
        Some("aauth:someone-else@agent.example"),
        now,
    );
    let sub_jkt = trogon_aauth_verify::jwk_thumbprint(&sub_key.jwk_json).unwrap();
    let resource_token = mint_resource_jwt(
        &resource_key,
        "resource-kid",
        RESOURCE_ISS,
        AS_ISS,
        "aauth:sub@agent.example",
        &sub_jkt,
        "documents:read",
        now,
    );

    let jwks = StaticJwks::new()
        .with(AP_ISS, jwk_set(&[&ap_key]))
        .with(RESOURCE_ISS, jwk_set(&[&resource_key]))
        .with(AS_ISS, jwk_set(&[&as_key]));
    let verifier = TokenVerifier::new(jwks, SystemTimeSource);
    let trust = TrustRegistry::from_entries([(PS_ISS.to_string(), TrustBasisRecord::PreEstablished)]).unwrap();
    let req = AsTokenRequest {
        resource_token,
        agent_token,
        subagent_token: Some(subagent_token),
        upstream_token: None,
    };

    let err = verify_request(&verifier, &trust, AS_ISS, PS_ISS, &req)
        .await
        .unwrap_err();
    assert!(matches!(err, RequestVerificationError::SubagentParentMismatch { .. }));
}

#[tokio::test]
async fn verify_request_accepts_trusted_upstream_token_bound_to_intermediary() {
    let now = now_unix();

    // The upstream (call-chaining) issuer must itself be trusted, alongside the PS.
    let mut trust = TrustRegistry::from_entries([(PS_ISS.to_string(), TrustBasisRecord::PreEstablished)]).unwrap();
    let upstream_issuer_key = key_fixture("upstream-as-kid");
    trust.trust(
        crate::trust::PsIssuer::new("https://upstream-as.example").unwrap(),
        TrustBasisRecord::PreEstablished,
    );

    let mut jwks = StaticJwks::new();
    let as_key = key_fixture("as-kid-2");
    let ap_key = key_fixture("ap-kid-2");
    let resource_key = key_fixture("resource-kid-2");
    let agent_key = key_fixture("agent-key-2");

    let agent_token = mint_agent_jwt(
        &ap_key,
        "ap-kid-2",
        AP_ISS,
        "aauth:intermediary@agent.example",
        &agent_key.jwk_json,
        None,
        now,
    );
    let agent_jkt = trogon_aauth_verify::jwk_thumbprint(&agent_key.jwk_json).unwrap();
    let resource_token = mint_resource_jwt(
        &resource_key,
        "resource-kid-2",
        RESOURCE_ISS,
        AS_ISS,
        "aauth:intermediary@agent.example",
        &agent_jkt,
        "documents:read",
        now,
    );

    // Upstream token's aud MUST equal the intermediary agent_token's iss (AP_ISS).
    let upstream_token = mint_auth_jwt_raw(
        &upstream_issuer_key,
        "upstream-as-kid",
        "https://upstream-as.example",
        AP_ISS,
        "user:alice",
        "aauth:root@agent.example",
        "root-jkt",
        &serde_json::json!({"kty": "EC"}),
        "documents:read",
        None,
        now,
    );

    jwks.insert("https://upstream-as.example", jwk_set(&[&upstream_issuer_key]));
    jwks.insert(AP_ISS, jwk_set(&[&ap_key]));
    jwks.insert(RESOURCE_ISS, jwk_set(&[&resource_key]));
    jwks.insert(AS_ISS, jwk_set(&[&as_key]));
    let verifier = TokenVerifier::new(jwks, SystemTimeSource);

    let req = AsTokenRequest {
        resource_token,
        agent_token,
        subagent_token: None,
        upstream_token: Some(upstream_token),
    };

    let (verified, _) = verify_request(&verifier, &trust, AS_ISS, PS_ISS, &req).await.unwrap();
    let upstream = verified.upstream.expect("upstream present");
    assert_eq!(upstream.claims.iss, "https://upstream-as.example");
    assert_eq!(upstream.claims.aud, AP_ISS);
}

#[tokio::test]
async fn verify_request_rejects_upstream_token_untrusted_issuer() {
    let now = now_unix();
    let as_key = key_fixture("as-kid-3");
    let ap_key = key_fixture("ap-kid-3");
    let resource_key = key_fixture("resource-kid-3");
    let agent_key = key_fixture("agent-key-3");
    let untrusted_upstream_key = key_fixture("untrusted-upstream-kid");

    let agent_token = mint_agent_jwt(
        &ap_key,
        "ap-kid-3",
        AP_ISS,
        "aauth:intermediary@agent.example",
        &agent_key.jwk_json,
        None,
        now,
    );
    let agent_jkt = trogon_aauth_verify::jwk_thumbprint(&agent_key.jwk_json).unwrap();
    let resource_token = mint_resource_jwt(
        &resource_key,
        "resource-kid-3",
        RESOURCE_ISS,
        AS_ISS,
        "aauth:intermediary@agent.example",
        &agent_jkt,
        "documents:read",
        now,
    );
    let upstream_token = mint_auth_jwt_raw(
        &untrusted_upstream_key,
        "untrusted-upstream-kid",
        "https://untrusted-as.example",
        AP_ISS,
        "user:alice",
        "aauth:root@agent.example",
        "root-jkt",
        &serde_json::json!({"kty": "EC"}),
        "documents:read",
        None,
        now,
    );

    let jwks = StaticJwks::new()
        .with(AP_ISS, jwk_set(&[&ap_key]))
        .with(RESOURCE_ISS, jwk_set(&[&resource_key]))
        .with(AS_ISS, jwk_set(&[&as_key]))
        .with("https://untrusted-as.example", jwk_set(&[&untrusted_upstream_key]));
    let verifier = TokenVerifier::new(jwks, SystemTimeSource);
    let trust = TrustRegistry::from_entries([(PS_ISS.to_string(), TrustBasisRecord::PreEstablished)]).unwrap();
    let req = AsTokenRequest {
        resource_token,
        agent_token,
        subagent_token: None,
        upstream_token: Some(upstream_token),
    };

    let err = verify_request(&verifier, &trust, AS_ISS, PS_ISS, &req)
        .await
        .unwrap_err();
    assert!(matches!(err, RequestVerificationError::UpstreamUntrustedIssuer(_)));
}

#[tokio::test]
async fn verify_request_rejects_subagent_key_binding_mismatch() {
    let now = now_unix();
    let as_key = key_fixture("as-kid-sk");
    let ap_key = key_fixture("ap-kid-sk");
    let resource_key = key_fixture("resource-kid-sk");
    let parent_key = key_fixture("parent-key-sk");
    let sub_key = key_fixture("sub-key-sk");
    let other_key = key_fixture("other-key-sk");

    let agent_token = mint_agent_jwt(
        &ap_key,
        "ap-kid-sk",
        AP_ISS,
        "aauth:parent@agent.example",
        &parent_key.jwk_json,
        None,
        now,
    );
    let subagent_token = mint_agent_jwt(
        &ap_key,
        "ap-kid-sk",
        AP_ISS,
        "aauth:sub@agent.example",
        &sub_key.jwk_json,
        Some("aauth:parent@agent.example"),
        now,
    );
    // resource_token binds to a different key's thumbprint than the
    // subagent's own cnf.jwk.
    let other_jkt = trogon_aauth_verify::jwk_thumbprint(&other_key.jwk_json).unwrap();
    let resource_token = mint_resource_jwt(
        &resource_key,
        "resource-kid-sk",
        RESOURCE_ISS,
        AS_ISS,
        "aauth:sub@agent.example",
        &other_jkt,
        "documents:read",
        now,
    );

    let jwks = StaticJwks::new()
        .with(AP_ISS, jwk_set(&[&ap_key]))
        .with(RESOURCE_ISS, jwk_set(&[&resource_key]))
        .with(AS_ISS, jwk_set(&[&as_key]));
    let verifier = TokenVerifier::new(jwks, SystemTimeSource);
    let trust = TrustRegistry::from_entries([(PS_ISS.to_string(), TrustBasisRecord::PreEstablished)]).unwrap();
    let req = AsTokenRequest {
        resource_token,
        agent_token,
        subagent_token: Some(subagent_token),
        upstream_token: None,
    };

    let err = verify_request(&verifier, &trust, AS_ISS, PS_ISS, &req)
        .await
        .unwrap_err();
    assert!(matches!(
        err,
        RequestVerificationError::ResourceTokenSubagentKeyMismatch
    ));
}

#[tokio::test]
async fn verify_request_rejects_upstream_token_wrong_audience_as_upstream_token_error() {
    // `TokenVerifier::verify_auth` itself enforces `aud == expected_aud`
    // (see `trogon_aauth_verify::token::TokenVerifier::decode_with_jwks`),
    // so a mismatched upstream `aud` never reaches `verify.rs`'s own
    // `UpstreamAudienceBindingMismatch` check -- it surfaces as the
    // upstream token failing verification outright.
    let now = now_unix();
    let as_key = key_fixture("as-kid-ub");
    let ap_key = key_fixture("ap-kid-ub");
    let resource_key = key_fixture("resource-kid-ub");
    let agent_key = key_fixture("agent-key-ub");
    let upstream_issuer_key = key_fixture("upstream-as-kid-ub");

    let mut trust = TrustRegistry::from_entries([(PS_ISS.to_string(), TrustBasisRecord::PreEstablished)]).unwrap();
    trust.trust(
        crate::trust::PsIssuer::new("https://upstream-as-ub.example").unwrap(),
        TrustBasisRecord::PreEstablished,
    );

    let agent_token = mint_agent_jwt(
        &ap_key,
        "ap-kid-ub",
        AP_ISS,
        "aauth:intermediary@agent.example",
        &agent_key.jwk_json,
        None,
        now,
    );
    let agent_jkt = trogon_aauth_verify::jwk_thumbprint(&agent_key.jwk_json).unwrap();
    let resource_token = mint_resource_jwt(
        &resource_key,
        "resource-kid-ub",
        RESOURCE_ISS,
        AS_ISS,
        "aauth:intermediary@agent.example",
        &agent_jkt,
        "documents:read",
        now,
    );

    // Upstream token's aud does NOT match the intermediary agent_token's iss
    // (AP_ISS).
    let upstream_token = mint_auth_jwt_raw(
        &upstream_issuer_key,
        "upstream-as-kid-ub",
        "https://upstream-as-ub.example",
        "https://some-other-audience.example",
        "user:alice",
        "aauth:root@agent.example",
        "root-jkt",
        &serde_json::json!({"kty": "EC"}),
        "documents:read",
        None,
        now,
    );

    let jwks = StaticJwks::new()
        .with(AP_ISS, jwk_set(&[&ap_key]))
        .with(RESOURCE_ISS, jwk_set(&[&resource_key]))
        .with(AS_ISS, jwk_set(&[&as_key]))
        .with("https://upstream-as-ub.example", jwk_set(&[&upstream_issuer_key]));
    let verifier = TokenVerifier::new(jwks, SystemTimeSource);
    let req = AsTokenRequest {
        resource_token,
        agent_token,
        subagent_token: None,
        upstream_token: Some(upstream_token),
    };

    let err = verify_request(&verifier, &trust, AS_ISS, PS_ISS, &req)
        .await
        .unwrap_err();
    assert!(matches!(err, RequestVerificationError::UpstreamToken(_)));
}

#[tokio::test]
async fn verify_request_rejects_agent_identifier_binding_mismatch() {
    let now = now_unix();
    let as_key = key_fixture("as-kid");
    let ap_key = key_fixture("ap-kid");
    let resource_key = key_fixture("resource-kid");
    let agent_key = key_fixture("agent-key");

    let agent_token = mint_agent_jwt(
        &ap_key,
        "ap-kid",
        AP_ISS,
        "aauth:asst@agent.example",
        &agent_key.jwk_json,
        None,
        now,
    );
    // resource_token carries the presenting agent's key thumbprint but names
    // a different agent identifier: key match alone must not verify.
    let agent_jkt = trogon_aauth_verify::jwk_thumbprint(&agent_key.jwk_json).unwrap();
    let resource_token = mint_resource_jwt(
        &resource_key,
        "resource-kid",
        RESOURCE_ISS,
        AS_ISS,
        "aauth:someone-else@agent.example",
        &agent_jkt,
        "documents:read",
        now,
    );

    let jwks = StaticJwks::new()
        .with(AP_ISS, jwk_set(&[&ap_key]))
        .with(RESOURCE_ISS, jwk_set(&[&resource_key]))
        .with(AS_ISS, jwk_set(&[&as_key]));
    let verifier = TokenVerifier::new(jwks, SystemTimeSource);
    let trust = TrustRegistry::from_entries([(PS_ISS.to_string(), TrustBasisRecord::PreEstablished)]).unwrap();
    let req = AsTokenRequest {
        resource_token,
        agent_token,
        subagent_token: None,
        upstream_token: None,
    };

    let err = verify_request(&verifier, &trust, AS_ISS, PS_ISS, &req)
        .await
        .unwrap_err();
    assert!(matches!(
        err,
        RequestVerificationError::ResourceTokenAgentIdentifierMismatch
    ));
}
