//! Integration test: mint an `aa-agent+jwt` with a real P-256 key, sign a NATS
//! envelope, verify end-to-end through `NatsPopVerifier`.

use std::sync::Arc;
use std::sync::atomic::{AtomicI64, Ordering};

use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use jsonwebtoken::jwk::{
    AlgorithmParameters, CommonParameters, EllipticCurve, EllipticCurveKeyParameters, EllipticCurveKeyType, Jwk, JwkSet,
    PublicKeyUse,
};
use jsonwebtoken::{Algorithm, EncodingKey, Header, encode};
use p256::ecdsa::signature::Signer;
use p256::ecdsa::{Signature, SigningKey};
use rand_core::OsRng;
use trogon_aauth_verify::{
    InMemoryReplayStore, NatsPopVerifier, NatsRequest, StaticJwks, SystemTimeSource, TimeSource,
    nats_pop::{NatsHeaders, content_digest_sha256},
};
use trogon_identity_types::aauth::{Cnf, DWK_AGENT, TYP_AGENT, NatsSignatureEnvelope, headers};

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
async fn end_to_end_nats_pop_verifies() {
    // 1) Agent Provider key — signs the `aa-agent+jwt`.
    let ap_signing = SigningKey::random(&mut OsRng);
    let ap_verify = ap_signing.verifying_key();
    let (ap_jwk, _) = p256_to_jwk(ap_verify, "ap-key-1");
    let ap_set = JwkSet { keys: vec![ap_jwk] };

    // 2) Agent's own key — used for PoP. This goes into cnf.jwk.
    let agent_signing = SigningKey::random(&mut OsRng);
    let agent_verify = agent_signing.verifying_key();
    let (_agent_jwk_obj, agent_jwk_val) = p256_to_jwk(agent_verify, "agent-key-1");

    let ap_iss = "https://ap.test".to_string();
    let agent_sub = "aauth:agent-1@test".to_string();

    let clock_value = Arc::new(AtomicI64::new(1_700_000_000));
    let clock = FixedClock(clock_value.clone());

    // Mint the aa-agent+jwt using p256 → ECDSA-SHA256. We can use `jsonwebtoken`'s
    // EncodingKey::from_ec_pem path, but here we use the raw signing key via PKCS#8.
    let pem = pkcs8_pem_from_signing_key(&ap_signing);
    let enc_key = EncodingKey::from_ec_pem(pem.as_bytes()).expect("from pem");
    let mut header = Header::new(Algorithm::ES256);
    header.typ = Some(TYP_AGENT.into());
    header.kid = Some("ap-key-1".into());
    let now = clock.now();
    let claims = serde_json::json!({
        "iss": ap_iss,
        "sub": agent_sub,
        "jti": "agent-jti-1",
        "iat": now - 5,
        "exp": now + 600,
        "dwk": DWK_AGENT,
        "cnf": Cnf { jwk: agent_jwk_val.clone() },
    });
    let agent_jwt = encode(&header, &claims, &enc_key).expect("encode agent jwt");

    // 3) Sign a NATS request with the agent's key.
    let subject = "trogon.demo.tool.read";
    let reply = "_INBOX.x.1";
    let payload = b"{\"params\":{\"k\":1}}".to_vec();
    let digest = content_digest_sha256(&payload);
    let created = clock.now();
    let nonce = "n-abc-1";
    let sig_input = "(\"@subject\" \"@reply\" \"content-digest\" \"aauth-token\" \"aauth-sig-created\" \"aauth-sig-nonce\")";

    // Compute jkt of agent key for the canonical-base keyid.
    let jkt = trogon_aauth_verify::jwk_thumbprint(&agent_jwk_val).expect("jkt");

    let envelope = NatsSignatureEnvelope {
        token: agent_jwt.clone(),
        sig_input: sig_input.to_string(),
        sig: String::new(),
        created,
        nonce: nonce.into(),
        content_digest: digest.clone(),
    };
    let canonical = envelope.canonical_base(subject, Some(reply), &jkt);
    let sig: Signature = agent_signing.sign(canonical.as_bytes());
    let sig_b64 = URL_SAFE_NO_PAD.encode(sig.to_bytes());

    // 4) Assemble inbound headers and verify.
    let headers_vec: Vec<(String, String)> = vec![
        (headers::NATS_TOKEN.to_string(), agent_jwt.clone()),
        (headers::NATS_SIG_INPUT.to_string(), sig_input.into()),
        (headers::NATS_SIG.to_string(), sig_b64),
        (headers::NATS_SIG_CREATED.to_string(), created.to_string()),
        (headers::NATS_SIG_NONCE.to_string(), nonce.into()),
        (headers::CONTENT_DIGEST.to_string(), digest),
    ];
    let req = NatsRequest {
        subject,
        reply: Some(reply),
        payload: payload.as_slice(),
        headers: NatsHeaders::new(&headers_vec),
    };

    let resolver = StaticJwks::new().with(ap_iss.clone(), ap_set);
    let verifier = NatsPopVerifier::new(resolver, clock.clone(), InMemoryReplayStore::new());
    let verified = verifier.verify(&req).await.expect("pop verifies");
    assert_eq!(verified.claims.sub, agent_sub);

    // 5) Replay must fail.
    let err = verifier.verify(&req).await.expect_err("replay");
    assert!(matches!(err, trogon_aauth_verify::nats_pop::NatsPopError::Replay));

    let _ = SystemTimeSource; // keep referenced
}

/// PKCS#8 PEM for a p256 SigningKey.
fn pkcs8_pem_from_signing_key(sk: &SigningKey) -> String {
    use p256::pkcs8::EncodePrivateKey;
    sk.to_pkcs8_pem(p256::pkcs8::LineEnding::LF).expect("pkcs8").to_string()
}
