//! Posta-style "supply-chain demo" in test form: resource ⇒ challenge ⇒
//! Person Server ⇒ agent retries with `aa-auth+jwt` and is admitted.
//!
//! Drives the full handshake end-to-end against PersonCore + the verifier
//! components the gateway uses in production.

use std::sync::Arc;
use std::sync::atomic::{AtomicI64, Ordering};

use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use jsonwebtoken::jwk::{
    AlgorithmParameters, CommonParameters, EllipticCurve, EllipticCurveKeyParameters,
    EllipticCurveKeyType, Jwk, JwkSet, PublicKeyUse,
};
use jsonwebtoken::{Algorithm, EncodingKey};
use p256::ecdsa::SigningKey;
use p256::pkcs8::EncodePrivateKey;
use rand_core::OsRng;
use trogon_aauth_person::{
    AllowConfiguredScopes, BootstrapRequest as PsBootstrap, InMemoryStore, PersonCore,
    TokenRequest as PsToken,
};
use trogon_aauth_sdk::{
    AgentKeypair, ChallengeOutcome, NatsRequestSigner, parse_challenge_headers,
};
use trogon_aauth_verify::challenge::ResourceChallenge;
use trogon_aauth_verify::replay::InMemoryReplayStore;
use trogon_aauth_verify::{
    ChallengeMinter, NatsHeaders, NatsPopVerifier, NatsRequest, StaticJwks, TokenVerifier,
    time_source::TimeSource,
};
use trogon_identity_types::aauth::headers as aauth_headers;

#[derive(Clone)]
struct FixedClock(Arc<AtomicI64>);
impl TimeSource for FixedClock {
    fn now(&self) -> i64 {
        self.0.load(Ordering::SeqCst)
    }
}

fn p256_jwk(verifying: &p256::ecdsa::VerifyingKey, kid: &str) -> Jwk {
    let p = verifying.to_encoded_point(false);
    Jwk {
        common: CommonParameters {
            public_key_use: Some(PublicKeyUse::Signature),
            key_id: Some(kid.into()),
            ..Default::default()
        },
        algorithm: AlgorithmParameters::EllipticCurve(EllipticCurveKeyParameters {
            key_type: EllipticCurveKeyType::EC,
            curve: EllipticCurve::P256,
            x: URL_SAFE_NO_PAD.encode(p.x().unwrap()),
            y: URL_SAFE_NO_PAD.encode(p.y().unwrap()),
        }),
    }
}

