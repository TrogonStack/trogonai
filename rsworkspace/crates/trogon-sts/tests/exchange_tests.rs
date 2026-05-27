mod support;

use jsonwebtoken::{Algorithm, DecodingKey, Validation, decode};
use support::*;
use trogon_identity_types::ActChainEntry;
use trogon_sts::DEFAULT_MESH_ISSUER;

#[tokio::test]
async fn bootstrap_to_mesh_exchange_produces_one_entry_act_chain() {
    let keys = shared_test_keys();
    let record = sample_registry_record();
    let service = build_service(keys, record);
    let subject = mint_bootstrap_token(keys, bootstrap_claims(keys));
    let request = sample_exchange_request(&subject);

    let response = service.handle(request, None).await.expect("exchange");
    let payload = decode_payload(&response.access_token);
    let chain = payload
        .get("act_chain")
        .and_then(|v| serde_json::from_value::<Vec<ActChainEntry>>(v.clone()).ok())
        .expect("act_chain");
    assert_eq!(chain.len(), 1);
    assert_eq!(chain[0].sub, "user:alice");
    assert_eq!(chain[0].agent_id.as_deref(), Some("acme/oncall-agent"));
    assert_eq!(payload.get("iss").and_then(|v| v.as_str()), Some(DEFAULT_MESH_ISSUER));
    assert_eq!(
        payload.get("aud").and_then(|v| v.as_str()),
        Some("urn:trogon:mcp:backend:acme:github")
    );

    let mesh_jwk = keys.mesh_jwks.keys.first().expect("mesh jwk");
    let dec = DecodingKey::from_jwk(mesh_jwk).expect("dec key");
    let mut validation = Validation::new(Algorithm::RS256);
    validation.set_issuer(&[DEFAULT_MESH_ISSUER]);
    validation.validate_aud = false;
    decode::<serde_json::Value>(&response.access_token, &dec, &validation).expect("mesh jwt verifies");
}

#[tokio::test]
async fn depth_nine_inbound_chain_rejected() {
    let keys = shared_test_keys();
    let mut record = sample_registry_record();
    record.mesh_token_ttl_s = Some(120);
    let service = build_service(keys, record);

    let chain: Vec<ActChainEntry> = (0..9)
        .map(|i| ActChainEntry {
            sub: format!("hop-{i}"),
            agent_id: Some(format!("agent-{i}")),
            wkl: Some(format!("spiffe://acme/sa/{i}")),
            iat: i as i64,
        })
        .collect();
    let mut claims = bootstrap_claims(keys);
    claims["act_chain"] = serde_json::to_value(chain).unwrap();
    let subject = mint_bootstrap_token(keys, claims);
    let err = service
        .handle(sample_exchange_request(&subject), None)
        .await
        .expect_err("depth exceeded");
    assert_eq!(err.error, "act_chain_depth_exceeded");
}

#[tokio::test]
async fn agent_wkl_loop_rejected() {
    let keys = shared_test_keys();
    let service = build_service(keys, sample_registry_record());
    let wkl = "spiffe://acme.local/ns/prod/sa/oncall-agent";
    let chain = vec![ActChainEntry {
        sub: "user:alice".into(),
        agent_id: Some("acme/oncall-agent".into()),
        wkl: Some(wkl.into()),
        iat: 1,
    }];
    let mut claims = bootstrap_claims(keys);
    claims["act_chain"] = serde_json::to_value(chain).unwrap();
    let subject = mint_bootstrap_token(keys, claims);
    let err = service
        .handle(sample_exchange_request(&subject), None)
        .await
        .expect_err("loop");
    assert_eq!(err.error, "act_chain_loop_detected");
}

#[tokio::test]
async fn audience_not_allowed_returns_invalid_target() {
    let keys = shared_test_keys();
    let service = build_service(keys, sample_registry_record());
    let subject = mint_bootstrap_token(keys, bootstrap_claims(keys));
    let mut request = sample_exchange_request(&subject);
    request.audience = "urn:trogon:mcp:backend:acme:unknown".into();
    let err = service.handle(request, None).await.expect_err("invalid aud");
    assert_eq!(err.error, "invalid_target");
}

#[tokio::test]
async fn ttl_clamps_to_registry_mesh_token_ttl_s() {
    let keys = shared_test_keys();
    let mut record = sample_registry_record();
    record.mesh_token_ttl_s = Some(60);
    let service = build_service(keys, record);
    let subject = mint_bootstrap_token(keys, bootstrap_claims(keys));
    let response = service
        .handle(sample_exchange_request(&subject), None)
        .await
        .expect("exchange");
    assert_eq!(response.expires_in, 60);
    let payload = decode_payload(&response.access_token);
    let iat = payload.get("iat").and_then(|v| v.as_i64()).expect("iat");
    let exp = payload.get("exp").and_then(|v| v.as_i64()).expect("exp");
    assert_eq!(exp - iat, 60);
}
