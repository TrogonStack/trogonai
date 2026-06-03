//! End-to-end: bootstrap an agent against the PersonCore, then exchange a
//! resource-minted challenge for an `aa-auth+jwt`.

use std::sync::Arc;
use std::sync::atomic::{AtomicI64, Ordering};

use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use jsonwebtoken::jwk::{
    AlgorithmParameters, CommonParameters, EllipticCurve, EllipticCurveKeyParameters, EllipticCurveKeyType, Jwk, JwkSet,
    PublicKeyUse,
};
use jsonwebtoken::{Algorithm, EncodingKey};
use p256::ecdsa::SigningKey;
use p256::pkcs8::EncodePrivateKey;
use rand_core::OsRng;
use trogon_aauth_person::{
    AllowConfiguredScopes, BootstrapRequest, InMemoryStore, PersonCore, TokenRequest,
};
use trogon_aauth_verify::{StaticJwks, TokenVerifier, time_source::TimeSource};

#[derive(Clone)]
struct FixedClock(Arc<AtomicI64>);
impl TimeSource for FixedClock {
    fn now(&self) -> i64 {
        self.0.load(Ordering::SeqCst)
    }
}

fn p256_to_jwk(verifying: &p256::ecdsa::VerifyingKey, kid: &str) -> (Jwk, serde_json::Value) {
    let point = verifying.to_encoded_point(false);
    let x = URL_SAFE_NO_PAD.encode(point.x().expect("x"));
    let y = URL_SAFE_NO_PAD.encode(point.y().expect("y"));
    let jwk = Jwk {
        common: CommonParameters {
            public_key_use: Some(PublicKeyUse::Signature),
            key_id: Some(kid.into()),
            ..Default::default()
        },
        algorithm: AlgorithmParameters::EllipticCurve(EllipticCurveKeyParameters {
            key_type: EllipticCurveKeyType::EC,
            curve: EllipticCurve::P256,
            x: x.clone(),
            y: y.clone(),
        }),
    };
    let val = serde_json::json!({"kty": "EC", "crv": "P-256", "x": x, "y": y, "kid": kid});
    (jwk, val)
}

#[tokio::test(flavor = "current_thread")]
async fn bootstrap_then_exchange_succeeds() {
    let clock_value = Arc::new(AtomicI64::new(1_700_000_000));
    let clock = FixedClock(clock_value.clone());

    // PS signing key.
    let ps_signing = SigningKey::random(&mut OsRng);
    let ps_verifying = ps_signing.verifying_key();
    let (ps_jwk, _) = p256_to_jwk(ps_verifying, "ps-key-1");
    let ps_set = JwkSet { keys: vec![ps_jwk.clone()] };
    let ps_pem = ps_signing.to_pkcs8_pem(p256::pkcs8::LineEnding::LF).unwrap().to_string();
    let ps_enc = EncodingKey::from_ec_pem(ps_pem.as_bytes()).unwrap();

    // Resource signing key (mints aa-resource+jwt).
    let res_signing = SigningKey::random(&mut OsRng);
    let res_verifying = res_signing.verifying_key();
    let (res_jwk, _) = p256_to_jwk(res_verifying, "res-key-1");
    let res_set = JwkSet { keys: vec![res_jwk.clone()] };
    let res_pem = res_signing.to_pkcs8_pem(p256::pkcs8::LineEnding::LF).unwrap().to_string();
    let res_enc = EncodingKey::from_ec_pem(res_pem.as_bytes()).unwrap();

    // Agent keypair — its public JWK goes into cnf.jwk during bootstrap.
    let agent_signing = SigningKey::random(&mut OsRng);
    let (_agent_jwk_obj, agent_jwk_val) = p256_to_jwk(agent_signing.verifying_key(), "agent-key-1");

    let ps_iss = "https://ps.test".to_string();
    let res_iss = "https://res.test".to_string();

    let jwks = StaticJwks::new()
        .with(ps_iss.clone(), ps_set.clone())
        .with(res_iss.clone(), res_set);

    let store = Arc::new(InMemoryStore::default());
    let policy = Arc::new(AllowConfiguredScopes::new(300).with("user-a", "agent-1", &res_iss, "read:tools"));

    let core = Arc::new(PersonCore {
        iss: ps_iss.clone(),
        signing_kid: "ps-key-1".into(),
        signing_alg: Algorithm::ES256,
        signing_key: ps_enc,
        agent_jwt_ttl_secs: 3600,
        auth_jwt_ttl_secs: 600,
        store,
        policy,
        token_verifier: TokenVerifier::new(jwks, clock.clone()).with_leeway(60),
        clock: clock.clone(),
    });

    // 1) Bootstrap.
    let boot = core
        .bootstrap(BootstrapRequest {
            cnf_jwk: agent_jwk_val.clone(),
            principal: Some("user-a".into()),
            agent_id_hint: Some("agent-1".into()),
        })
        .await
        .expect("bootstrap");

    assert_eq!(boot.agent_id, "agent-1");
    assert!(boot.agent_jwt.split('.').count() == 3);

    // 2) Mint a resource challenge bound to the agent's jkt.
    let agent_jkt = trogon_aauth_verify::jwk_thumbprint(&agent_jwk_val).unwrap();
    let now = clock.now();
    let res_claims = serde_json::json!({
        "iss": res_iss,
        "aud": ps_iss,
        "jti": "rch-1",
        "iat": now - 5,
        "exp": now + 60,
        "dwk": "aauth-resource.json",
        "agent": "agent-1",
        "agent_jkt": agent_jkt,
        "scope": "read:tools",
    });
    let mut hdr = jsonwebtoken::Header::new(Algorithm::ES256);
    hdr.typ = Some(trogon_identity_types::aauth::TYP_RESOURCE.into());
    hdr.kid = Some("res-key-1".into());
    let resource_jwt = jsonwebtoken::encode(&hdr, &res_claims, &res_enc).unwrap();

    // 3) Exchange.
    let exchanged = core
        .exchange(TokenRequest {
            resource_jwt,
            agent_jwt: boot.agent_jwt.clone(),
            principal: Some("user-a".into()),
        })
        .await
        .expect("exchange");

    assert_eq!(exchanged.scope, "read:tools");
    assert!(exchanged.auth_jwt.split('.').count() == 3);
}