#[tokio::test(flavor = "current_thread")]
async fn agent_recovers_from_resource_challenge() {
    let clock_value = Arc::new(AtomicI64::new(1_700_000_000));
    let clock = FixedClock(clock_value.clone());

    // PS, resource keys.
    let ps_signing = SigningKey::random(&mut OsRng);
    let ps_jwk = p256_jwk(ps_signing.verifying_key(), "ps-key");
    let ps_set = JwkSet { keys: vec![ps_jwk] };
    let ps_pem = ps_signing.to_pkcs8_pem(p256::pkcs8::LineEnding::LF).unwrap().to_string();
    let ps_enc = EncodingKey::from_ec_pem(ps_pem.as_bytes()).unwrap();

    let res_signing = SigningKey::random(&mut OsRng);
    let res_jwk = p256_jwk(res_signing.verifying_key(), "res-key");
    let res_set = JwkSet { keys: vec![res_jwk] };
    let res_pem = res_signing.to_pkcs8_pem(p256::pkcs8::LineEnding::LF).unwrap().to_string();
    let res_enc = EncodingKey::from_ec_pem(res_pem.as_bytes()).unwrap();

    let ps_iss = "https://ps.demo".to_string();
    let res_iss = "https://res.demo".to_string();

    let jwks = StaticJwks::new()
        .with(ps_iss.clone(), ps_set)
        .with(res_iss.clone(), res_set);

    // PersonCore.
    let store = Arc::new(InMemoryStore::default());
    let policy = Arc::new(
        AllowConfiguredScopes::new(300).with("user-alice", "agent-supplychain", &res_iss, "read:po"),
    );
    let core = Arc::new(PersonCore {
        iss: ps_iss.clone(),
        signing_kid: "ps-key".into(),
        signing_alg: Algorithm::ES256,
        signing_key: ps_enc,
        agent_jwt_ttl_secs: 3600,
        auth_jwt_ttl_secs: 600,
        store,
        policy,
        token_verifier: TokenVerifier::new(jwks.clone(), clock.clone()).with_leeway(60),
        clock: clock.clone(),
    });

    // Resource-side machinery (a gateway would do this on deny).
    let challenge = ChallengeMinter::new(res_enc, Algorithm::ES256, clock.clone());
    let pop_verifier = NatsPopVerifier::new(jwks.clone(), clock.clone(), InMemoryReplayStore::default());
    let auth_verifier = TokenVerifier::new(jwks, clock.clone()).with_leeway(60);

    // 1) Agent generates keypair, bootstraps.
    let agent = AgentKeypair::generate().unwrap();
    let boot = core
        .bootstrap(PsBootstrap {
            cnf_jwk: agent.public_jwk().clone(),
            principal: Some("user-alice".into()),
            agent_id_hint: Some("agent-supplychain".into()),
        })
        .await
        .expect("bootstrap");

    // 2) Agent signs an outbound request to the resource.
    let subject = "mcp.tool.list_purchase_orders";
    let payload = br#"{"order_id":"PO-42"}"#;
    let signer = NatsRequestSigner::new(&agent, &boot.agent_jwt);
    let signed = signer.sign(subject, Some("_INBOX.A"), payload, clock.now(), "n-1");
    let headers: Vec<(String, String)> = signed.into_pairs();

    // 3) Resource verifies PoP (agent identity OK), but the request lacks an
    //    `aa-auth+jwt` → denied with a resource challenge.
    let pop = pop_verifier
        .verify(&NatsRequest {
            subject,
            reply: Some("_INBOX.A"),
            payload,
            headers: NatsHeaders::new(&headers),
        })
        .await
        .expect("agent identity verifies");

    let challenge_jwt = challenge
        .mint(&ResourceChallenge {
            iss: &res_iss,
            aud_ps: &ps_iss,
            agent: &pop.claims.sub,
            agent_jkt: &pop.jkt,
            scope: "read:po",
            ttl_secs: 60,
            kid: "res-key",
            jti: "rch-demo-1",
            mission: None,
        })
        .unwrap();
    let requirement_header = format!("requirement=auth-token; resource-token=\"{challenge_jwt}\"");

    // 4) Agent parses the challenge and exchanges it for an auth_jwt.
    let outcome = parse_challenge_headers(Some(&requirement_header));
    let resource_jwt = match outcome {
        ChallengeOutcome::NeedsExchange { resource_jwt } => resource_jwt,
        other => panic!("expected NeedsExchange, got {other:?}"),
    };
    let exchanged = core
        .exchange(PsToken {
            resource_jwt,
            agent_jwt: boot.agent_jwt.clone(),
            principal: Some("user-alice".into()),
        })
        .await
        .expect("exchange");
    assert_eq!(exchanged.scope, "read:po");

    // 5) Agent retries the same request, now attaching the auth_jwt.
    //    Resource verifies the auth_jwt (signature, audience = res_iss,
    //    agent_jkt matches the agent we just verified).
    let auth = auth_verifier
        .verify_auth(&exchanged.auth_jwt, &res_iss)
        .await
        .expect("auth verifies");
    assert_eq!(auth.claims.agent_jkt, agent.jkt());
    assert_eq!(auth.claims.scope, "read:po");
    assert_eq!(auth.claims.principal.as_deref(), Some("user-alice"));

    // Sanity: the challenge header name is what the SDK and gateway agree on.
    assert_eq!(aauth_headers::REQUIREMENT, "AAuth-Requirement");
}
