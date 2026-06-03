//! End-to-end: agent SDK keypair → PersonCore bootstrap → resource challenge →
//! exchange → NATS PoP sign → verifier round-trip.

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
use trogon_aauth_verify::replay::InMemoryReplayStore;
use trogon_aauth_verify::{NatsHeaders, NatsPopVerifier, NatsRequest, StaticJwks, TokenVerifier, time_source::TimeSource};

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
async fn sdk_drives_full_handshake() {
    let clock_value = Arc::new(AtomicI64::new(1_700_000_000));
    let clock = FixedClock(clock_value.clone());

    // PS key.
    let ps_signing = SigningKey::random(&mut OsRng);
    let ps_jwk = p256_jwk(ps_signing.verifying_key(), "ps-key-1");
    let ps_set = JwkSet { keys: vec![ps_jwk] };
    let ps_pem = ps_signing.to_pkcs8_pem(p256::pkcs8::LineEnding::LF).unwrap().to_string();
    let ps_enc = EncodingKey::from_ec_pem(ps_pem.as_bytes()).unwrap();

    // Resource key.
    let res_signing = SigningKey::random(&mut OsRng);
    let res_jwk = p256_jwk(res_signing.verifying_key(), "res-key-1");
    let res_set = JwkSet { keys: vec![res_jwk] };
    let res_pem = res_signing.to_pkcs8_pem(p256::pkcs8::LineEnding::LF).unwrap().to_string();
    let res_enc = EncodingKey::from_ec_pem(res_pem.as_bytes()).unwrap();

    // Agent — via the SDK.
    let agent = AgentKeypair::generate().unwrap();

    let ps_iss = "https://ps.test".to_string();
    let res_iss = "https://res.test".to_string();

    let jwks = StaticJwks::new()
        .with(ps_iss.clone(), ps_set.clone())
        .with(res_iss.clone(), res_set);

    let store = Arc::new(InMemoryStore::default());
    let policy = Arc::new(
        AllowConfiguredScopes::new(300).with("user-a", "agent-1", &res_iss, "read:tools"),
    );

    let core = Arc::new(PersonCore {
        iss: ps_iss.clone(),
        signing_kid: "ps-key-1".into(),
        signing_alg: Algorithm::ES256,
        signing_key: ps_enc,
        agent_jwt_ttl_secs: 3600,
        auth_jwt_ttl_secs: 600,
        store,
        policy,
        token_verifier: TokenVerifier::new(jwks.clone(), clock.clone()).with_leeway(60),
        clock: clock.clone(),
    });

    // 1) SDK bootstrap (we go through PersonCore directly; the HTTP/NATS clients
    //    would just wrap this).
    let boot = core
        .bootstrap(PsBootstrap {
            cnf_jwk: agent.public_jwk().clone(),
            principal: Some("user-a".into()),
            agent_id_hint: Some("agent-1".into()),
        })
        .await
        .expect("bootstrap");

    assert_eq!(boot.agent_id, "agent-1");

    // 2) Resource mints a challenge bound to the agent's jkt.
    let now = clock.now();
    let res_claims = serde_json::json!({
        "iss": res_iss,
        "aud": ps_iss,
        "jti": "rch-1",
        "iat": now - 5,
        "exp": now + 60,
        "dwk": "aauth-resource.json",
        "agent": "agent-1",
        "agent_jkt": agent.jkt(),
        "scope": "read:tools",
    });
    let mut hdr = jsonwebtoken::Header::new(Algorithm::ES256);
    hdr.typ = Some(trogon_identity_types::aauth::TYP_RESOURCE.into());
    hdr.kid = Some("res-key-1".into());
    let resource_jwt = jsonwebtoken::encode(&hdr, &res_claims, &res_enc).unwrap();

    // SDK parses the resource header → resource_jwt.
    let header_value = format!("requirement=auth-token; resource-token=\"{resource_jwt}\"");
    let outcome = parse_challenge_headers(Some(&header_value));
    let extracted = match outcome {
        ChallengeOutcome::NeedsExchange { resource_jwt } => resource_jwt,
        _ => panic!("expected NeedsExchange"),
    };
    assert_eq!(extracted, resource_jwt);

    // 3) SDK exchanges via PersonCore.
    let exchanged = core
        .exchange(PsToken {
            resource_jwt: extracted,
            agent_jwt: boot.agent_jwt.clone(),
            principal: Some("user-a".into()),
        })
        .await
        .expect("exchange");
    assert_eq!(exchanged.scope, "read:tools");

    // 4) SDK signs an outbound NATS request and the verifier accepts it.
    let signer = NatsRequestSigner::new(&agent, &boot.agent_jwt);
    let payload = b"{\"tool\":\"echo\"}";
    let signed = signer.sign("mcp.tool.echo", Some("_INBOX.1"), payload, now, "nonce-1");
    let header_pairs: Vec<(String, String)> = signed.into_pairs();

    let verifier = NatsPopVerifier::new(jwks, clock.clone(), InMemoryReplayStore::default());
    let req = NatsRequest {
        subject: "mcp.tool.echo",
        reply: Some("_INBOX.1"),
        payload,
        headers: NatsHeaders::new(&header_pairs),
    };
    let verified = verifier.verify(&req).await.expect("PoP verify");
    assert_eq!(verified.jkt, agent.jkt());
}
