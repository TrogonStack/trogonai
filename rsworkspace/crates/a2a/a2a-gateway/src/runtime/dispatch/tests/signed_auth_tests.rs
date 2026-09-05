use std::sync::Arc;

use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use jsonwebtoken::jwk::JwkSet;
use jsonwebtoken::{Algorithm, EncodingKey, Header, encode};
use p256::ecdsa::SigningKey;
use p256::pkcs8::EncodePrivateKey;
use rand_core::OsRng;
use serde_json::{Value, json};
use trogon_aauth_sdk::AgentSigner;
use trogon_identity_types::aauth::{Cnf, DWK_AGENT, TYP_AGENT, TYP_AUTH, headers};

use crate::aauth::{
    AAuthConfig, AAuthIngress, AAuthMode, ChallengeKid, GatewayAAuthIngress, GatewayJwksResolver, LeewaySecs,
    NonNegativeSecs, PersonServerAudience, ResourceIssuer, StaticJwks,
};
use crate::policy::spicedb_tier1::Tier1AuthorizeOutcome;

use super::fixture::{DispatchFixture, RecordingTier1, TestResult, assert_empty, receive, request};

struct SignedRequestFixture {
    ingress: GatewayAAuthIngress,
    signer: AgentSigner,
    auth: String,
}

fn public_jwk(key: &SigningKey) -> Value {
    let point = key.verifying_key().to_encoded_point(false);
    json!({
        "kty": "EC", "crv": "P-256",
        "x": URL_SAFE_NO_PAD.encode(point.x().expect("P-256 x")),
        "y": URL_SAFE_NO_PAD.encode(point.y().expect("P-256 y"))
    })
}

fn signed_fixture() -> TestResult<SignedRequestFixture> {
    let provider = SigningKey::random(&mut OsRng);
    let agent = SigningKey::random(&mut OsRng);
    let pem = provider.to_pkcs8_pem(p256::pkcs8::LineEnding::LF)?;
    let key = EncodingKey::from_ec_pem(pem.as_bytes())?;
    let now = time::OffsetDateTime::now_utc().unix_timestamp();
    let agent_jwk = public_jwk(&agent);
    let mut header = Header::new(Algorithm::ES256);
    header.kid = Some("provider".into());
    header.typ = Some(TYP_AGENT.into());
    let agent_token = encode(
        &header,
        &json!({
            "iss": "https://provider.test", "sub": "agent-1", "jti": "agent-token",
            "iat": now - 5, "exp": now + 600, "dwk": DWK_AGENT,
            "cnf": Cnf::public(agent_jwk.clone()).expect("public agent key")
        }),
        &key,
    )?;
    let signer = AgentSigner::new(agent, agent_token).expect("agent signer");
    header.typ = Some(TYP_AUTH.into());
    let auth = encode(
        &header,
        &json!({
            "iss": "https://provider.test", "sub": "agent-1", "aud": "https://resource.test",
            "jti": "grant-7", "iat": now - 5, "exp": now + 600,
            "agent": "agent-1", "agent_jkt": signer.jkt(), "scope": "message.send", "principal": "bob"
        }),
        &key,
    )?;
    let mut provider_jwk = public_jwk(&provider);
    provider_jwk["kid"] = json!("provider");
    provider_jwk["use"] = json!("sig");
    let jwks = StaticJwks::new().with(
        "https://provider.test",
        JwkSet {
            keys: vec![serde_json::from_value(provider_jwk)?],
        },
    );
    let ingress = Arc::new(AAuthIngress::new_in_memory(AAuthConfig {
        mode: AAuthMode::Enforce,
        jwks: GatewayJwksResolver::Static(jwks),
        resource_iss: ResourceIssuer::new("https://resource.test")?,
        person_server_aud: PersonServerAudience::new("https://person.test")?,
        leeway_secs: LeewaySecs::new(30),
        challenge_alg: Algorithm::ES256,
        challenge_key: key,
        challenge_kid: ChallengeKid::new("gateway")?,
        challenge_ttl_secs: NonNegativeSecs::new(60)?,
        max_skew_secs: NonNegativeSecs::new(60)?,
    }));
    Ok(SignedRequestFixture { ingress, signer, auth })
}

fn sign(signer: &AgentSigner) -> async_nats::Message {
    let mut message = request("message.send", json!({"id": "task-1"}));
    for (name, value) in signer
        .sign_nats_request_now(
            message.subject.as_str(),
            message.reply.as_ref().map(async_nats::Subject::as_str),
            &message.payload,
        )
        .into_pairs()
    {
        message
            .headers
            .as_mut()
            .expect("request headers")
            .insert(name.as_str(), value.as_str());
    }
    message
}

#[tokio::test]
async fn verified_grant_controls_authorization_audit_and_forwarded_access_identity() -> TestResult {
    let mut fixture = DispatchFixture::new().await?;
    let signed = signed_fixture()?;
    fixture.aauth = Some(signed.ingress);
    let tier1 = Arc::new(RecordingTier1::new(Tier1AuthorizeOutcome::Allowed { zed_token: None }));
    fixture.tier1 = tier1.clone();
    let signer = signed.signer.with_auth_token(signed.auth);
    fixture.dispatch(sign(&signer)).await;
    let forwarded = receive(&mut fixture.agents).await?;
    assert_eq!(
        forwarded
            .headers
            .expect("forwarded headers")
            .get(headers::ACCESS)
            .map(async_nats::HeaderValue::as_str),
        Some("grant-7")
    );
    {
        let calls = tier1.calls.lock().expect("authorization calls");
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].session.sub(), "bob");
        assert_eq!(
            calls[0].principal.spicedb_subject().expect("principal").as_str(),
            "user/bob"
        );
    }
    let audit = fixture.audit("ok").await?;
    assert_eq!(audit["caller_id"], "user/bob");
    assert_eq!(audit["caller_source"], "aauth");
    assert_eq!(audit["rules_fired"][3], "gateway.aauth.enforced_allow");
    Ok(())
}

#[tokio::test]
async fn invalid_grant_returns_a_bound_challenge_before_any_authorization() -> TestResult {
    let mut fixture = DispatchFixture::new().await?;
    let signed = signed_fixture()?;
    fixture.aauth = Some(signed.ingress);
    let tier1 = Arc::new(RecordingTier1::new(Tier1AuthorizeOutcome::Allowed { zed_token: None }));
    fixture.tier1 = tier1.clone();
    fixture
        .dispatch(sign(&signed.signer.with_auth_token("invalid-grant")))
        .await;
    let reply = receive(&mut fixture.replies).await?;
    let response_headers = reply.headers.expect("challenge headers");
    let requirement = response_headers.get(headers::REQUIREMENT).expect("bound challenge");
    assert!(requirement.as_str().starts_with("requirement=auth-token"));
    let body: Value = serde_json::from_slice(&reply.payload)?;
    assert_eq!(body["id"], "request-7");
    assert_eq!(body["error"]["code"], -32118);
    assert!(tier1.calls.lock().expect("authorization calls").is_empty());
    assert_empty(&fixture.client, &mut fixture.agents, "a2a.v1.agents.barrier").await?;
    assert_eq!(
        fixture.audit("err").await?["rules_fired"],
        json!(["gateway.aauth.denied.auth"])
    );
    Ok(())
}
