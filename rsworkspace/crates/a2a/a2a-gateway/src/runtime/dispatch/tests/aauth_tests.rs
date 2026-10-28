use std::sync::Arc;

use jsonwebtoken::{Algorithm, EncodingKey};
use p256::ecdsa::SigningKey;
use p256::pkcs8::EncodePrivateKey;
use rand_core::OsRng;
use serde_json::{Value, json};
use trogon_identity_types::aauth::headers;

use crate::aauth::{
    AAuthConfig, AAuthIngress, AAuthMode, ChallengeKid, GatewayAAuthIngress, GatewayJwksResolver, LeewaySecs,
    NonNegativeSecs, PersonServerAudience, ResourceIssuer, StaticJwks,
};
use crate::policy::spicedb_tier1::Tier1AuthorizeOutcome;

use super::fixture::{DispatchFixture, RecordingTier1, TestResult, assert_empty, receive, request};

fn ingress(mode: AAuthMode) -> TestResult<GatewayAAuthIngress> {
    let key = SigningKey::random(&mut OsRng).to_pkcs8_pem(p256::pkcs8::LineEnding::LF)?;
    Ok(Arc::new(AAuthIngress::new_in_memory(AAuthConfig {
        mode,
        jwks: GatewayJwksResolver::Static(StaticJwks::new()),
        resource_iss: ResourceIssuer::new("https://resource.test")?,
        person_server_aud: PersonServerAudience::new("https://person.test")?,
        leeway_secs: LeewaySecs::new(30),
        challenge_alg: Algorithm::ES256,
        challenge_key: EncodingKey::from_ec_pem(key.as_bytes())?,
        challenge_kid: ChallengeKid::new("gateway")?,
        challenge_ttl_secs: NonNegativeSecs::new(60)?,
        max_skew_secs: NonNegativeSecs::new(60)?,
    })))
}

#[tokio::test]
async fn enforce_authentication_denies_before_authorization_without_unbound_challenge() -> TestResult {
    let mut fixture = DispatchFixture::new().await?;
    fixture.aauth = Some(ingress(AAuthMode::Enforce)?);
    let gate = Arc::new(RecordingTier1::new(Tier1AuthorizeOutcome::Allowed { zed_token: None }));
    fixture.tier1 = gate.clone();
    fixture.dispatch(request("message.send", json!({}))).await;
    let reply = receive(&mut fixture.replies).await?;
    let body: Value = serde_json::from_slice(&reply.payload)?;
    assert_eq!(body["id"], "request-7");
    assert_eq!(body["error"]["code"], -32118);
    let response_headers = reply.headers.expect("authentication denial headers");
    assert!(response_headers.get(headers::REQUIREMENT).is_none());
    assert_eq!(
        response_headers
            .get("Jsonrpc-Error-Code")
            .map(async_nats::HeaderValue::as_str),
        Some("-32118")
    );
    assert!(gate.calls.lock().expect("authorization calls").is_empty());
    assert_empty(&fixture.client, &mut fixture.agents, "a2a.v1.agents.barrier").await?;
    let audit = fixture.audit("err").await?;
    assert_eq!(audit["code"], -32118);
    assert_eq!(audit["caller_id"], "alice");
    assert_eq!(audit["rules_fired"], json!(["gateway.aauth.denied.pop"]));
    Ok(())
}

#[tokio::test]
async fn shadow_and_disabled_authentication_preserve_delivery_and_audit_mode() -> TestResult {
    let mut fixture = DispatchFixture::new().await?;
    for (mode, rule) in [
        (AAuthMode::Shadow, "gateway.aauth.shadow"),
        (AAuthMode::Off, "gateway.aauth.layer_disabled"),
    ] {
        fixture.aauth = Some(ingress(mode)?);
        fixture.dispatch(request("message.send", json!({}))).await;
        let forwarded = receive(&mut fixture.agents).await?;
        assert!(
            forwarded
                .headers
                .expect("forwarded headers")
                .get(headers::ACCESS)
                .is_none()
        );
        assert_eq!(fixture.audit("ok").await?["rules_fired"][3], rule);
    }
    Ok(())
}
